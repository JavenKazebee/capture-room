//! The benchmark runner: how many feeds this node can record with a set of
//! outputs, keep replay buffers of in a codec, or run as playout channels.
//!
//! Each feed is a looping media file (real footage: test patterns compress
//! unrealistically) with its own monitor and recording legs (or replay
//! buffer), built the same way as a real recording but kept out of the
//! [`SourceManager`], so they never show up as sources. A channel feed is a
//! playout channel's program instead (the compositor and mixer at the
//! footage's format) playing the footage on loop through its player, with
//! its NDI and SRT outputs. Each step runs some number of feeds and, after a
//! warm-up, measures:
//!
//! - **Source shortfall:** frames the feeds didn't deliver, from the
//!   monitors' frame counts against the footage's frame rate. Decoding (or
//!   the monitors) fell behind. For a channel, also the clip frames that
//!   reached its program too late to be shown.
//! - **Output drops:** frames the legs' (or buffers' or channel outputs')
//!   input queues dropped because an encoder couldn't keep up.
//! - CPU, memory, and what the legs wrote per second.
//!
//! The feed count is searched for (see [`Search`]): doubled with quick checks
//! until frames drop, then narrowed down with full-length steps, so a node
//! that sustains 40 feeds takes about a dozen steps, not 40. A full step that
//! drops frames is measured once more before it counts, and the answer is
//! always confirmed by a full step. The search is capped by the feed limit,
//! and by memory or a scratch volume running short; a run also ends on an
//! error, or when cancelled — starting a real recording cancels it, since
//! real recordings always win. Files go to a
//! scratch folder beside where the outputs would write (so the same disks are
//! measured) and are deleted at the end.
//!
//! [`SourceManager`]: crate::sources::manager::SourceManager

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use gstreamer as gst;
use sysinfo::System;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    BenchmarkRequest, BenchmarkRunDto, BenchmarkStatus, BenchmarkStepDto, ChannelConfig, ClipEnd,
    FileSourceConfig, LiveVideoFormat, MediaInfo, OutputConfig, PresetOutputInput, WsEvent,
};
use crate::capacity;
use crate::db;
use crate::pipeline::monitor::{MonitorPipeline, VideoProgress};
use crate::pipeline::output::{self, OutputContext, OutputLeg, Program};
use crate::pipeline::profile::{plan_legs, PathVars, RecordingProfile};
use crate::pipeline::recording::{self, OnLegError, OnLegFile, RecordingLeg};
use crate::pipeline::replay::{Index, ReplayIndex, ReplayLeg};
use crate::sources::channel::{ChannelSource, ClipRequest, PlayedProgress, Playout};
use crate::sources::file::{probe, FileSource};
use crate::sources::InputSource;
use crate::state::AppState;

const DEFAULT_MAX_FEEDS: u32 = 64;
const MAX_FEEDS_LIMIT: u32 = 128;
const DEFAULT_STEP_SECS: u32 = 20;
const DEFAULT_DROP_THRESHOLD_PCT: f64 = 0.5;
/// Settling time after changing the feed count, before measuring: encoders
/// start up, queues fill…
const WARMUP: Duration = Duration::from_secs(5);
/// …plus a second for every this many feeds started, so starting a lot at
/// once doesn't fail a step that would otherwise pass.
const WARMUP_FEEDS_PER_SEC: u32 = 4;
/// How long a quick check measures, while doubling the feeds.
const QUICK_SECS: u32 = 5;
/// A quick check counts as a pass only well clear of the limit: under half
/// the drop threshold, and below this much CPU. Anything closer is measured
/// in full.
const QUICK_MAX_CPU_PCT: f64 = 85.0;
/// A quick check this many times over the drop threshold fails outright.
const QUICK_FAIL_FACTOR: f64 = 4.0;
/// Times a full step is measured before it fails.
const MEASURE_ATTEMPTS: u32 = 2;
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);
/// A run stops (and keeps its result) before a scratch volume gets this full.
const MIN_FREE_BYTES: u64 = 2_000_000_000;
/// The machine's available memory is never let fall below this…
const MIN_FREE_MEMORY: u64 = 3 << 29;
/// …or this share of all of it, whichever is more. See [`MemoryBudget`].
const MIN_FREE_MEMORY_SHARE: u64 = 8;
/// A feed is assumed to need this much more than the most any has used so
/// far, when deciding whether more of them fit.
const MEMORY_MARGIN: f64 = 1.25;
/// How often memory is looked at while feeds run.
const MEMORY_POLL: Duration = Duration::from_millis(500);
/// How long a recording start waits for a cancelled benchmark to tear down.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(15);
/// Scratch folders are named this plus the start of the run id. Only folders
/// named so are ever deleted.
const SCRATCH_PREFIX: &str = ".capture-room-benchmark-";
/// How much a replay buffer feed keeps. Old segments are deleted as they
/// would be in a real buffer, and the scratch volume doesn't fill.
const REPLAY_KEEP: gst::ClockTime = gst::ClockTime::from_seconds(30);

/// The benchmark running on this node; there's at most one.
pub struct Running {
    id: String,
    cancel: CancellationToken,
    /// Why it was cancelled, for its message.
    reason: Arc<Mutex<Option<String>>>,
    /// Becomes `true` once the run has torn down and saved its result.
    done: watch::Receiver<bool>,
}

// ── Control ───────────────────────────────────────────────────────────────────

/// Check a request, set up the run and start it. Refused while recording or
/// keeping replay buffers (the result would be skewed, and the benchmark
/// would compete with them) or while another benchmark runs.
pub async fn start(state: &Arc<AppState>, req: BenchmarkRequest) -> ApiResult<BenchmarkRunDto> {
    if let Some(outputs) = &req.channel {
        check_channel(&req, outputs)?;
    }
    match req.replay {
        Some(_) if !req.outputs.is_empty() => {
            return Err(ApiError::BadRequest(
                "a replay buffer benchmark takes no outputs".into(),
            ));
        }
        Some(codec) if !codec.available() => {
            return Err(ApiError::BadRequest(
                "this node can't keep replay buffers in that codec".into(),
            ));
        }
        Some(_) => {}
        None if req.channel.is_some() => {}
        None if req.outputs.is_empty() => {
            return Err(ApiError::BadRequest(
                "at least one output is required".into(),
            ));
        }
        None => {
            plan_legs(&req.outputs, None).map_err(|e| ApiError::BadRequest(e.into()))?;
        }
    }
    let path = req.media_path.trim().to_string();
    let probe_path = path.clone();
    let media = tokio::task::spawn_blocking(move || probe(&probe_path))
        .await?
        .map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
    if media.fps_num == 0 {
        return Err(ApiError::BadRequest(
            "the footage has no fixed frame rate, so dropped frames can't be counted; pick another file".into(),
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let (done_tx, done_rx) = watch::channel(false);
    let cancel = CancellationToken::new();
    let reason = Arc::new(Mutex::new(None));
    {
        let mut slot = state.benchmark.lock().unwrap();
        if slot.is_some() {
            return Err(ApiError::Conflict(
                "a benchmark is already running on this node",
            ));
        }
        *slot = Some(Running {
            id: id.clone(),
            cancel: cancel.clone(),
            reason: Arc::clone(&reason),
            done: done_rx,
        });
    }
    // Checked after taking the slot: a recording or replay buffer that
    // starts from here on cancels this run instead.
    let busy = {
        let mgr = state.source_manager.read().await;
        if !mgr.active_sessions().is_empty() {
            Some("stop every recording on this node before benchmarking it")
        } else if !mgr.running_load(&[], &[]).buffers.is_empty() {
            Some("turn off this node's replay buffers before benchmarking it: they would skew the result")
        } else {
            None
        }
    };
    if let Some(why) = busy {
        state.benchmark.lock().unwrap().take();
        return Err(ApiError::Conflict(why));
    }
    // Channels always run, so they aren't stopped for a benchmark; the
    // result says they ran.
    let channels_running = state
        .source_manager
        .read()
        .await
        .running_load(&[], &[])
        .channels
        .len();
    let memory = MemoryWatch::new();
    if memory.budget.short(memory.budget.baseline) {
        state.benchmark.lock().unwrap().take();
        return Err(ApiError::Conflict(
            "this node is too short of memory to benchmark: close something and try again",
        ));
    }

    let setup = async {
        let scratch = match req.replay {
            // A channel writes nothing.
            _ if req.channel.is_some() => Vec::new(),
            Some(_) => {
                let root = state
                    .source_manager
                    .read()
                    .await
                    .replay_root()
                    .to_path_buf();
                vec![scratch_dir(&root, &id)?]
            }
            None => scratch_dirs(&req.outputs, &id, &state.node_name(), &media)?,
        };
        let dto = BenchmarkRunDto {
            id: id.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            status: BenchmarkStatus::Running,
            preset_id: req.preset_id.clone(),
            preset_name: req.preset_name.clone(),
            outputs: req.outputs.clone(),
            replay: req.replay,
            // What's measured, so the run is keyed like the channels it
            // stands for.
            channel: req.channel.as_deref().map(capacity::channel_outputs),
            media_path: path.clone(),
            media: media.clone(),
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
        anyhow::Ok((dto, scratch))
    };
    let (dto, scratch) = match setup.await {
        Ok(v) => v,
        Err(e) => {
            state.benchmark.lock().unwrap().take();
            return Err(e.into());
        }
    };

    info!(id = %id, media = %path, max_feeds = dto.max_feeds, "benchmark started");
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
    tokio::spawn(run.run(done_tx));
    Ok(dto)
}

/// Refuse a channel benchmark that can't run: one that also asks for
/// recording outputs or a replay codec, or for outputs it doesn't measure
/// (see [`capacity::channel_outputs`]) or this node can't send.
fn check_channel(req: &BenchmarkRequest, outputs: &[OutputConfig]) -> ApiResult<()> {
    if !req.outputs.is_empty() || req.replay.is_some() {
        return Err(ApiError::BadRequest(
            "a channel benchmark takes no recording outputs or replay codec".into(),
        ));
    }
    let installed = |name: &str| gst::ElementFactory::find(name).is_some();
    for o in outputs {
        match o {
            OutputConfig::Ndi { .. } if !installed("ndisink") => {
                return Err(ApiError::BadRequest(
                    "this node can't send NDI (no NDI plugin)".into(),
                ));
            }
            OutputConfig::Srt { .. } if !installed("srtsink") => {
                return Err(ApiError::BadRequest(
                    "this node can't send SRT (no SRT plugin)".into(),
                ));
            }
            OutputConfig::Ndi { .. } | OutputConfig::Srt { .. } => {}
            _ => {
                return Err(ApiError::BadRequest(
                    "a channel benchmark measures NDI and SRT outputs only: RTSP and WHEP encode only for viewers, and a display needs a screen".into(),
                ));
            }
        }
    }
    if !["compositor", "audiomixer"].into_iter().all(installed) {
        return Err(ApiError::BadRequest(
            "this node can't run playout channels".into(),
        ));
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

/// The id of the running benchmark, if any.
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

// ── Run ───────────────────────────────────────────────────────────────────────

/// One feed: a looping file (or a channel's program) with its monitor, and
/// what it feeds.
struct Feed {
    monitor: Arc<MonitorPipeline>,
    work: Work,
}

/// What a feed's footage goes to.
enum Work {
    Record(Vec<RecordingLeg>),
    /// A replay buffer. The leg is dropped before the index it writes into.
    Replay(ReplayLeg, ReplayIndex),
    /// A playout channel playing the footage, and its outputs. Its clip is
    /// stopped before it's dropped (see [`discard_feeds`]).
    Channel(Arc<Playout>, Vec<OutputLeg>),
}

impl Feed {
    fn dropped_frames(&self) -> u64 {
        match &self.work {
            Work::Record(legs) => legs.iter().map(RecordingLeg::dropped_frames).sum(),
            Work::Replay(leg, _) => leg.dropped_frames(),
            Work::Channel(_, outputs) => outputs.iter().filter_map(OutputLeg::dropped_frames).sum(),
        }
    }

    /// The encoder elements it uses, one per output (a channel's outputs,
    /// by name).
    fn encoders(&self) -> Vec<String> {
        match &self.work {
            Work::Record(legs) => legs.iter().map(|l| l.encoder().to_string()).collect(),
            Work::Replay(leg, _) => vec![leg.encoder().to_string()],
            Work::Channel(_, outputs) => outputs.iter().map(|o| o.label().to_string()).collect(),
        }
    }

    /// Why it failed, if it has. Recording legs report theirs as they go.
    fn error(&self) -> Option<String> {
        self.monitor.error().or_else(|| match &self.work {
            Work::Record(_) => None,
            Work::Replay(leg, _) => leg.error().map(|e| format!("replay buffer: {e}")),
            Work::Channel(playout, outputs) => playout
                .status()
                .error
                .map(|e| format!("playing the footage: {e}"))
                .or_else(|| {
                    outputs
                        .iter()
                        .find_map(|o| Some(format!("{}: {}", o.label(), o.error()?)))
                }),
        })
    }

    /// How a channel's clip has reached its program.
    fn played(&self) -> Option<PlayedProgress> {
        match &self.work {
            Work::Channel(playout, _) => Some(playout.played()),
            _ => None,
        }
    }
}

/// Why a run ended early.
enum Stop {
    Cancelled,
    /// The machine's memory ran short: the step is abandoned, not the run.
    ShortOfMemory,
    Failed(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Stop {
    fn from(e: E) -> Self {
        Self::Failed(e.into())
    }
}

struct Run {
    state: Arc<AppState>,
    dto: BenchmarkRunDto,
    /// Per output, the folder its files go in.
    scratch: Vec<PathBuf>,
    cancel: CancellationToken,
    reason: Arc<Mutex<Option<String>>>,
    /// The first leg error, reported from a streaming thread.
    failure: Arc<Mutex<Option<String>>>,
    memory: Mutex<MemoryWatch>,
    /// Playout channels running on the node when the run started.
    channels_running: usize,
}

impl Run {
    async fn run(mut self, done: watch::Sender<bool>) {
        let mut feeds = Vec::new();
        let outcome = self.search(&mut feeds).await;

        discard_feeds(feeds).await;
        let dirs = dedup(&self.scratch);
        if let Err(e) = tokio::task::spawn_blocking(move || remove_scratch(&dirs)).await {
            error!(error = %e, "benchmark cleanup panicked");
        }

        let dto = &mut self.dto;
        dto.finished_at = Some(chrono::Utc::now().to_rfc3339());
        dto.feeds_running = 0;
        match outcome {
            Ok(message) => {
                dto.status = BenchmarkStatus::Completed;
                dto.message = Some(message);
            }
            Err(Stop::Cancelled) => {
                dto.status = BenchmarkStatus::Cancelled;
                dto.message = Some(
                    self.reason
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| "Cancelled.".into()),
                );
            }
            Err(Stop::Failed(e)) => {
                dto.status = BenchmarkStatus::Error;
                dto.message = Some(format!("{e:#}"));
            }
            Err(Stop::ShortOfMemory) => unreachable!("handled by the search"),
        }
        info!(id = %dto.id, status = ?dto.status, sustainable = dto.sustainable_feeds, message = ?dto.message, "benchmark finished");
        self.publish().await;
        self.state
            .benchmark
            .lock()
            .unwrap()
            .take_if(|r| r.id == self.dto.id);
        let _ = done.send(true);
    }

    /// Search for the most feeds the node sustains. Returns the run's closing
    /// message.
    async fn search(&mut self, feeds: &mut Vec<Feed>) -> Result<String, Stop> {
        let mut search = Search::new(self.dto.max_feeds);
        let mut system = System::new();
        // Why the search was capped below the feed limit.
        let mut capped: Option<String> = None;

        loop {
            let (n, full) = match search.next() {
                Next::Done(n) => {
                    self.dto.sustainable_feeds = n;
                    let mut message = self.closing_message(n, &search, capped.as_deref());
                    if self.channels_running > 0 {
                        let c = self.channels_running as u32;
                        message.push_str(&format!(
                            " {c} playout channel{} ran on the node meanwhile, taking some of it.",
                            plural(c)
                        ));
                    }
                    return Ok(message);
                }
                Next::Measure { feeds, full } => (feeds, full),
            };

            // More feeds than have run so far: only if they'd fit in memory.
            if n > search.best() {
                let fit = self.memory.lock().unwrap().budget.fits();
                if let Some(fit) = fit.filter(|&fit| n > fit) {
                    info!(
                        feeds = n,
                        fit, "benchmark capped: more feeds wouldn't fit in memory"
                    );
                    search.cap(fit.max(search.best()));
                    capped = Some(format!(
                        "{n} feeds wouldn't fit in the memory left (about {fit} would)"
                    ));
                    continue;
                }
            }

            match self.step(feeds, n, full, &mut system).await {
                Ok(Some(full)) => {
                    search.passed(n, full);
                    if let Some(volume) = self.low_volume().await {
                        search.cap(n);
                        capped = Some(format!("{volume} is nearly full"));
                    }
                }
                Ok(None) => search.failed(n),
                Err(Stop::ShortOfMemory) => {
                    warn!(feeds = n, "benchmark step stopped: memory ran short");
                    // Back below this count at once, so memory recovers.
                    let back = search.best().min(n - 1);
                    discard_feeds(feeds.drain(back as usize..).collect()).await;
                    self.dto.feeds_running = back;
                    search.failed(n);
                    capped = Some(format!("memory ran short at {n} feed{}", plural(n)));
                }
                Err(e) => return Err(e),
            }
            self.dto.sustainable_feeds = search.best();
            self.publish().await;
        }
    }

    /// Run `n` feeds and measure them (see [`Self::check`]), noting how much
    /// memory they take.
    async fn step(
        &mut self,
        feeds: &mut Vec<Feed>,
        n: u32,
        full: bool,
        system: &mut System,
    ) -> Result<Option<bool>, Stop> {
        let started = self.set_feeds(feeds, n).await?;
        self.dto.feeds_running = n;
        self.publish().await;
        let warmup = WARMUP + Duration::from_secs(started.div_ceil(WARMUP_FEEDS_PER_SEC) as u64);
        self.pause(feeds, warmup).await?;
        let result = self.check(feeds, n, full, system).await?;
        self.memory.lock().unwrap().observe(n);
        Ok(result)
    }

    /// Measure `n` feeds: a quick check that passes only well clear of the
    /// limit (and fails outright far over it), else full-length steps, one
    /// more if the first drops frames.
    /// Returns `Some(full)` if they kept up — `full` when a full-length step
    /// showed it — or `None` if they didn't.
    async fn check(
        &mut self,
        feeds: &[Feed],
        n: u32,
        full: bool,
        system: &mut System,
    ) -> Result<Option<bool>, Stop> {
        if !full {
            let mut step = self.measure_window(feeds, n, QUICK_SECS, system).await?;
            step.quick = true;
            step.passed = step.drop_pct <= self.dto.drop_threshold_pct / 2.0
                && step.cpu_pct < QUICK_MAX_CPU_PCT;
            let passed = step.passed;
            info!(
                feeds = n,
                drop_pct = step.drop_pct,
                cpu = step.cpu_pct,
                passed,
                "benchmark quick check"
            );
            self.dto.steps.push(step);
            if passed {
                return Ok(Some(false));
            }
            // Far over the limit: no need for full steps to say so.
            if self.dto.steps.last().unwrap().drop_pct
                > self.dto.drop_threshold_pct * QUICK_FAIL_FACTOR
            {
                return Ok(None);
            }
            self.publish().await;
        }
        for attempt in 1..=MEASURE_ATTEMPTS {
            let step = self
                .measure_window(feeds, n, self.dto.step_secs, system)
                .await?;
            let passed = step.passed;
            info!(
                feeds = n,
                attempt,
                drop_pct = step.drop_pct,
                cpu = step.cpu_pct,
                passed,
                "benchmark step"
            );
            self.dto.steps.push(step);
            if passed {
                return Ok(Some(true));
            }
            if attempt < MEASURE_ATTEMPTS {
                self.publish().await;
            }
        }
        Ok(None)
    }

    /// Measure the running feeds for `secs`.
    async fn measure_window(
        &self,
        feeds: &[Feed],
        n: u32,
        secs: u32,
        system: &mut System,
    ) -> Result<BenchmarkStepDto, Stop> {
        let fps = self.dto.media.fps_num as f64 / self.dto.media.fps_den as f64;
        let replay = self.dto.replay.is_some();
        let outputs = match (&self.dto.channel, replay) {
            (Some(channel), _) => channel.len(),
            (None, true) => 1,
            (None, false) => self.dto.outputs.len(),
        };
        let before = Snapshot::take(feeds, replay).await?;
        system.refresh_cpu_usage();
        let mut cpu = Vec::new();
        for _ in 0..secs {
            self.pause(feeds, Duration::from_secs(1)).await?;
            system.refresh_cpu_usage();
            cpu.push(system.global_cpu_usage() as f64);
        }
        let after = Snapshot::take(feeds, replay).await?;
        system.refresh_memory();
        let memory_pct = system.used_memory() as f64 / system.total_memory().max(1) as f64 * 100.0;
        let threshold = self.dto.drop_threshold_pct;
        Ok(measure(
            n, fps, outputs, &before, &after, &cpu, memory_pct, threshold,
        ))
    }

    /// Start or stop feeds until `target` run. Returns how many were started.
    async fn set_feeds(&mut self, feeds: &mut Vec<Feed>, target: u32) -> Result<u32, Stop> {
        let target = target as usize;
        if feeds.len() > target {
            discard_feeds(feeds.drain(target..).collect()).await;
        }
        let mut started = 0;
        while feeds.len() < target {
            if self.memory.lock().unwrap().short() {
                return Err(Stop::ShortOfMemory);
            }
            let feed = self.add_feed(feeds.len() as u32 + 1).await?;
            if self.dto.encoders.is_empty() {
                self.dto.encoders = feed.encoders();
            }
            feeds.push(feed);
            started += 1;
        }
        Ok(started)
    }

    /// Why the search ended at `n` feeds.
    fn closing_message(&self, n: u32, search: &Search, capped: Option<&str>) -> String {
        let threshold = self.dto.drop_threshold_pct;
        // The full step that failed at the lowest failing count.
        let failed = search.fail.and_then(|hi| {
            self.dto
                .steps
                .iter()
                .rev()
                .find(|s| s.feeds == hi && !s.quick && !s.passed)
        });
        match (failed, capped) {
            (Some(step), _) if n == 0 => {
                format!(
                    "Even 1 feed dropped {:.2}% of frames (the limit is {threshold}%).",
                    step.drop_pct
                )
            }
            (Some(step), _) => format!(
                "{n} feed{} kept up; {} dropped {:.2}% of frames (the limit is {threshold}%).",
                plural(n),
                step.feeds,
                step.drop_pct,
            ),
            (None, Some(reason)) => format!("Stopped at {n} feed{}: {reason}.", plural(n)),
            (None, None) => {
                format!(
                    "All {n} feed{} kept up. Raise the feed limit to find where this node stops.",
                    plural(n)
                )
            }
        }
    }

    /// Start feed `n` and its legs, once its monitor is delivering video.
    async fn add_feed(&self, n: u32) -> Result<Feed, Stop> {
        if let Some(outputs) = &self.dto.channel {
            return self.add_channel(n, outputs).await;
        }
        let source = FileSource::new(
            format!("benchmark-{n}"),
            format!("Benchmark feed {n}"),
            FileSourceConfig {
                path: self.dto.media_path.clone(),
                media: Some(self.dto.media.clone()),
            },
        );
        let monitor = self.start_monitor(n, &source).await?;

        if let Some(codec) = self.dto.replay {
            let index = Index::create(&self.scratch[0].join(format!("feed{n:02}")), REPLAY_KEEP);
            let leg = ReplayLeg::start(&monitor, codec, &index, &format!("bench-{n}"))
                .with_context(|| format!("start feed {n}'s replay buffer"))?;
            return Ok(Feed {
                monitor,
                work: Work::Replay(leg, index),
            });
        }

        let legs = self.feed_legs(n, &monitor)?;
        let failure = Arc::clone(&self.failure);
        let on_error: OnLegError = Arc::new(move |path, error| {
            failure
                .lock()
                .unwrap()
                .get_or_insert_with(|| format!("{}: {error}", path.display()));
        });
        let on_file: OnLegFile = Arc::new(|| {});
        let legs = recording::start_legs(
            &monitor,
            &format!("bench-{n}"),
            &legs,
            &on_error,
            &on_file,
            None,
            None,
        )
        .with_context(|| format!("start feed {n}'s outputs"))?;
        Ok(Feed {
            monitor,
            work: Work::Record(legs),
        })
    }

    /// Run `source` for feed `n`, once its monitor is delivering video.
    async fn start_monitor(
        &self,
        n: u32,
        source: &dyn InputSource,
    ) -> Result<Arc<MonitorPipeline>, Stop> {
        let config = *self.state.source_manager.read().await.monitor_config();
        let monitor = MonitorPipeline::new(source, &config, &self.state.clock.clock())
            .with_context(|| format!("start feed {n}"))?;

        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        while monitor.video_frames() == 0 {
            if let Some(e) = monitor.error() {
                return Err(Stop::Failed(anyhow!("feed {n}: {e}")));
            }
            if Instant::now() > deadline {
                return Err(Stop::Failed(anyhow!(
                    "feed {n} delivered no video within {FIRST_FRAME_TIMEOUT:?}"
                )));
            }
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(Stop::Cancelled),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
        Ok(Arc::new(monitor))
    }

    /// Start feed `n` as a playout channel at the footage's format, with
    /// `outputs` (each its own: a numbered NDI name, an SRT listener on a
    /// free local port), playing the footage on loop.
    async fn add_channel(&self, n: u32, outputs: &[OutputConfig]) -> Result<Feed, Stop> {
        let m = &self.dto.media;
        let channel = ChannelSource::new(
            format!("benchmark-{n}"),
            format!("Benchmark channel {n}"),
            ChannelConfig {
                format: LiveVideoFormat {
                    width: m.width,
                    height: m.height,
                    fps_num: m.fps_num,
                    fps_den: m.fps_den,
                },
                audio_channels: if m.audio_channels == 0 {
                    2
                } else {
                    m.audio_channels.min(8)
                },
                outputs: Vec::new(),
            },
        );
        let monitor = self.start_monitor(n, &channel).await?;
        let program = Program::from(&channel.capabilities().context("channel format")?);
        let (displays, devices) = self.state.source_manager.read().await.output_parts();
        let context = OutputContext {
            displays: &displays,
            devices: &devices,
        };
        let legs = outputs
            .iter()
            .map(|o| {
                let config = feed_output(o, n)?;
                let sink = output::sink(&config, channel.display_name(), program, &context);
                OutputLeg::start(&monitor, sink.as_ref())
                    .with_context(|| format!("start feed {n}'s {}", sink.label()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let playout = channel
            .playout()
            .context("a channel has a transport")?
            .clone();
        let clip = ClipRequest {
            media_id: "benchmark".into(),
            name: "Benchmark footage".into(),
            path: self.dto.media_path.clone(),
            in_point: gst::ClockTime::ZERO,
            out_point: None,
            end: ClipEnd::Loop,
            rate: 1.0,
            replay: None,
        };
        if let Err(e) = playout.load(clip, true).await {
            playout.stop().await;
            return Err(Stop::Failed(e.context(format!("play feed {n}'s footage"))));
        }
        Ok(Feed {
            monitor,
            work: Work::Channel(playout, legs),
        })
    }

    /// Feed `n`'s legs: the run's outputs, writing into the scratch folders.
    fn feed_legs(
        &self,
        n: u32,
        monitor: &MonitorPipeline,
    ) -> anyhow::Result<Vec<(PathBuf, RecordingProfile)>> {
        let outputs: Vec<PresetOutputInput> = self
            .dto
            .outputs
            .iter()
            .zip(&self.scratch)
            .enumerate()
            .map(|(i, (o, dir))| PresetOutputInput {
                path_template: dir
                    .join(format!("feed{n:02}_out{}.{{ext}}", i + 1))
                    .display()
                    .to_string(),
                ..o.clone()
            })
            .collect();
        let format = monitor.source_format();
        let mut vars = path_vars(&self.dto.media, &self.state.node_name(), n);
        vars.source_resolution = format.size.or(vars.source_resolution);
        vars.source_framerate = format.rate.or(vars.source_framerate);
        vars.source_audio = format.audio.or(vars.source_audio);
        plan_legs(&outputs, Some(&vars)).map_err(|e| anyhow!(e))
    }

    /// Wait `duration`, returning early on a cancel, a failure, or memory
    /// running short (checked every [`MEMORY_POLL`]).
    async fn pause(&self, feeds: &[Feed], duration: Duration) -> Result<(), Stop> {
        let until = tokio::time::Instant::now() + duration;
        loop {
            let wake = until.min(tokio::time::Instant::now() + MEMORY_POLL);
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(Stop::Cancelled),
                _ = tokio::time::sleep_until(wake) => {}
            }
            if self.memory.lock().unwrap().short() {
                return Err(Stop::ShortOfMemory);
            }
            if wake >= until {
                break;
            }
        }
        if let Some(e) = self.failure.lock().unwrap().clone() {
            return Err(Stop::Failed(anyhow!("an output failed: {e}")));
        }
        if let Some((i, e)) = feeds
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.error().map(|e| (i, e)))
        {
            return Err(Stop::Failed(anyhow!("feed {} failed: {e}", i + 1)));
        }
        Ok(())
    }

    /// A scratch volume with too little space left to carry on, if any.
    async fn low_volume(&self) -> Option<String> {
        let dirs = dedup(&self.scratch);
        tokio::task::spawn_blocking(move || {
            let volumes = crate::storage::list_volumes();
            dirs.iter().find_map(|dir| {
                let v = &volumes[crate::storage::volume_of(dir, &volumes)?];
                (v.available_bytes < MIN_FREE_BYTES).then(|| v.mount_point.clone())
            })
        })
        .await
        .ok()
        .flatten()
    }

    /// Save the run's state and send it to the UI.
    async fn publish(&self) {
        if let Err(e) = db::benchmark_save(&self.state.db, &self.dto).await {
            error!(error = %e, "save benchmark");
        }
        self.state.emit(&WsEvent::BenchmarkUpdated {
            run: Box::new(self.dto.clone()),
        });
    }
}

/// Tear feeds down. Legs are discarded rather than finished: their files are
/// thrown away, so there's nothing to finalize.
async fn discard_feeds(feeds: Vec<Feed>) {
    // A channel's clip first: its player's bus task holds the transport.
    for feed in &feeds {
        if let Work::Channel(playout, _) = &feed.work {
            playout.stop().await;
        }
    }
    let teardown = tokio::task::spawn_blocking(move || {
        for feed in feeds {
            match feed.work {
                Work::Record(legs) => legs.into_iter().for_each(RecordingLeg::discard),
                Work::Replay(leg, index) => {
                    drop(leg);
                    drop(index);
                }
                Work::Channel(playout, outputs) => {
                    drop(outputs);
                    drop(playout);
                }
            }
            drop(feed.monitor);
        }
    });
    if let Err(e) = teardown.await {
        error!(error = %e, "benchmark teardown panicked");
    }
}

// ── Search ────────────────────────────────────────────────────────────────────

/// What to measure next.
#[derive(Debug, PartialEq)]
enum Next {
    /// Run this many feeds and measure them: in full, or with a quick check
    /// first.
    Measure { feeds: u32, full: bool },
    /// The search is over: the node sustains this many.
    Done(u32),
}

/// The search for the most feeds a node sustains. Doubles the feeds (1, 2,
/// 4, …) with quick checks until a count fails or the limit is reached, then
/// halves the gap between the most that passed and the fewest that failed
/// with full-length steps. The answer must have passed a full step; if one
/// that only passed a quick check fails it, the search carries on below.
struct Search {
    limit: u32,
    /// Counts that passed, and whether a full step showed it.
    passes: BTreeMap<u32, bool>,
    /// The fewest feeds that failed.
    fail: Option<u32>,
}

impl Search {
    fn new(limit: u32) -> Self {
        Self {
            limit: limit.max(1),
            passes: BTreeMap::new(),
            fail: None,
        }
    }

    /// The most feeds that have passed so far.
    fn best(&self) -> u32 {
        self.passes.keys().next_back().copied().unwrap_or(0)
    }

    fn next(&self) -> Next {
        let lo = self.best();
        let narrowing = self.fail.is_some_and(|hi| hi - lo > 1);
        if narrowing {
            let hi = self.fail.unwrap();
            return Next::Measure {
                feeds: lo + (hi - lo) / 2,
                full: true,
            };
        }
        let settled = self.fail.is_some() || lo >= self.limit;
        if !settled {
            let feeds = if lo == 0 { 1 } else { (lo * 2).min(self.limit) };
            return Next::Measure { feeds, full: false };
        }
        if lo == 0 || self.passes[&lo] {
            Next::Done(lo)
        } else {
            Next::Measure {
                feeds: lo,
                full: true,
            }
        }
    }

    fn passed(&mut self, feeds: u32, full: bool) {
        let entry = self.passes.entry(feeds).or_default();
        *entry |= full;
    }

    fn failed(&mut self, feeds: u32) {
        self.fail = Some(self.fail.map_or(feeds, |f| f.min(feeds)));
        self.passes.retain(|&n, _| n < feeds);
    }

    /// Go no higher than `feeds` (memory or disk is running short).
    fn cap(&mut self, feeds: u32) {
        self.limit = self.limit.min(feeds);
    }
}

// ── Memory ────────────────────────────────────────────────────────────────────

/// How much memory feeds take, and how many more fit. Each feed (a decoder,
/// a monitor, encoders, and on macOS VideoToolbox's own service) takes
/// memory outside what a step measures, and an encoder that keeps up never
/// fails a step, so without this a run would add feeds until the machine
/// ran out.
#[derive(Debug, Clone, Copy)]
struct MemoryBudget {
    /// Available before any feed ran.
    baseline: u64,
    /// Never go below this much available.
    floor: u64,
    /// The most any one feed has taken so far: the drop from the baseline
    /// over the feeds running.
    per_feed: u64,
}

impl MemoryBudget {
    fn new(total: u64, available: u64) -> Self {
        Self {
            baseline: available,
            floor: MIN_FREE_MEMORY.max(total / MIN_FREE_MEMORY_SHARE),
            per_feed: 0,
        }
    }

    fn short(&self, available: u64) -> bool {
        available < self.floor
    }

    fn observe(&mut self, available: u64, feeds: u32) {
        if feeds > 0 {
            let used = self.baseline.saturating_sub(available) / u64::from(feeds);
            self.per_feed = self.per_feed.max(used);
        }
    }

    /// How many feeds fit, with a margin; `None` until feeds have shown
    /// what they take.
    fn fits(&self) -> Option<u32> {
        (self.per_feed > 0).then(|| {
            let room = self.baseline.saturating_sub(self.floor) as f64;
            (room / (self.per_feed as f64 * MEMORY_MARGIN)) as u32
        })
    }
}

/// [`MemoryBudget`] over the machine's available memory.
struct MemoryWatch {
    system: System,
    total: u64,
    budget: MemoryBudget,
}

impl MemoryWatch {
    fn new() -> Self {
        let mut system = System::new();
        system.refresh_memory();
        let total = system.total_memory();
        let mut watch = Self {
            system,
            total,
            budget: MemoryBudget::new(total, 0),
        };
        watch.budget = MemoryBudget::new(total, watch.available());
        watch
    }

    /// Memory new feeds could have. On macOS, the kernel's own measure (the
    /// share it counts as free, as `memory_pressure` shows it): sysinfo's
    /// subtracts compressed memory, and reads nothing at all on a Mac that's
    /// merely compressing. Elsewhere, sysinfo's (`MemAvailable` on Linux).
    fn available(&mut self) -> u64 {
        #[cfg(target_os = "macos")]
        if let Some(pct) = macos_free_pct() {
            return self.total / 100 * pct;
        }
        self.system.refresh_memory();
        self.system.available_memory()
    }

    fn short(&mut self) -> bool {
        let available = self.available();
        self.budget.short(available)
    }

    fn observe(&mut self, feeds: u32) {
        let available = self.available();
        self.budget.observe(available, feeds);
    }
}

/// `kern.memorystatus_level`: the percentage of memory macOS counts as free.
#[cfg(target_os = "macos")]
fn macos_free_pct() -> Option<u64> {
    let mut level: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is a C string and `level` is an int `len` bytes long,
    // which is what this sysctl writes.
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_level".as_ptr(),
            (&mut level as *mut libc::c_int).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    (ok && (0..=100).contains(&level)).then_some(level as u64)
}

// ── Measuring ─────────────────────────────────────────────────────────────────

/// Counters across every feed at one moment.
struct Snapshot {
    at: Instant,
    /// Per feed.
    frames: Vec<VideoProgress>,
    /// Per channel feed, how its clip has reached its program.
    played: Vec<VideoProgress>,
    /// Clip frames that reached a program too late to show, across feeds.
    late: u64,
    dropped: u64,
    /// Per output: bytes written so far, across every feed.
    bytes: Vec<u64>,
}

impl Snapshot {
    /// `replay` when the feeds keep replay buffers: their one "output" is
    /// counted by what the buffers have written, since old segments go.
    async fn take(feeds: &[Feed], replay: bool) -> anyhow::Result<Self> {
        let frames = feeds.iter().map(|f| f.monitor.video_progress()).collect();
        let played: Vec<PlayedProgress> = feeds.iter().filter_map(Feed::played).collect();
        let late = played.iter().map(|p| p.late).sum();
        let played = played.into_iter().map(|p| p.progress).collect();
        let dropped = feeds.iter().map(Feed::dropped_frames).sum();
        if replay {
            let written = feeds
                .iter()
                .map(|f| match &f.work {
                    Work::Replay(_, index) => index.lock().unwrap().written(),
                    Work::Record(_) | Work::Channel(..) => 0,
                })
                .sum();
            return Ok(Self {
                at: Instant::now(),
                frames,
                played,
                late,
                dropped,
                bytes: vec![written],
            });
        }
        // Per output, every feed's files.
        let mut files: Vec<Vec<String>> = Vec::new();
        for feed in feeds {
            if let Work::Record(legs) = &feed.work {
                files.resize(legs.len(), Vec::new());
                for (list, leg) in files.iter_mut().zip(legs) {
                    list.extend(leg.files());
                }
            }
        }
        let at = Instant::now();
        let bytes = tokio::task::spawn_blocking(move || {
            files
                .iter()
                .map(|list| {
                    list.iter()
                        .filter_map(|f| std::fs::metadata(f).ok())
                        .map(|m| m.len())
                        .sum()
                })
                .collect()
        })
        .await?;
        Ok(Self {
            at,
            frames,
            played,
            late,
            dropped,
            bytes,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn measure(
    feeds: u32,
    fps: f64,
    outputs: usize,
    before: &Snapshot,
    after: &Snapshot,
    cpu: &[f64],
    memory_pct: f64,
    threshold_pct: f64,
) -> BenchmarkStepDto {
    let secs = (after.at - before.at).as_secs_f64();
    let (produced, program_expected, program_shortfall) =
        shortfall(&before.frames, &after.frames, fps, secs);
    // A channel's clip: frames that didn't arrive, or came too late to show.
    let (_, clip_expected, clip_shortfall) = shortfall(&before.played, &after.played, fps, secs);
    let clip_shortfall = clip_shortfall + after.late.saturating_sub(before.late);
    let expected = program_expected + clip_expected;
    let source_shortfall = program_shortfall + clip_shortfall;
    let output_dropped = after.dropped.saturating_sub(before.dropped);
    let pct = |n: u64, of: u64| n as f64 / of.max(1) as f64 * 100.0;
    let source_pct =
        pct(program_shortfall, program_expected).max(pct(clip_shortfall, clip_expected));
    let output_pct = output_dropped as f64 / (produced * outputs as u64).max(1) as f64 * 100.0;
    let drop_pct = source_pct.max(output_pct);
    let per_sec = |bytes: u64| (bytes as f64 / secs.max(0.001)) as u64;
    let written: Vec<u64> = after
        .bytes
        .iter()
        .zip(&before.bytes)
        .map(|(a, b)| a.saturating_sub(*b))
        .collect();
    BenchmarkStepDto {
        feeds,
        secs,
        expected_frames: expected,
        source_shortfall,
        output_dropped,
        drop_pct,
        cpu_pct: if cpu.is_empty() {
            0.0
        } else {
            cpu.iter().sum::<f64>() / cpu.len() as f64
        },
        memory_pct,
        write_bytes_per_sec: per_sec(written.iter().sum()),
        output_bytes_per_sec: written.iter().map(|&b| per_sec(b) / feeds as u64).collect(),
        passed: drop_pct <= threshold_pct,
        quick: false,
    }
}

/// Per feed: the frames delivered against the time their timestamps span,
/// less the gaps where the footage looped (see `VideoProgress`). A frame of
/// slack covers one landing between the count and the timestamp reads.
/// Returns the frames delivered, those due, and those missing.
fn shortfall(
    before: &[VideoProgress],
    after: &[VideoProgress],
    fps: f64,
    secs: f64,
) -> (u64, u64, u64) {
    let (mut produced, mut expected, mut missing) = (0, 0, 0);
    for (p0, p1) in before.iter().zip(after) {
        let delivered = p1.frames.saturating_sub(p0.frames);
        let span = match (p0.last_pts, p1.last_pts) {
            (Some(t0), Some(t1)) => {
                let skipped = p1.skipped.saturating_sub(p0.skipped);
                t1.saturating_sub(t0).saturating_sub(skipped).nseconds() as f64 / 1e9
            }
            _ => secs,
        };
        let should = (span * fps).round() as u64;
        produced += delivered;
        expected += should;
        missing += should.saturating_sub(delivered + 1);
    }
    (produced, expected, missing)
}

// ── Scratch folders ───────────────────────────────────────────────────────────

/// Per output, a scratch folder on the volume it would write to: in the
/// nearest existing folder of where its files would go. Created here.
fn scratch_dirs(
    outputs: &[PresetOutputInput],
    id: &str,
    node: &str,
    media: &MediaInfo,
) -> anyhow::Result<Vec<PathBuf>> {
    let legs = plan_legs(outputs, Some(&path_vars(media, node, 1))).map_err(|e| anyhow!(e))?;
    legs.iter()
        .map(|(path, _)| scratch_dir(path.parent().context("output path has no folder")?, id))
        .collect()
}

/// A scratch folder on the volume `folder` is on: in its nearest existing
/// folder. Created here.
fn scratch_dir(folder: &std::path::Path, id: &str) -> anyhow::Result<PathBuf> {
    let existing = folder
        .ancestors()
        .find(|p| p.is_dir())
        .context("no existing folder to write in")?;
    let dir = existing.join(format!("{SCRATCH_PREFIX}{}", &id[..8]));
    if !dir.exists() {
        std::fs::create_dir(&dir).with_context(|| format!("create {}", dir.display()))?;
    }
    Ok(dir)
}

/// Delete scratch folders, refusing anything not named like one.
fn remove_scratch(dirs: &[PathBuf]) {
    for dir in dirs {
        let named = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(SCRATCH_PREFIX));
        if !named {
            warn!(dir = ?dir, "not a benchmark scratch folder; leaving it");
            continue;
        }
        match std::fs::remove_dir_all(dir) {
            Ok(()) => info!(dir = ?dir, "removed benchmark files"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(dir = ?dir, error = %e, "could not remove benchmark files"),
        }
    }
}

fn dedup(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !out.contains(d) {
            out.push(d.clone());
        }
    }
    out
}

/// Channel feed `n`'s own copy of a measured output: NDI under a numbered
/// name (each feed's shows on the network while the run lasts), SRT
/// listening on a free local port, so N of them can run at once.
fn feed_output(config: &OutputConfig, n: u32) -> anyhow::Result<OutputConfig> {
    Ok(match config {
        OutputConfig::Ndi { .. } => OutputConfig::Ndi {
            ndi_name: Some(format!("Benchmark {n}")),
        },
        OutputConfig::Srt { bitrate_kbps, .. } => {
            let port = std::net::UdpSocket::bind(("127.0.0.1", 0))
                .and_then(|s| s.local_addr())
                .context("find a free port for SRT")?
                .port();
            OutputConfig::Srt {
                url: format!("srt://127.0.0.1:{port}?mode=listener"),
                latency_ms: 120,
                bitrate_kbps: *bitrate_kbps,
                passphrase: None,
            }
        }
        other => anyhow::bail!("a channel benchmark doesn't measure {other:?}"),
    })
}

/// Path template values for benchmark feed `n`, with the footage's format.
fn path_vars(media: &MediaInfo, node: &str, n: u32) -> PathVars {
    let channels = if media.audio_channels > 0 {
        media.audio_channels
    } else {
        2
    };
    PathVars {
        source: format!("benchmark-{n}"),
        source_name: format!("Benchmark feed {n}"),
        node: node.to_string(),
        preset: "benchmark".into(),
        at: chrono::Local::now(),
        take: 1,
        suffix: 1,
        source_resolution: Some((media.width, media.height)),
        source_framerate: Some((media.fps_num, media.fps_den)),
        source_audio: Some((channels, channels <= 2)),
    }
}

fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gstreamer as gst;

    fn secs(s: f64) -> gst::ClockTime {
        gst::ClockTime::from_nseconds((s * 1e9) as u64)
    }

    fn ts(frames: u64, at: f64) -> VideoProgress {
        VideoProgress {
            frames,
            last_pts: Some(secs(at)),
            skipped: gst::ClockTime::ZERO,
        }
    }

    fn snapshot(
        at: Instant,
        frames: Vec<VideoProgress>,
        dropped: u64,
        bytes: Vec<u64>,
    ) -> Snapshot {
        Snapshot {
            at,
            frames,
            played: Vec::new(),
            late: 0,
            dropped,
            bytes,
        }
    }

    #[test]
    fn measures_a_step() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(500, 100.0), ts(500, 100.0)], 0, vec![0, 0]);
        // 2 feeds at 30 fps for 10 s: 600 frames expected, all delivered.
        let after = snapshot(
            t + Duration::from_secs(10),
            vec![ts(800, 110.0), ts(800, 110.0)],
            3,
            vec![20_000_000, 2_000_000],
        );
        let step = measure(2, 30.0, 2, &before, &after, &[50.0, 70.0], 40.0, 0.5);
        assert_eq!(step.expected_frames, 600);
        assert_eq!(step.source_shortfall, 0);
        assert_eq!(step.output_dropped, 3);
        // 3 of 1200 encoded frames.
        assert!((step.drop_pct - 0.25).abs() < 1e-9);
        assert!(step.passed);
        assert_eq!(step.cpu_pct, 60.0);
        assert_eq!(step.write_bytes_per_sec, 2_200_000);
        assert_eq!(step.output_bytes_per_sec, [1_000_000, 100_000]);
    }

    #[test]
    fn timestamps_not_the_clock_decide_the_shortfall() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(0, 0.0)], 0, vec![0]);
        // The window ran 10 s by the clock, but the last frame counted is
        // from 9.9 s: 297 frames is everything that was due.
        let after = snapshot(t + Duration::from_secs(10), vec![ts(297, 9.9)], 0, vec![0]);
        let step = measure(1, 30.0, 1, &before, &after, &[], 0.0, 0.5);
        assert_eq!(step.source_shortfall, 0);
        assert!(step.passed);
    }

    #[test]
    fn loop_gaps_are_not_a_shortfall() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(0, 0.0)], 0, vec![0]);
        // 10 s, but the footage's audio runs 0.9 s past its video, so each
        // loop leaves a gap: 273 frames is everything that was due.
        let after = VideoProgress {
            frames: 273,
            last_pts: Some(secs(10.0)),
            skipped: secs(0.9),
        };
        let after = snapshot(t + Duration::from_secs(10), vec![after], 0, vec![0]);
        let step = measure(1, 30.0, 1, &before, &after, &[], 0.0, 0.5);
        assert_eq!(step.expected_frames, 273);
        assert_eq!(step.source_shortfall, 0);
    }

    #[test]
    fn source_shortfall_fails_a_step() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(0, 0.0); 4], 0, vec![0]);
        // Each of 4 feeds should deliver 300 frames; one delivered 250.
        let after = snapshot(
            t + Duration::from_secs(10),
            vec![ts(300, 10.0), ts(300, 10.0), ts(300, 10.0), ts(250, 10.0)],
            0,
            vec![0],
        );
        let step = measure(4, 30.0, 1, &before, &after, &[], 0.0, 0.5);
        assert_eq!(step.source_shortfall, 49);
        assert!(!step.passed);
    }

    #[test]
    fn late_clip_frames_fail_a_channel_step() {
        let t = Instant::now();
        let mut before = snapshot(t, vec![ts(0, 0.0)], 0, vec![]);
        before.played = vec![ts(0, 0.1)];
        // The program kept up, and the clip delivered every frame, but 30 of
        // its 300 came too late to show.
        let mut after = snapshot(t + Duration::from_secs(10), vec![ts(300, 10.0)], 0, vec![]);
        after.played = vec![ts(300, 10.1)];
        after.late = 30;
        let step = measure(1, 30.0, 0, &before, &after, &[], 0.0, 0.5);
        assert_eq!(step.expected_frames, 600);
        assert_eq!(step.source_shortfall, 30);
        // Against the clip's frames, not diluted by the program's.
        assert!((step.drop_pct - 10.0).abs() < 1e-9);
        assert!(!step.passed);
        assert!(step.output_bytes_per_sec.is_empty());
    }

    /// Run a search against a node that sustains `capacity` feeds, where
    /// quick checks also pass up to `quick_ok` (a quick check can be fooled
    /// near the limit). Returns the answer and the counts measured, in order.
    fn simulate(limit: u32, capacity: u32, quick_ok: u32) -> (u32, Vec<(u32, bool)>) {
        let mut search = Search::new(limit);
        let mut measured = Vec::new();
        loop {
            match search.next() {
                Next::Done(n) => return (n, measured),
                Next::Measure { feeds, full } => {
                    measured.push((feeds, full));
                    let ok = feeds
                        <= if full {
                            capacity
                        } else {
                            quick_ok.max(capacity)
                        };
                    if ok {
                        search.passed(feeds, full);
                    } else if !full && feeds <= capacity {
                        // Not clear from the quick check: measured in full.
                        search.passed(feeds, true);
                    } else {
                        search.failed(feeds);
                    }
                }
            }
            assert!(measured.len() < 50, "search didn't end");
        }
    }

    #[test]
    fn search_finds_the_capacity() {
        for capacity in 0..=70 {
            let (found, measured) = simulate(64, capacity, 0);
            assert_eq!(found, capacity.min(64), "capacity {capacity}: {measured:?}");
            assert!(
                measured.len() <= 15,
                "capacity {capacity}: {} steps",
                measured.len()
            );
        }
    }

    #[test]
    fn search_doubles_then_narrows() {
        let (found, measured) = simulate(64, 40, 0);
        assert_eq!(found, 40);
        let quick: Vec<u32> = measured
            .iter()
            .filter(|(_, full)| !full)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(quick, [1, 2, 4, 8, 16, 32, 64]);
        let full: Vec<u32> = measured
            .iter()
            .filter(|(_, full)| *full)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(full, [48, 40, 44, 42, 41]);
    }

    #[test]
    fn answer_is_confirmed_by_a_full_step() {
        // Quick checks pass up to 40, but only 30 keep up in full.
        let (found, measured) = simulate(64, 30, 40);
        assert_eq!(found, 30);
        assert!(measured.contains(&(30, true)), "{measured:?}");

        // Every quick check passes up to the limit: the limit is confirmed in full.
        let (found, measured) = simulate(16, 100, 0);
        assert_eq!(found, 16);
        assert_eq!(measured.last(), Some(&(16, true)));
    }

    #[test]
    fn capping_stops_the_search() {
        let mut search = Search::new(64);
        search.passed(1, false);
        search.passed(2, false);
        search.passed(4, true);
        search.cap(4);
        assert_eq!(search.next(), Next::Done(4));
    }

    #[test]
    fn memory_budget() {
        const GB: u64 = 1 << 30;
        const MB: u64 = 1 << 20;
        // 8 GB with 3 GB available: never below 1.5 GB, so 1.5 GB to use.
        let mut budget = MemoryBudget::new(8 * GB, 3 * GB);
        assert_eq!(budget.floor, 1536 * MB);
        assert_eq!(budget.fits(), None);
        assert!(!budget.short(1536 * MB));
        assert!(budget.short(1535 * MB));
        // 4 feeds took 400 MB: 100 MB each, 125 MB with the margin.
        budget.observe(3 * GB - 400 * MB, 4);
        assert_eq!(budget.fits(), Some(12));
        // A step where they took less doesn't lower the estimate.
        budget.observe(3 * GB - 400 * MB, 8);
        assert_eq!(budget.fits(), Some(12));
        // On 32 GB, an eighth is kept free.
        assert_eq!(MemoryBudget::new(32 * GB, 20 * GB).floor, 4 * GB);
    }

    #[test]
    fn only_scratch_folders_are_removed() {
        let root = std::env::temp_dir().join(format!("cr-bench-test-{}", uuid::Uuid::new_v4()));
        let scratch = root.join(format!("{SCRATCH_PREFIX}abc"));
        let other = root.join("recordings");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(scratch.join("f.mov"), b"x").unwrap();
        remove_scratch(&[scratch.clone(), other.clone()]);
        assert!(!scratch.exists());
        assert!(other.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
