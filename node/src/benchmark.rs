//! The benchmark runner: how many feeds this node can record with a set of
//! outputs.
//!
//! Each feed is a looping media file (real footage: test patterns compress
//! unrealistically) with its own monitor and recording legs, built the same
//! way as a real recording but kept out of the [`SourceManager`], so they
//! never show up as sources. Each step runs some number of feeds and, after a
//! warm-up, measures:
//!
//! - **Source shortfall:** frames the feeds didn't deliver, from the
//!   monitors' frame counts against the footage's frame rate. Decoding (or
//!   the monitors) fell behind.
//! - **Output drops:** frames the legs' input queues dropped because an
//!   encoder couldn't keep up.
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
use sysinfo::System;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    BenchmarkRequest, BenchmarkRunDto, BenchmarkStatus, BenchmarkStepDto, FileSourceConfig,
    MediaInfo, PresetOutputInput, WsEvent,
};
use crate::db;
use crate::pipeline::monitor::{MonitorPipeline, VideoProgress};
use crate::pipeline::profile::{plan_legs, PathVars, RecordingProfile};
use crate::pipeline::recording::{self, OnLegError, OnLegFile, RecordingLeg};
use crate::sources::file::{probe, FileSource};
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
/// A run stops (and keeps its result) before a scratch volume gets this full…
const MIN_FREE_BYTES: u64 = 2_000_000_000;
/// …or the machine runs out of memory.
const MAX_MEMORY_PCT: f64 = 95.0;
/// How long a recording start waits for a cancelled benchmark to tear down.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(15);
/// Scratch folders are named this plus the start of the run id. Only folders
/// named so are ever deleted.
const SCRATCH_PREFIX: &str = ".capture-room-benchmark-";

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

/// Check a request, set up the run and start it. Refused while recording (the
/// result would be skewed, and the benchmark would compete with real
/// recordings) or while another benchmark runs.
pub async fn start(state: &Arc<AppState>, req: BenchmarkRequest) -> ApiResult<BenchmarkRunDto> {
    if req.outputs.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one output is required".into(),
        ));
    }
    plan_legs(&req.outputs, None).map_err(|e| ApiError::BadRequest(e.into()))?;
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
    // Checked after taking the slot: a recording that starts from here on
    // cancels this run instead.
    if !state
        .source_manager
        .read()
        .await
        .active_sessions()
        .is_empty()
    {
        state.benchmark.lock().unwrap().take();
        return Err(ApiError::Conflict(
            "stop every recording on this node before benchmarking it",
        ));
    }

    let setup = async {
        let scratch = scratch_dirs(&req.outputs, &id, &state.node_name(), &media)?;
        let dto = BenchmarkRunDto {
            id: id.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            status: BenchmarkStatus::Running,
            preset_id: req.preset_id.clone(),
            preset_name: req.preset_name.clone(),
            outputs: req.outputs.clone(),
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
    };
    tokio::spawn(run.run(done_tx));
    Ok(dto)
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

/// One feed: a looping file with its monitor and recording legs.
struct Feed {
    monitor: MonitorPipeline,
    legs: Vec<RecordingLeg>,
}

/// Why a run ended early.
enum Stop {
    Cancelled,
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
                    return Ok(self.closing_message(n, &search, capped.as_deref()));
                }
                Next::Measure { feeds, full } => (feeds, full),
            };

            let started = self.set_feeds(feeds, n).await?;
            self.dto.feeds_running = n;
            self.publish().await;
            let warmup =
                WARMUP + Duration::from_secs(started.div_ceil(WARMUP_FEEDS_PER_SEC) as u64);
            self.pause(feeds, warmup).await?;

            match self.check(feeds, n, full, &mut system).await? {
                Some(full) => {
                    search.passed(n, full);
                    let memory_pct = self.dto.steps.last().map_or(0.0, |s| s.memory_pct);
                    let reason = if memory_pct > MAX_MEMORY_PCT {
                        Some("memory is nearly full".to_string())
                    } else {
                        self.low_volume()
                            .await
                            .map(|volume| format!("{volume} is nearly full"))
                    };
                    if let Some(reason) = reason {
                        search.cap(n);
                        capped = Some(reason);
                    }
                }
                None => search.failed(n),
            }
            self.dto.sustainable_feeds = search.best();
            self.publish().await;
        }
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
        let before = Snapshot::take(feeds).await?;
        system.refresh_cpu_usage();
        let mut cpu = Vec::new();
        for _ in 0..secs {
            self.pause(feeds, Duration::from_secs(1)).await?;
            system.refresh_cpu_usage();
            cpu.push(system.global_cpu_usage() as f64);
        }
        let after = Snapshot::take(feeds).await?;
        system.refresh_memory();
        let memory_pct = system.used_memory() as f64 / system.total_memory().max(1) as f64 * 100.0;
        let threshold = self.dto.drop_threshold_pct;
        Ok(measure(
            n,
            fps,
            self.dto.outputs.len(),
            &before,
            &after,
            &cpu,
            memory_pct,
            threshold,
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
            let feed = self.add_feed(feeds.len() as u32 + 1).await?;
            if self.dto.encoders.is_empty() {
                self.dto.encoders = feed.legs.iter().map(|l| l.encoder().to_string()).collect();
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
        let source = FileSource::new(
            format!("benchmark-{n}"),
            format!("Benchmark feed {n}"),
            FileSourceConfig {
                path: self.dto.media_path.clone(),
                media: Some(self.dto.media.clone()),
            },
        );
        let config = *self.state.source_manager.read().await.monitor_config();
        let monitor = MonitorPipeline::new(&source, &config, &self.state.clock.clock())
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
        Ok(Feed { monitor, legs })
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

    /// Wait `duration`, returning early on a cancel or a failure.
    async fn pause(&self, feeds: &[Feed], duration: Duration) -> Result<(), Stop> {
        tokio::select! {
            _ = self.cancel.cancelled() => return Err(Stop::Cancelled),
            _ = tokio::time::sleep(duration) => {}
        }
        if let Some(e) = self.failure.lock().unwrap().clone() {
            return Err(Stop::Failed(anyhow!("an output failed: {e}")));
        }
        if let Some((i, e)) = feeds
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.monitor.error().map(|e| (i, e)))
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
    let teardown = tokio::task::spawn_blocking(move || {
        for feed in feeds {
            for leg in feed.legs {
                leg.discard();
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

// ── Measuring ─────────────────────────────────────────────────────────────────

/// Counters across every feed at one moment.
struct Snapshot {
    at: Instant,
    /// Per feed.
    frames: Vec<VideoProgress>,
    dropped: u64,
    /// Per output: bytes written so far, across every feed.
    bytes: Vec<u64>,
}

impl Snapshot {
    async fn take(feeds: &[Feed]) -> anyhow::Result<Self> {
        let outputs = feeds.first().map_or(0, |f| f.legs.len());
        let files: Vec<Vec<String>> = (0..outputs)
            .map(|i| feeds.iter().flat_map(|f| f.legs[i].files()).collect())
            .collect();
        let frames = feeds.iter().map(|f| f.monitor.video_progress()).collect();
        let dropped = feeds
            .iter()
            .flat_map(|f| &f.legs)
            .map(RecordingLeg::dropped_frames)
            .sum();
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
    // Per feed: the frames delivered against the time their timestamps span,
    // less the gaps where the footage looped (see `VideoProgress`). A frame
    // of slack covers one landing between the count and the timestamp reads.
    let (mut produced, mut expected, mut source_shortfall) = (0, 0, 0);
    for (p0, p1) in before.frames.iter().zip(&after.frames) {
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
        source_shortfall += should.saturating_sub(delivered + 1);
    }
    let output_dropped = after.dropped.saturating_sub(before.dropped);
    let source_pct = source_shortfall as f64 / expected.max(1) as f64 * 100.0;
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
    let name = format!("{SCRATCH_PREFIX}{}", &id[..8]);
    let mut dirs = Vec::with_capacity(legs.len());
    for (path, _) in &legs {
        let parent = path.parent().context("output path has no folder")?;
        let existing = parent
            .ancestors()
            .find(|p| p.is_dir())
            .context("no existing folder for the output")?;
        let dir = existing.join(&name);
        if !dir.exists() {
            std::fs::create_dir(&dir).with_context(|| format!("create {}", dir.display()))?;
        }
        dirs.push(dir);
    }
    Ok(dirs)
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
