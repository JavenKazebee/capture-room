//! A benchmark run: its feeds, the steps it measures and how it ends.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use gstreamer as gst;
use sysinfo::System;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::api::types::{
    BenchmarkRunDto, BenchmarkStatus, BenchmarkStepDto, ChannelConfig, ClipEnd, FileSourceConfig,
    LiveVideoFormat, MediaInfo, OutputConfig, PresetOutputInput, WsEvent,
};
use crate::capacity::Setup;
use crate::db;
use crate::pipeline::monitor::MonitorPipeline;
use crate::pipeline::output::{self, OutputContext, OutputLeg, Program};
use crate::pipeline::profile::{plan_legs, PathVars, RecordingProfile};
use crate::pipeline::recording::{self, OnLegError, OnLegFile, RecordingLeg};
use crate::pipeline::replay::{Index, ReplayIndex, ReplayLeg};
use crate::sources::channel::{ChannelSource, ClipRequest, PlayedProgress, Playout};
use crate::sources::file::{self, FileSource};
use crate::sources::InputSource;
use crate::state::AppState;

use super::measure::{measure, Snapshot};
use super::memory::MemoryWatch;
use super::search::{Next, Search};

/// Settling time after changing the feed count, before measuring…
const WARMUP: Duration = Duration::from_secs(5);
/// …plus a second per this many feeds started, so a big start doesn't fail a passing step.
const WARMUP_FEEDS_PER_SEC: u32 = 4;
/// How long a quick check measures, while doubling the feeds.
const QUICK_SECS: u32 = 5;
/// A quick check passes only well clear of the limit: under half the drop
/// threshold and below this CPU. Anything closer is measured in full.
const QUICK_MAX_CPU_PCT: f64 = 85.0;
/// A quick check this many times over the drop threshold fails outright.
const QUICK_FAIL_FACTOR: f64 = 4.0;
/// Times a full step is measured before it fails.
const MEASURE_ATTEMPTS: u32 = 2;
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);
/// A run stops (and keeps its result) before a scratch volume gets this full.
const MIN_FREE_BYTES: u64 = 2_000_000_000;
const MEMORY_POLL: Duration = Duration::from_millis(500);
/// Scratch folder prefix (plus the run id's start). Only folders named so are deleted.
const SCRATCH_PREFIX: &str = ".capture-room-benchmark-";
/// How much a replay buffer feed keeps, so old segments go as in a real
/// buffer and the scratch volume doesn't fill.
const REPLAY_KEEP: gst::ClockTime = gst::ClockTime::from_seconds(30);

/// One feed: a looping file (or a channel's program), its monitor, and what it feeds.
pub(super) struct Feed {
    pub(super) monitor: Arc<MonitorPipeline>,
    pub(super) work: Work,
}

/// What a feed's footage goes to.
pub(super) enum Work {
    Record(Vec<RecordingLeg>),
    /// A replay buffer. The leg is dropped before the index it writes into.
    Replay(ReplayLeg, ReplayIndex),
    /// A playout channel playing the footage, and its outputs. Its clip is
    /// stopped before it's dropped (see [`discard_feeds`]).
    Channel(Arc<Playout>, Vec<OutputLeg>),
}

impl Feed {
    pub(super) fn dropped_frames(&self) -> u64 {
        match &self.work {
            Work::Record(legs) => legs.iter().map(RecordingLeg::dropped_frames).sum(),
            Work::Replay(leg, _) => leg.dropped_frames(),
            Work::Channel(_, outputs) => outputs.iter().filter_map(OutputLeg::dropped_frames).sum(),
        }
    }

    /// The encoder elements it uses, one per output (a channel's by name).
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
    pub(super) fn played(&self) -> Option<PlayedProgress> {
        match &self.work {
            Work::Channel(playout, _) => Some(playout.played()),
            _ => None,
        }
    }
}

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

pub(super) struct Run {
    pub(super) state: Arc<AppState>,
    pub(super) dto: BenchmarkRunDto,
    pub(super) scratch: Vec<PathBuf>,
    pub(super) cancel: CancellationToken,
    pub(super) reason: Arc<Mutex<Option<String>>>,
    /// The first leg error, reported from a streaming thread.
    pub(super) failure: Arc<Mutex<Option<String>>>,
    pub(super) memory: Mutex<MemoryWatch>,
    /// Playout channels running on the node when the run started.
    pub(super) channels_running: usize,
}

impl Run {
    pub(super) async fn run(mut self, done: watch::Sender<bool>) {
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

    /// Search for the most feeds the node sustains. Returns the closing message.
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

    /// Run `n` feeds and measure them, noting how much memory they take.
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

    /// Measure `n` feeds: a quick check passing only well clear of the limit
    /// (failing outright far over it), else full steps, one more if the first
    /// drops frames. `Some(full)` if they kept up, `None` if not.
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

    async fn measure_window(
        &self,
        feeds: &[Feed],
        n: u32,
        secs: u32,
        system: &mut System,
    ) -> Result<BenchmarkStepDto, Stop> {
        let setup = Setup::of(&self.dto);
        let before = Snapshot::take(feeds, setup).await?;
        system.refresh_cpu_usage();
        let mut cpu = Vec::new();
        for _ in 0..secs {
            self.pause(feeds, Duration::from_secs(1)).await?;
            system.refresh_cpu_usage();
            cpu.push(system.global_cpu_usage() as f64);
        }
        let after = Snapshot::take(feeds, setup).await?;
        system.refresh_memory();
        let memory_pct = system.used_memory() as f64 / system.total_memory().max(1) as f64 * 100.0;
        Ok(measure(&self.dto, n, &before, &after, &cpu, memory_pct))
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
            feed_id(n),
            feed_name(n),
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

    /// Start feed `n` as a playout channel at the footage's format, with its
    /// own copies of `outputs`, playing the footage on loop.
    async fn add_channel(&self, n: u32, outputs: &[OutputConfig]) -> Result<Feed, Stop> {
        let m = &self.dto.media;
        let channel = ChannelSource::new(
            feed_id(n),
            format!("Benchmark channel {n}"),
            ChannelConfig {
                format: LiveVideoFormat {
                    width: m.width,
                    height: m.height,
                    fps_num: m.fps_num,
                    fps_den: m.fps_den,
                },
                audio_channels: file::audio_channels(m).min(output::SRT_MAX_AUDIO_CHANNELS),
                outputs: Vec::new(),
            },
        );
        let monitor = self.start_monitor(n, &channel).await?;
        let program = Program::from(&channel.capabilities().context("channel format")?);
        let legs = {
            let manager = self.state.source_manager.read().await;
            let context = OutputContext {
                displays: manager.displays(),
                devices: manager.devices(),
            };
            outputs
                .iter()
                .map(|o| {
                    let config = feed_output(o, n)?;
                    let sink = output::sink(&config, channel.display_name(), program, &context);
                    OutputLeg::start(&monitor, sink.as_ref())
                        .with_context(|| format!("start feed {n}'s {}", sink.label()))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        };

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
            start: None,
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

    /// Wait `duration`, returning early on a cancel, a failure, or short memory.
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
        tokio::task::spawn_blocking(move || crate::storage::short_volume(&dirs, MIN_FREE_BYTES))
            .await
            .ok()
            .flatten()
            .map(|v| v.mount_point)
    }

    async fn publish(&self) {
        if let Err(e) = db::benchmark_save(&self.state.db, &self.dto).await {
            error!(error = %e, "save benchmark");
        }
        self.state.emit(&WsEvent::BenchmarkUpdated {
            run: Box::new(self.dto.clone()),
        });
    }
}

/// Tear feeds down. Legs are discarded, not finished: their files are thrown away.
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

/// Per output, a scratch folder (created here) in the nearest existing folder
/// of where its files would go.
pub(super) fn scratch_dirs(
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

/// A scratch folder (created here) in `folder`'s nearest existing folder.
pub(super) fn scratch_dir(folder: &std::path::Path, id: &str) -> anyhow::Result<PathBuf> {
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
pub(super) fn remove_scratch(dirs: &[PathBuf]) {
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

pub(super) fn dedup(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !out.contains(d) {
            out.push(d.clone());
        }
    }
    out
}

/// Channel feed `n`'s own copy of a measured output (a numbered NDI name, an
/// SRT listener on a free port), so N can run at once.
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
    let channels = file::audio_channels(media);
    PathVars {
        source: feed_id(n),
        source_name: feed_name(n),
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

/// Benchmark feed `n`'s source id…
fn feed_id(n: u32) -> String {
    format!("benchmark-{n}")
}

/// …and the name it plays under (a channel feed's is its own).
fn feed_name(n: u32) -> String {
    format!("Benchmark feed {n}")
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
