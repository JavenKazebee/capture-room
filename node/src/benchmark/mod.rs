//! The benchmark runner: how many feeds this node can record with a set of
//! outputs, keep replay buffers of in a codec, or run as playout channels.
//!
//! Each feed loops real footage (test patterns compress unrealistically)
//! through its own monitor and legs or buffer, built like a real recording but
//! kept out of the [`SourceManager`]. A channel feed is a playout program at
//! the footage's format playing it with its NDI and SRT outputs. After a
//! warm-up each step measures source shortfall (frames not delivered, plus a
//! channel's late clip frames), output drops (encoder input queues), CPU,
//! memory and bytes written.
//!
//! The feed count doubles with quick checks until frames drop, then narrows
//! with full steps (see [`Search`](search::Search)). A run stops at the feed limit, short
//! memory or disk, an error, or a cancel (a real recording start cancels it).
//! Scratch files go beside where the outputs would write, so the same disks
//! are measured, and are deleted at the end.
//!
//! [`SourceManager`]: crate::sources::manager::SourceManager

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    BenchmarkRequest, BenchmarkRunDto, BenchmarkStatus, MediaInfo, OutputConfig, WsEvent,
};
use crate::capacity::{self, Setup};
use crate::db;
use crate::pipeline::profile::plan_legs;
use crate::state::AppState;

mod measure;
mod memory;
mod run;
mod search;

use memory::MemoryWatch;
use run::{dedup, remove_scratch, scratch_dir, scratch_dirs, Run};

const DEFAULT_MAX_FEEDS: u32 = 64;
const MAX_FEEDS_LIMIT: u32 = 128;
const DEFAULT_STEP_SECS: u32 = 20;
const DEFAULT_DROP_THRESHOLD_PCT: f64 = 0.5;
/// How long a recording start waits for a cancelled benchmark to tear down.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(15);

/// The benchmark running on this node; there's at most one.
pub struct Running {
    id: String,
    cancel: CancellationToken,
    /// Why it was cancelled, for its message.
    reason: Arc<Mutex<Option<String>>>,
    /// Becomes `true` once the run has torn down and saved its result.
    done: watch::Receiver<bool>,
}

/// Check a request, set up the run and start it. Refused while recording or
/// keeping replay buffers (they'd skew and compete) or while another runs.
pub async fn start(state: &Arc<AppState>, req: BenchmarkRequest) -> ApiResult<BenchmarkRunDto> {
    let setup = Setup::new(&req.outputs, req.replay, req.channel.as_deref());
    check_request(&req, setup)?;
    let path = req.media_path.trim().to_string();
    let media = crate::api::probe(path.clone()).await?;
    if media.fps_num == 0 {
        return Err(ApiError::bad("the footage has no fixed frame rate, so dropped frames can't be counted; pick another file"));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let (done_tx, done_rx) = watch::channel(false);
    let cancel = CancellationToken::new();
    let reason = Arc::new(Mutex::new(None));
    let slot = SlotGuard::claim(
        state,
        Running {
            id: id.clone(),
            cancel: cancel.clone(),
            reason: Arc::clone(&reason),
            done: done_rx,
        },
    )?;
    // Checked after taking the slot: a recording or buffer starting from here
    // on cancels this run instead.
    let channels_running = check_idle(state).await?;
    let memory = MemoryWatch::new();
    if memory.budget.short(memory.budget.baseline) {
        return Err(ApiError::Conflict(
            "this node is too short of memory to benchmark: close something and try again",
        ));
    }

    let (dto, scratch) = prepare(state, &req, setup, id, path, media).await?;
    info!(id = %dto.id, media = %dto.media_path, max_feeds = dto.max_feeds, "benchmark started");
    state.emit(&WsEvent::BenchmarkUpdated {
        run: Box::new(dto.clone()),
    });
    let run = Run {
        state: Arc::clone(state),
        dto: dto.clone(),
        scratch,
        cancel,
        reason,
        failure: Default::default(),
        memory: Mutex::new(memory),
        channels_running,
    };
    slot.disarm();
    tokio::spawn(run.run(done_tx));
    Ok(dto)
}

/// Refuse a request whose outputs, replay codec or channel don't make a
/// benchmark this node can run.
fn check_request(req: &BenchmarkRequest, setup: Setup<'_>) -> ApiResult<()> {
    match setup {
        Setup::Channel(outputs) => check_channel_benchmark(req, outputs),
        Setup::Replay(_) if !req.outputs.is_empty() => {
            Err(ApiError::bad("a replay buffer benchmark takes no outputs"))
        }
        Setup::Replay(codec) if !codec.available() => Err(ApiError::bad(
            "this node can't keep replay buffers in that codec",
        )),
        Setup::Replay(_) => Ok(()),
        Setup::Record([]) => Err(ApiError::bad("at least one output is required")),
        Setup::Record(outputs) => plan_legs(outputs, None).map(|_| ()).map_err(ApiError::bad),
    }
}

/// Refuse while the node records or keeps replay buffers. Returns how many
/// playout channels run: they always do, so they aren't stopped, and the
/// result says they ran.
async fn check_idle(state: &AppState) -> ApiResult<usize> {
    let mgr = state.source_manager.read().await;
    let load = mgr.running_load(&[], &[]);
    if !mgr.active_sessions().is_empty() {
        Err(ApiError::Conflict(
            "stop every recording on this node before benchmarking it",
        ))
    } else if !load.buffers.is_empty() {
        Err(ApiError::Conflict("turn off this node's replay buffers before benchmarking it: they would skew the result"))
    } else {
        Ok(load.channels.len())
    }
}

/// Create the run's scratch folders and save it as running.
async fn prepare(
    state: &AppState,
    req: &BenchmarkRequest,
    setup: Setup<'_>,
    id: String,
    media_path: String,
    media: MediaInfo,
) -> anyhow::Result<(BenchmarkRunDto, Vec<PathBuf>)> {
    let scratch = match setup {
        // A channel writes nothing.
        Setup::Channel(_) => Vec::new(),
        Setup::Replay(_) => {
            let root = state
                .source_manager
                .read()
                .await
                .replay_root()
                .to_path_buf();
            vec![scratch_dir(&root, &id)?]
        }
        Setup::Record(outputs) => scratch_dirs(outputs, &id, &state.node_name(), &media)?,
    };
    let dto = BenchmarkRunDto {
        id,
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        status: BenchmarkStatus::Running,
        preset_id: req.preset_id.clone(),
        preset_name: req.preset_name.clone(),
        outputs: req.outputs.clone(),
        replay: req.replay,
        // Keyed like the channels it stands for.
        channel: req.channel.as_deref().map(capacity::channel_outputs),
        media_path,
        media,
        max_feeds: req
            .max_feeds
            .unwrap_or(DEFAULT_MAX_FEEDS)
            .clamp(1, MAX_FEEDS_LIMIT),
        step_secs: req.step_secs.unwrap_or(DEFAULT_STEP_SECS).clamp(5, 300),
        drop_threshold_pct: req
            .drop_threshold_pct
            .unwrap_or(DEFAULT_DROP_THRESHOLD_PCT)
            .clamp(0.01, 10.0),
        feeds_running: 0,
        steps: Vec::new(),
        sustainable_feeds: 0,
        encoders: Vec::new(),
        message: None,
        scratch_dirs: dedup(&scratch)
            .iter()
            .map(|d| d.display().to_string())
            .collect(),
    };
    db::benchmark_save(&state.db, &dto).await?;
    Ok((dto, scratch))
}

/// Frees [`start`]'s benchmark slot when dropped, unless disarmed once the run
/// starts (it frees the slot itself when it ends).
struct SlotGuard<'a> {
    state: &'a AppState,
    armed: bool,
}

impl<'a> SlotGuard<'a> {
    /// Take the benchmark slot, unless another run has it.
    fn claim(state: &'a AppState, running: Running) -> ApiResult<Self> {
        let mut slot = state.benchmark.lock().unwrap();
        if slot.is_some() {
            return Err(ApiError::Conflict(
                "a benchmark is already running on this node",
            ));
        }
        *slot = Some(running);
        Ok(Self { state, armed: true })
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.state.benchmark.lock().unwrap().take();
        }
    }
}

/// Refuse a channel benchmark that also asks for recording outputs or a replay
/// codec, or for outputs it doesn't measure or this node can't send.
fn check_channel_benchmark(req: &BenchmarkRequest, outputs: &[OutputConfig]) -> ApiResult<()> {
    if !req.outputs.is_empty() || req.replay.is_some() {
        return Err(ApiError::bad(
            "a channel benchmark takes no recording outputs or replay codec",
        ));
    }
    for o in outputs {
        match o {
            OutputConfig::Ndi { .. } if !crate::plugins::has("ndisink") => {
                return Err(ApiError::bad("this node can't send NDI (no NDI plugin)"));
            }
            OutputConfig::Srt { .. } if !crate::plugins::has("srtsink") => {
                return Err(ApiError::bad("this node can't send SRT (no SRT plugin)"));
            }
            OutputConfig::Ndi { .. } | OutputConfig::Srt { .. } => {}
            _ => {
                return Err(ApiError::bad("a channel benchmark measures NDI and SRT outputs only: RTSP and WHEP encode only for viewers, and a display needs a screen"));
            }
        }
    }
    if !["compositor", "audiomixer"]
        .into_iter()
        .all(crate::plugins::has)
    {
        return Err(ApiError::bad("this node can't run playout channels"));
    }
    Ok(())
}

/// Cancel the running benchmark (only if it's `id`, when given) and wait for
/// it to tear down. Returns whether one was cancelled.
pub async fn cancel(state: &AppState, id: Option<&str>, reason: &str) -> bool {
    let done = {
        let slot = state.benchmark.lock().unwrap();
        let Some(running) = slot.as_ref().filter(|r| id.is_none_or(|id| r.id == id)) else {
            return false;
        };
        running
            .reason
            .lock()
            .unwrap()
            .get_or_insert_with(|| reason.to_string());
        running.cancel.cancel();
        running.done.clone()
    };
    let mut done = done;
    let finished = tokio::time::timeout(CANCEL_TIMEOUT, done.wait_for(|d| *d)).await;
    if finished.is_err() {
        warn!("benchmark didn't finish tearing down within {CANCEL_TIMEOUT:?}");
    }
    true
}

pub fn running_id(state: &AppState) -> Option<String> {
    state
        .benchmark
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.id.clone())
}

/// After a crash or restart: runs left `running` end as errors, and their
/// scratch folders are deleted.
pub async fn recover(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    for mut run in db::benchmarks_running(pool).await? {
        let dirs = run
            .scratch_dirs
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || remove_scratch(&dirs)).await?;
        run.status = BenchmarkStatus::Error;
        run.finished_at = Some(chrono::Utc::now().to_rfc3339());
        run.feeds_running = 0;
        run.message = Some("The node restarted during the benchmark.".into());
        db::benchmark_save(pool, &run).await?;
    }
    Ok(())
}
