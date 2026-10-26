//! The [`SourceManager`]: every source on this node, its monitor, and what
//! runs on it (recording sessions, channel outputs, replay buffers).
//!
//! A [`scan`](SourceManager::scan) rebuilds the source list from the
//! configured sources, the NDI senders and the local devices. A source whose
//! id and fingerprint are unchanged keeps its monitor; anything else is torn
//! down and rebuilt.
//!
//! The manager lives behind an `RwLock` shared by every API call and the WS
//! emitter, so nothing here blocks or awaits. Starting or stopping a pipeline,
//! or deleting a replay folder, is planned under the lock and handed back to
//! the caller to run off it:
//!
//! - [`Work`] starts monitors, channel outputs and replay legs (and stops what
//!   they replace). The caller runs it, then hands the result to
//!   [`SourceManager::apply`] under a second, short lock, which rejects
//!   anything that went stale meanwhile (its monitor was replaced, its buffer
//!   removed).
//! - [`RecordingStart`] starts a session's legs the same way: the session is
//!   reserved first, so no other start takes its source or paths, and
//!   [`SourceManager::finish_start_recording`] adds it.
//! - [`StopJob`] and [`Teardown`] drain recordings and stop monitors.
//!
//! [`Work::run`] starts each source's pipelines on their own thread, so a
//! slow source doesn't hold up the others.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use chrono::Utc;
use gstreamer::{self as gst, prelude::*};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use uuid::Uuid;

use crate::api::types::{
    ChannelLevelDto, ConfiguredSourceDto, LinkState, MonitorSettingsDto, NdiConnect, NdiSenderDto,
    NdiSubscriptionsDto, NdiSubscriptionsInput, OutputConfig, OutputStatusDto, RecordingSessionDto,
    RecordingStatus, ReplayBufferDto, ReplayCodec, ReplayConfigInput, SessionClockDto,
    StartRecordingRequest, TransportState,
};
use crate::pipeline::monitor::{MonitorPipeline, SourceFormat};
use crate::pipeline::output::{self, OutputLeg, Program};
use crate::pipeline::profile::RecordingProfile;
use crate::pipeline::recording::{self, OnLegError, OnLegFile, RecordingLeg};
use crate::pipeline::replay::{Index, ReplayIndex, ReplayLeg};

use super::channel::ReplayClip;
use super::device::LocalDevices;
use super::ndi::{NdiMonitor, NdiSource, OwnSenders};
use super::{configured, InputSource};
use crate::display::Displays;

/// Resolves to a stopping session's final DTO once its legs have finished.
pub type StopResult = watch::Receiver<Option<RecordingSessionDto>>;

struct ActiveSession {
    legs: Vec<RecordingLeg>,
    dto: RecordingSessionDto,
    /// Legs that have failed while recording (each reports only once).
    failed_legs: usize,
    /// What the outputs encode, to count the session against the node's capacity.
    outputs_key: String,
    started: Instant,
}

impl ActiveSession {
    fn dropped_frames(&self) -> Vec<u64> {
        self.legs.iter().map(RecordingLeg::dropped_frames).collect()
    }

    fn files(&self) -> Vec<Vec<String>> {
        self.legs.iter().map(RecordingLeg::files).collect()
    }

    /// The session's clock, with its first frame once one has arrived.
    fn clock(&self) -> Option<SessionClockDto> {
        let first = self.legs.iter().filter_map(RecordingLeg::first_frame).min();
        self.dto.clock.clone().map(|c| SessionClockDto {
            first_frame_us: first.map(|t| t.useconds()),
            first_frame_utc: first
                .zip(c.utc_offset_us)
                .and_then(|(t, offset)| {
                    chrono::DateTime::from_timestamp_micros(t.useconds() as i64 + offset)
                })
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            ..c
        })
    }
}

/// A synchronized start may be at most this far ahead.
const MAX_START_LEAD: Duration = Duration::from_secs(10);

/// A session taken out of the active set whose legs still have to drain and
/// close their files. The caller runs it on a spawned task.
pub struct StopJob {
    pub legs: Vec<RecordingLeg>,
    pub dto: RecordingSessionDto,
    /// Publish the final DTO here, then call [`SourceManager::finish_stop`].
    pub tx: watch::Sender<Option<RecordingSessionDto>>,
}

/// A monitor removed from the manager. The caller stops its recordings, then
/// its pipeline, on a spawned task.
pub struct Teardown {
    pub source_id: String,
    pub pipeline: Arc<MonitorPipeline>,
    pub stops: Vec<StopJob>,
}

/// The monitor a job runs on.
enum Feed {
    Running(Arc<MonitorPipeline>),
    /// The one the same [`Work`] starts for the source.
    Starting,
}

/// A channel's outputs, started on its monitor.
struct OutputJob {
    feed: Feed,
    name: String,
    program: Program,
    /// Each output's identity and config, and its leg if it keeps running.
    legs: Vec<(OutputConfig, OutputConfig, Option<OutputLeg>)>,
}

struct ReplayJob {
    /// `None`: there's nothing to record from yet.
    feed: Option<Feed>,
    codec: ReplayCodec,
    index: ReplayIndex,
    /// Delete what a previous run left in its folder.
    clear: bool,
}

/// One source's part of a [`Work`], run on a thread of its own.
#[derive(Default)]
struct SourceJob {
    monitor: Option<Arc<dyn InputSource>>,
    outputs: Option<OutputJob>,
    replay: Option<ReplayJob>,
}

/// Pipelines to stop and start, planned under the manager's lock. Run it off
/// the lock with [`Work::run`], which blocks, and hand the result to
/// [`SourceManager::apply`].
#[must_use]
pub struct Work {
    /// Stopped before anything starts: an NDI name can only be sent once,
    /// and an SRT port bound once.
    drops: Vec<Box<dyn Send>>,
    jobs: HashMap<String, SourceJob>,
    config: MonitorSettingsDto,
    clock: gst::Clock,
    displays: Displays,
    devices: Arc<LocalDevices>,
}

impl Work {
    pub fn is_empty(&self) -> bool {
        self.drops.is_empty() && self.jobs.is_empty()
    }

    fn job(&mut self, source_id: &str) -> &mut SourceJob {
        self.jobs.entry(source_id.to_string()).or_default()
    }

    fn starts_monitor(&self, source_id: &str) -> bool {
        self.jobs
            .get(source_id)
            .is_some_and(|j| j.monitor.is_some())
    }

    /// Stop what it replaces, then start each source's pipelines, a thread
    /// per source so a slow one holds up only its own.
    pub fn run(self) -> Done {
        let Work {
            drops,
            jobs,
            config,
            clock,
            displays,
            devices,
        } = self;
        drop(drops);
        // A monitor watches its bus on a task.
        let runtime = tokio::runtime::Handle::try_current().ok();
        let context = output::OutputContext {
            displays: &displays,
            devices: &devices,
        };
        let started = std::thread::scope(|s| {
            let threads: Vec<_> = jobs
                .into_iter()
                .map(|(id, job)| {
                    let (runtime, config, clock, context) = (&runtime, &config, &clock, &context);
                    s.spawn(move || {
                        let _entered = runtime.as_ref().map(|r| r.enter());
                        job.run(id, config, clock, context)
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|t| t.join().expect("a source's start panicked"))
                .collect()
        });
        Done(started)
    }
}

impl SourceJob {
    fn run(
        self,
        id: String,
        config: &MonitorSettingsDto,
        clock: &gst::Clock,
        context: &output::OutputContext,
    ) -> Started {
        let monitor = self.monitor.map(|source| {
            let pipeline = MonitorPipeline::new(source.as_ref(), config, clock)
                .map(Arc::new)
                .map_err(|e| {
                    let error = format!("{e:#}");
                    warn!(source = %id, error = %error, "failed to start monitor");
                    error
                });
            if pipeline.is_ok() {
                info!(source = %id, "monitor started");
            }
            (source, pipeline)
        });
        let started = monitor
            .as_ref()
            .and_then(|(_, pipeline)| pipeline.as_ref().ok())
            .cloned();
        let resolve = |feed: Feed| match feed {
            Feed::Running(monitor) => Some(monitor),
            Feed::Starting => started.clone(),
        };

        let outputs = self.outputs.and_then(|job| {
            let monitor = resolve(job.feed)?;
            let legs = job
                .legs
                .into_iter()
                .map(|(identity, config, kept)| {
                    if let Some(leg) = kept {
                        return (identity, Ok(leg));
                    }
                    let sink = output::sink(&config, &job.name, job.program, context);
                    let leg = OutputLeg::start(&monitor, sink.as_ref()).map_err(|e| {
                        let error = format!("{e:#}");
                        warn!(channel = %id, error = %error, "output failed to start");
                        (sink.label(), error)
                    });
                    (identity, leg)
                })
                .collect();
            Some((monitor, legs))
        });

        let replay = self.replay.map(|job| {
            if job.clear {
                Index::clear(&job.index);
            }
            let leg = job.feed.and_then(resolve).map(|monitor| {
                let leg = ReplayLeg::start(&monitor, job.codec, &job.index, &id).map_err(|e| {
                    let error = format!("{e:#}");
                    warn!(source = %id, error = %error, "replay buffer failed to start");
                    error
                });
                (leg, monitor)
            });
            (job.index, job.codec, leg)
        });

        Started {
            id,
            monitor,
            outputs,
            replay,
        }
    }
}

type StartedMonitor = (Arc<dyn InputSource>, Result<Arc<MonitorPipeline>, String>);
type StartedOutputs = (Arc<MonitorPipeline>, Vec<(OutputConfig, Leg)>);
type StartedReplayLeg = (Result<ReplayLeg, String>, Arc<MonitorPipeline>);

/// What a [`SourceJob`] started.
struct Started {
    id: String,
    monitor: Option<StartedMonitor>,
    outputs: Option<StartedOutputs>,
    replay: Option<(ReplayIndex, ReplayCodec, Option<StartedReplayLeg>)>,
}

/// What a [`Work`] started, for [`SourceManager::apply`].
#[must_use]
pub struct Done(Vec<Started>);

/// A recording reserved by [`SourceManager::begin_start_recording`]: no
/// other start takes its source or paths until it's finished.
struct Reservation {
    source_id: String,
    paths: Vec<PathBuf>,
}

/// A recording's legs to start off the lock with [`RecordingStart::run`].
#[must_use]
pub struct RecordingStart {
    monitor: Arc<MonitorPipeline>,
    legs: Vec<(PathBuf, RecordingProfile)>,
    on_error: OnLegError,
    on_file: OnLegFile,
    start_at: Option<gst::ClockTime>,
    utc_offset_us: Option<i64>,
    dto: RecordingSessionDto,
    outputs_key: String,
}

impl RecordingStart {
    /// Start the legs. Blocks.
    pub fn run(self) -> StartedRecording {
        let legs = recording::start_legs(
            &self.monitor,
            &self.dto.id[..8],
            &self.legs,
            &self.on_error,
            &self.on_file,
            self.start_at,
            self.utc_offset_us,
        );
        StartedRecording { start: self, legs }
    }
}

/// For [`SourceManager::finish_start_recording`].
#[must_use]
pub struct StartedRecording {
    start: RecordingStart,
    legs: Result<Vec<RecordingLeg>>,
}

/// What runs on a node, each as its capacity key and source format.
pub struct RunningLoad {
    pub feeds: Vec<(String, SourceFormat)>,
    /// Replay buffers that record.
    pub buffers: Vec<(String, SourceFormat)>,
    /// Playout channels whose program runs.
    pub channels: Vec<(String, SourceFormat)>,
}

impl RunningLoad {
    pub fn iter(&self) -> impl Iterator<Item = (&str, SourceFormat)> + '_ {
        self.feeds
            .iter()
            .chain(&self.buffers)
            .chain(&self.channels)
            .map(|(key, format)| (key.as_str(), *format))
    }
}

/// A leg that failed while its session ran; applied with [`SourceManager::note_leg_failure`].
pub struct LegFailure {
    pub session_id: String,
    pub path: PathBuf,
    pub error: String,
}

pub struct NotedFailure {
    pub source_id: String,
    pub error: String,
    /// Every leg has now failed, so nothing is being recorded.
    pub all_failed: bool,
}

pub enum StopOutcome {
    /// This call gets to do the work.
    Start(Box<StopJob>),
    /// A stop for this session is already in flight; wait on its result.
    Join(StopResult),
    /// Not active and not stopping. The caller falls back to the DB for an
    /// orphaned row (e.g. after a crash).
    NotFound,
}

/// A channel's output, sending, or its label and why it couldn't start.
type Leg = Result<OutputLeg, (String, String)>;

struct ChannelOutputs {
    /// Each output's [`output::identity`], with its leg.
    legs: Vec<(OutputConfig, Leg)>,
}

/// A source's replay buffer. Outlives its source coming and going (an NDI
/// sender restarting), so what it recorded stays playable.
struct Replay {
    /// Dropped before the index: it writes into it.
    leg: Option<(ReplayLeg, Arc<MonitorPipeline>)>,
    config: ReplayConfigInput,
    index: ReplayIndex,
    /// Why it couldn't start recording.
    error: Option<String>,
    /// A [`Work`] is starting its leg; no other may until it's applied.
    starting: bool,
}

fn keep(config: &ReplayConfigInput) -> gst::ClockTime {
    gst::ClockTime::from_seconds(u64::from(config.minutes) * 60)
}

/// Owns the sources, their monitor pipelines and active recording sessions.
/// Nothing here blocks: see the module doc for how pipeline work is handed
/// back to the caller.
pub struct SourceManager {
    config: MonitorSettingsDto,
    /// The node clock new monitors run on.
    clock: gst::Clock,
    /// Its domain (see `ClockStatusDto::domain`), for synchronized starts.
    clock_domain: String,
    sources: Vec<Arc<dyn InputSource>>,
    monitors: HashMap<String, Arc<MonitorPipeline>>,
    /// Why a source's monitor couldn't be started, until a later start works
    /// or the source goes away. A running monitor reports its own errors.
    start_errors: HashMap<String, String>,
    sessions: HashMap<String, ActiveSession>, // session_id → session
    /// Recordings whose legs are starting, by session id.
    reserved: HashMap<String, Reservation>,
    /// Playout channels' outputs, by source id: their config, and each
    /// output's leg (or why it couldn't start).
    outputs: HashMap<String, ChannelOutputs>,
    /// Sessions whose legs are draining, so a retried stop joins the in-flight
    /// result instead of reporting "already stopped".
    stopping: HashMap<String, StopResult>,
    ndi_monitor: NdiMonitor,
    ndi: NdiSubscriptionsInput,
    devices: Arc<LocalDevices>,
    displays: Displays,
    leg_failures: mpsc::UnboundedSender<LegFailure>,
    /// Session ids whose legs opened a new file (a split), so the file list is saved.
    leg_files: mpsc::UnboundedSender<String>,
    /// Replay buffers, by source id, and the folder they're kept in.
    replays: HashMap<String, Replay>,
    replay_root: PathBuf,
    /// Where saved replay clips keep their files, a folder each.
    clip_root: PathBuf,
}

impl SourceManager {
    pub fn new(
        config: MonitorSettingsDto,
        (clock, clock_domain): (gst::Clock, String),
        ndi_monitor: NdiMonitor,
        devices: Arc<LocalDevices>,
        displays: Displays,
        leg_failures: mpsc::UnboundedSender<LegFailure>,
        leg_files: mpsc::UnboundedSender<String>,
    ) -> Self {
        Self {
            config,
            clock,
            clock_domain,
            sources: Vec::new(),
            monitors: HashMap::new(),
            start_errors: HashMap::new(),
            sessions: HashMap::new(),
            reserved: HashMap::new(),
            outputs: HashMap::new(),
            stopping: HashMap::new(),
            ndi_monitor,
            ndi: NdiSubscriptionsInput::default(),
            devices,
            displays,
            leg_failures,
            leg_files,
            replays: HashMap::new(),
            replay_root: std::env::temp_dir()
                .join("capture-room-replay")
                .join(std::process::id().to_string()),
            clip_root: std::env::temp_dir()
                .join("capture-room-replay-clips")
                .join(std::process::id().to_string()),
        }
    }

    /// Set before any buffer is. Buffers are cleared on start; clips stay.
    pub fn set_replay_root(&mut self, buffers: PathBuf, clips: PathBuf) {
        self.replay_root = buffers;
        self.clip_root = clips;
    }

    pub fn set_node_name(&mut self, name: &str) {
        self.ndi_monitor.set_node_name(name);
    }

    pub fn replay_root(&self) -> &Path {
        &self.replay_root
    }

    pub fn replay_clip_dir(&self, clip_id: &str) -> PathBuf {
        self.clip_root.join(clip_id)
    }

    pub fn devices(&self) -> &Arc<LocalDevices> {
        &self.devices
    }

    /// Whether the NDI library loaded (see [`NdiMonitor`]).
    pub fn ndi_runtime_loaded(&self) -> bool {
        self.ndi_monitor.runtime_loaded()
    }

    pub fn displays(&self) -> &Displays {
        &self.displays
    }

    pub fn sources(&self) -> &[Arc<dyn InputSource>] {
        &self.sources
    }

    pub fn get_source(&self, id: &str) -> Option<&dyn InputSource> {
        self.sources
            .iter()
            .find(|s| s.id() == id)
            .map(|s| s.as_ref())
    }

    pub fn is_monitored(&self, source_id: &str) -> bool {
        self.monitors.contains_key(source_id)
    }

    /// A live source's link state, while its monitor runs.
    pub fn link(&self, source: &dyn InputSource) -> Option<LinkState> {
        self.is_monitored(source.id())
            .then(|| source.link())
            .flatten()
    }

    /// Why a source's monitor stopped producing, or couldn't start.
    pub fn monitor_error(&self, source_id: &str) -> Option<String> {
        match self.monitors.get(source_id) {
            Some(monitor) => monitor.error(),
            None => self.start_errors.get(source_id).cloned(),
        }
    }

    /// Whether a source is being monitored and its monitor hasn't failed.
    fn is_healthy(&self, source_id: &str) -> bool {
        self.monitors
            .get(source_id)
            .is_some_and(|m| m.error().is_none())
    }

    /// Whether any source lacks a healthy monitor (failed or never started) or
    /// is idle on an old clock, and needs a rescan.
    pub fn needs_rescan(&self) -> bool {
        self.sources
            .iter()
            .any(|s| !self.is_healthy(s.id()) || self.should_migrate(s.as_ref()))
            || self
                .replays
                .iter()
                .any(|(id, r)| self.monitors.contains_key(id) && !r.starting && !Self::recording(r))
    }

    /// Monitors started from now on use `clock`; running ones move to it as
    /// they become idle (see [`Self::scan`]).
    pub fn set_clock(&mut self, (clock, domain): (gst::Clock, String)) {
        self.clock = clock;
        self.clock_domain = domain;
    }

    /// Sources whose monitors still run on an old clock.
    pub fn stale_sources(&self) -> u32 {
        self.monitors
            .values()
            .filter(|m| m.clock() != &self.clock)
            .count() as u32
    }

    /// Whether `source`'s monitor is on an old clock and can be rebuilt now:
    /// nothing records it and, for a channel, nothing plays.
    fn should_migrate(&self, source: &dyn InputSource) -> bool {
        let id = source.id();
        let stale = self
            .monitors
            .get(id)
            .is_some_and(|m| m.clock() != &self.clock);
        let recording = self.is_recording(id);
        let playing = source
            .playout()
            .is_some_and(|p| p.status().state != TransportState::Idle);
        stale && !recording && !playing
    }

    /// Rebuild the source list from configs and visible NDI senders. A source
    /// with unchanged id and fingerprint keeps its monitor and recordings unless
    /// the monitor failed, never started, or is idle on an old clock. Torn-down
    /// sources are returned for the caller to run, with the [`Work`] that
    /// starts what's new. Candidates are only descriptions, so a no-op scan
    /// builds no GStreamer elements.
    pub fn scan(&mut self, configs: &[ConfiguredSourceDto]) -> (Vec<Teardown>, Work) {
        let mut candidates: Vec<Box<dyn InputSource>> = configs
            .iter()
            .map(|c| configured(c, &self.devices))
            .collect();
        let own = self.own_senders();
        candidates.extend(
            self.ndi_monitor
                .current_sources()
                .into_iter()
                .filter(|s| self.wants_ndi(s, &own))
                .map(|s| Box::new(s) as Box<dyn InputSource>),
        );

        // Outputs aren't in a channel's fingerprint, so a kept source has its
        // old config: take the wanted outputs from the candidates.
        let wanted_outputs: HashMap<String, (String, Vec<OutputConfig>, Program)> = candidates
            .iter()
            .filter(|c| c.playout().is_some())
            .filter_map(|c| {
                let name = c.display_name().to_string();
                let program = Program::from(&c.capabilities()?);
                Some((c.id().to_string(), (name, c.outputs().to_vec(), program)))
            })
            .collect();

        let mut old: HashMap<String, Arc<dyn InputSource>> = self
            .sources
            .drain(..)
            .map(|s| (s.id().to_string(), s))
            .collect();
        let mut gone = Vec::new();
        let mut added = Vec::new();
        for mut candidate in candidates {
            let id = candidate.id().to_string();
            if self.sources.iter().any(|s| s.id() == id) {
                warn!(id = %id, "duplicate source id, ignoring");
                continue;
            }
            match old.remove(&id) {
                Some(mut existing)
                    if existing.fingerprint() == candidate.fingerprint()
                        && self.is_healthy(&id)
                        && !self.should_migrate(existing.as_ref()) =>
                {
                    // Only a Work starting its monitor shares a source.
                    match Arc::get_mut(&mut existing) {
                        Some(source) => source.set_name(candidate.display_name()),
                        None => warn!(id = %id, "source is starting; keeping its old name"),
                    }
                    self.sources.push(existing);
                    continue;
                }
                Some(existing) => {
                    candidate.adopt(existing.as_ref());
                    gone.push(id.clone());
                }
                None => {}
            }
            added.push(id);
            self.sources.push(Arc::from(candidate));
        }
        gone.extend(old.into_keys());

        // Tear down first: a changed source restarts under the same id.
        let mut work = self.work();
        let teardowns = gone
            .iter()
            .filter_map(|id| self.disconnect(id, &mut work))
            .collect();
        for id in &gone {
            self.start_errors.remove(id);
        }
        for id in &added {
            if let Some(outputs) = self.outputs.remove(id) {
                work.drops.push(Box::new(outputs));
            }
            if let Some(source) = self.sources.iter().find(|s| s.id() == id) {
                work.job(id).monitor = Some(Arc::clone(source));
            }
        }

        let unwanted: Vec<String> = self
            .outputs
            .keys()
            .filter(|id| !wanted_outputs.contains_key(*id))
            .cloned()
            .collect();
        for id in unwanted {
            work.drops.extend(
                self.outputs
                    .remove(&id)
                    .map(|o| Box::new(o) as Box<dyn Send>),
            );
        }
        for (id, (name, configs, program)) in wanted_outputs {
            self.reconcile_outputs(&id, &name, configs, program, &mut work);
        }

        self.sync_replays(&mut work);
        info!(count = self.sources.len(), "source scan complete");
        (teardowns, work)
    }

    fn work(&self) -> Work {
        Work {
            drops: Vec::new(),
            jobs: HashMap::new(),
            config: self.config,
            clock: self.clock.clone(),
            displays: self.displays.clone(),
            devices: Arc::clone(&self.devices),
        }
    }

    /// Put in place what `done` started, unless it went stale while the lock
    /// was free: its source or monitor was replaced, or its replay buffer
    /// removed or given another codec. Returns what was turned away, to drop
    /// off the lock.
    pub fn apply(&mut self, done: Done) -> Vec<Box<dyn Send>> {
        let mut rejected: Vec<Box<dyn Send>> = Vec::new();
        for started in done.0 {
            let Started {
                id,
                monitor,
                outputs,
                replay,
            } = started;
            if let Some((source, pipeline)) = monitor {
                let listed = self.sources.iter().any(|s| Arc::ptr_eq(s, &source));
                match pipeline {
                    Ok(pipeline) if listed && !self.monitors.contains_key(&id) => {
                        self.monitors.insert(id.clone(), pipeline);
                        self.start_errors.remove(&id);
                    }
                    Ok(pipeline) => rejected.push(Box::new(pipeline)),
                    Err(error) if listed => {
                        self.start_errors.insert(id.clone(), error);
                    }
                    Err(_) => {}
                }
            }
            if let Some((monitor, legs)) = outputs {
                if self.is_current(&id, &monitor) && !self.outputs.contains_key(&id) {
                    self.outputs.insert(id.clone(), ChannelOutputs { legs });
                } else {
                    rejected.push(Box::new(legs));
                }
            }
            if let Some((index, codec, leg)) = replay {
                let current = leg
                    .as_ref()
                    .is_none_or(|(_, monitor)| self.is_current(&id, monitor));
                match self.replays.get_mut(&id) {
                    Some(r) if Arc::ptr_eq(&r.index, &index) => {
                        r.starting = false;
                        match leg {
                            Some((Ok(leg), monitor)) if current && r.config.codec == codec => {
                                rejected.extend(
                                    r.leg
                                        .replace((leg, monitor))
                                        .map(|old| Box::new(old) as Box<dyn Send>),
                                );
                                r.error = None;
                            }
                            Some((Err(error), _)) if current => r.error = Some(error),
                            leg => rejected.push(Box::new(leg)),
                        }
                    }
                    _ => rejected.push(Box::new(leg)),
                }
                rejected.push(Box::new(index));
            }
        }
        rejected
    }

    /// Run `work` and apply it in place, blocking: before the manager is
    /// shared, at startup.
    pub fn run_now(&mut self, work: Work) {
        let done = work.run();
        drop(self.apply(done));
    }

    fn is_current(&self, source_id: &str, monitor: &Arc<MonitorPipeline>) -> bool {
        self.monitors
            .get(source_id)
            .is_some_and(|m| Arc::ptr_eq(m, monitor))
    }

    /// Connect to these NDI senders from the next scan.
    pub fn set_ndi_subscriptions(&mut self, subs: NdiSubscriptionsInput) {
        self.ndi = subs;
    }

    pub fn ndi_subscriptions(&self) -> NdiSubscriptionsDto {
        let own = self.own_senders();
        let mut senders: Vec<NdiSenderDto> = self
            .ndi_monitor
            .current_sources()
            .into_iter()
            .map(|s| NdiSenderDto {
                ndi_name: s.ndi_name().to_string(),
                source_id: s.id().to_string(),
                connected: self.get_source(s.id()).is_some(),
                in_use: self.in_use(s.id()),
                own: own.contains(s.ndi_name()),
            })
            .collect();
        senders.sort_by(|a, b| a.ndi_name.cmp(&b.ndi_name));
        NdiSubscriptionsDto {
            connect: self.ndi.connect,
            chosen: self.ndi.chosen.clone(),
            senders,
        }
    }

    fn own_senders(&self) -> OwnSenders {
        OwnSenders {
            host: crate::controller::discovery::local_hostname().unwrap_or_default(),
            names: self
                .outputs
                .values()
                .flat_map(|o| &o.legs)
                .filter_map(|(config, _)| match config {
                    OutputConfig::Ndi { ndi_name } => ndi_name.clone(),
                    _ => None,
                })
                .collect(),
        }
    }

    /// Never one of this node's own outputs; otherwise every sender, or the
    /// chosen ones and those in use.
    fn wants_ndi(&self, sender: &NdiSource, own: &OwnSenders) -> bool {
        !own.contains(sender.ndi_name())
            && match self.ndi.connect {
                NdiConnect::All => true,
                NdiConnect::Chosen => {
                    self.ndi.chosen.iter().any(|n| n == sender.ndi_name())
                        || self.in_use(sender.id())
                }
            }
    }

    /// A recording or replay buffer uses the source.
    fn in_use(&self, id: &str) -> bool {
        self.is_recording(id) || self.replays.contains_key(id)
    }

    fn is_recording(&self, id: &str) -> bool {
        self.sessions.values().any(|s| s.dto.source_id == id)
            || self.reserved.values().any(|r| r.source_id == id)
    }

    /// Add, change, or with `None` remove a source's replay buffer (deleting
    /// what it recorded, when the [`Work`] runs). A new codec starts it afresh.
    pub fn set_replay(&mut self, source_id: &str, config: Option<ReplayConfigInput>) -> Work {
        let mut work = self.work();
        let Some(config) = config else {
            if let Some(replay) = self.replays.remove(source_id) {
                info!(source = source_id, "replay buffer removed");
                work.drops.push(Box::new(replay));
            }
            return work;
        };
        let new = match self.replays.get_mut(source_id) {
            Some(replay) => {
                if replay.config.codec != config.codec {
                    work.drops
                        .extend(replay.leg.take().map(|l| Box::new(l) as Box<dyn Send>));
                }
                replay.index.lock().unwrap().set_keep(keep(&config));
                replay.config = config;
                false
            }
            None => {
                let index = Index::create(&self.replay_root.join(source_id), keep(&config));
                self.replays.insert(
                    source_id.to_string(),
                    Replay {
                        leg: None,
                        config,
                        index,
                        error: None,
                        starting: false,
                    },
                );
                true
            }
        };
        self.sync_replays(&mut work);
        // What a previous run left goes now, even before there's a source to
        // record.
        let replay = self.replays.get_mut(source_id).unwrap();
        if new && !replay.starting {
            replay.starting = true;
            work.job(source_id).replay = Some(ReplayJob {
                feed: None,
                codec: replay.config.codec,
                index: Arc::clone(&replay.index),
                clear: true,
            });
        }
        work
    }

    fn recording(replay: &Replay) -> bool {
        replay
            .leg
            .as_ref()
            .is_some_and(|(leg, _)| leg.error().is_none())
    }

    /// Start each replay buffer on its source's current monitor unless it
    /// already records from it; stop those whose source has none. Skips any
    /// already starting: the recovery loop catches one that's then left
    /// behind.
    fn sync_replays(&mut self, work: &mut Work) {
        for (id, replay) in &mut self.replays {
            if replay.starting {
                continue;
            }
            let feed = if work.starts_monitor(id) {
                Some(Feed::Starting)
            } else {
                self.monitors.get(id).cloned().map(Feed::Running)
            };
            let Some(feed) = feed else {
                work.drops
                    .extend(replay.leg.take().map(|l| Box::new(l) as Box<dyn Send>));
                continue;
            };
            let current = matches!(&feed, Feed::Running(monitor)
                if replay.leg.as_ref().is_some_and(|(_, m)| Arc::ptr_eq(m, monitor)));
            if current && Self::recording(replay) {
                continue;
            }
            work.drops
                .extend(replay.leg.take().map(|l| Box::new(l) as Box<dyn Send>));
            replay.starting = true;
            work.job(id).replay = Some(ReplayJob {
                feed: Some(feed),
                codec: replay.config.codec,
                index: Arc::clone(&replay.index),
                clear: false,
            });
        }
    }

    /// `utc_offset_us` maps the node clock to UTC.
    pub fn replay_buffers(&self, utc_offset_us: Option<i64>) -> Vec<ReplayBufferDto> {
        let now = self.clock.time().unwrap_or(gst::ClockTime::ZERO);
        let mut list: Vec<ReplayBufferDto> = self
            .replays
            .iter()
            .map(|(id, r)| {
                let index = r.index.lock().unwrap();
                // Times on a clock the node has since left mean nothing now.
                let current = index.clock() == Some(&self.clock);
                let span = index.span().filter(|_| current);
                ReplayBufferDto {
                    source_id: id.clone(),
                    source_name: self.get_source(id).map(|s| s.display_name().to_string()),
                    codec: r.config.codec,
                    minutes: r.config.minutes,
                    encoder: r
                        .leg
                        .as_ref()
                        .filter(|_| Self::recording(r))
                        .map(|(leg, _)| leg.encoder().to_string()),
                    error: r
                        .leg
                        .as_ref()
                        .and_then(|(leg, _)| leg.error())
                        .or_else(|| r.error.clone())
                        .or_else(|| {
                            (!self.monitors.contains_key(id))
                                .then(|| "the source isn't available".to_string())
                        }),
                    start_us: span.map(|(a, _)| a.useconds()),
                    end_us: span.map(|(_, b)| b.useconds()),
                    breaks_us: if current {
                        index.breaks().iter().map(|t| t.useconds()).collect()
                    } else {
                        Vec::new()
                    },
                    utc_offset_us,
                    now_us: now.useconds(),
                    bytes: index.bytes(),
                }
            })
            .collect();
        list.sort_by(|a, b| a.source_name.cmp(&b.source_name));
        list
    }

    /// Part of a source's replay buffer, `from` to `to` (node clock), to play on a channel.
    pub fn replay_clip(
        &self,
        source_id: &str,
        from: gst::ClockTime,
        to: Option<gst::ClockTime>,
    ) -> Result<ReplayClip> {
        let replay = self
            .replays
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("that source has no replay buffer"))?;
        if replay.index.lock().unwrap().clock() != Some(&self.clock) {
            bail!(
                "the replay buffer is on another clock; it starts again once its source restarts"
            );
        }
        Ok(ReplayClip {
            source_id: source_id.to_string(),
            clip_id: None,
            span: Index::clip(&replay.index, from, to)?,
        })
    }

    /// Start a channel's changed or not-running outputs on its current monitor
    /// and stop unwanted ones. Unchanged identities keep running, so a rename
    /// only restarts outputs named after the channel.
    fn reconcile_outputs(
        &mut self,
        id: &str,
        name: &str,
        configs: Vec<OutputConfig>,
        program: Program,
        work: &mut Work,
    ) {
        let wanted: Vec<OutputConfig> = configs.iter().map(|c| output::identity(c, name)).collect();
        let mut old = self.outputs.remove(id).map(|o| o.legs).unwrap_or_default();
        if old.len() == wanted.len() && old.iter().zip(&wanted).all(|((i, _), w)| i == w) {
            self.outputs
                .insert(id.to_string(), ChannelOutputs { legs: old });
            return;
        }
        // Stop unwanted legs before starting new ones: an NDI name can only be
        // sent once, and an SRT port bound once.
        let kept: Vec<Option<OutputLeg>> = wanted
            .iter()
            .map(|w| {
                let i = old.iter().position(|(identity, leg)| {
                    identity == w && leg.as_ref().is_ok_and(|l| l.error().is_none())
                })?;
                old.remove(i).1.ok()
            })
            .collect();
        work.drops.push(Box::new(old));
        let feed = if work.starts_monitor(id) {
            Feed::Starting
        } else if let Some(monitor) = self.monitors.get(id) {
            Feed::Running(Arc::clone(monitor))
        } else {
            work.drops.push(Box::new(kept));
            return;
        };
        work.job(id).outputs = Some(OutputJob {
            feed,
            name: name.to_string(),
            program,
            legs: wanted
                .into_iter()
                .zip(configs)
                .zip(kept)
                .map(|((identity, config), kept)| (identity, config, kept))
                .collect(),
        });
    }

    /// Drop channels' Decklink outputs so the next scan restarts them (with
    /// a card's new profile). The [`Work`] stops them.
    pub fn reset_decklink_outputs(&mut self) -> Work {
        let mut work = self.work();
        let decklink: Vec<String> = self
            .outputs
            .iter()
            .filter(|(_, o)| {
                o.legs
                    .iter()
                    .any(|(identity, _)| matches!(identity, OutputConfig::Decklink { .. }))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in decklink {
            work.drops.extend(
                self.outputs
                    .remove(&id)
                    .map(|o| Box::new(o) as Box<dyn Send>),
            );
        }
        work
    }

    /// Tell a channel's outputs whether it has something loaded.
    pub fn set_on_air(&self, source_id: &str, on_air: bool) {
        if let Some(outputs) = self.outputs.get(source_id) {
            for (_, leg) in &outputs.legs {
                let Ok(leg) = leg else { continue };
                leg.set_on_air(on_air);
            }
        }
    }

    /// A channel's outputs and whether each is sending.
    pub fn output_status(&self, source_id: &str) -> Vec<OutputStatusDto> {
        let Some(outputs) = self.outputs.get(source_id) else {
            return Vec::new();
        };
        outputs
            .legs
            .iter()
            .map(|(_, leg)| match leg {
                Ok(leg) => OutputStatusDto {
                    label: leg.label().to_string(),
                    error: leg.error(),
                    receivers: leg.receivers(),
                    detail: leg.detail(),
                },
                Err((label, error)) => OutputStatusDto {
                    label: label.clone(),
                    error: Some(error.clone()),
                    receivers: None,
                    detail: None,
                },
            })
            .collect()
    }

    pub fn thumbnail(&self, source_id: &str) -> Option<crate::pipeline::Thumbnail> {
        self.monitors.get(source_id)?.thumbnail.get()
    }

    pub fn all_audio_levels(&self) -> Vec<(String, Vec<ChannelLevelDto>)> {
        self.monitors
            .iter()
            .filter_map(|(id, m)| m.audio_meter.get().map(|lvl| (id.clone(), lvl)))
            .collect()
    }

    /// The format `source_id`'s monitor is producing; see [`MonitorPipeline::source_format`].
    pub fn source_format(&self, source_id: &str) -> SourceFormat {
        self.monitors
            .get(source_id)
            .map(|m| m.source_format())
            .unwrap_or_default()
    }

    /// Reserve a recording of `req.source_id` with `legs`, one per
    /// `req.outputs`, for [`RecordingStart::run`] to start off the lock and
    /// [`Self::finish_start_recording`] to add. With `req.start_at` on this
    /// node's clock domain, legs begin at the first frame due at or after it;
    /// otherwise now. `utc_offset_us` maps the domain to UTC, for timecode.
    pub fn begin_start_recording(
        &mut self,
        req: &StartRecordingRequest,
        legs: &[(PathBuf, RecordingProfile)],
        utc_offset_us: Option<i64>,
    ) -> Result<RecordingStart> {
        let source_id = req.source_id.as_str();
        let outputs = &req.outputs;
        if self.is_recording(source_id) {
            bail!("source {source_id} already has an active recording");
        }
        let monitor = self
            .monitors
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("no monitor running for source {source_id}"))?;
        // Another start may have taken a path since it was picked. Paths are
        // reserved under this lock until the files are open, so the check
        // can't race. Recording into a file truncates it; a failed start
        // deletes it.
        for (path, _) in legs {
            let active = self
                .sessions
                .values()
                .any(|s| s.dto.output_paths.iter().any(|p| Path::new(p) == path))
                || self.reserved.values().any(|r| r.paths.contains(path));
            if active || path.exists() {
                bail!("{} already exists", path.display());
            }
        }

        // A source still on a previous clock (busy when it changed) can't be
        // placed on the shared one.
        let on_node_clock = monitor.clock() == &self.clock;
        let start_at = match req.start_at.as_ref() {
            Some(at) if on_node_clock && at.domain == self.clock_domain => {
                let at = gst::ClockTime::from_useconds(at.time_us);
                let now = self.clock.time().unwrap_or(gst::ClockTime::ZERO);
                if at > now + gst::ClockTime::try_from(MAX_START_LEAD).unwrap() {
                    bail!(
                        "the start time is {:.1} s away; at most {} s ahead is allowed",
                        (at - now).seconds_f64(),
                        MAX_START_LEAD.as_secs()
                    );
                }
                if at < now {
                    warn!(source = source_id, late = %(now - at), "synchronized start arrived late");
                }
                Some(at)
            }
            Some(at) => {
                warn!(
                    source = source_id,
                    wanted = %at.domain,
                    have = %self.clock_domain,
                    on_node_clock,
                    "not on the requested clock; starting now"
                );
                None
            }
            None => None,
        };
        // Off the node clock, this machine's own wall clock maps it.
        let utc_offset_us = on_node_clock
            .then_some(utc_offset_us)
            .flatten()
            .or_else(|| crate::clock::local_utc_offset_us(monitor.clock()));
        let session_clock = on_node_clock.then(|| SessionClockDto {
            domain: self.clock_domain.clone(),
            start_at_us: start_at.map(|t| t.useconds()),
            first_frame_us: None,
            utc_offset_us,
            first_frame_utc: None,
        });

        let id = Uuid::new_v4().to_string();
        let failures = self.leg_failures.clone();
        let session_id = id.clone();
        let on_error: OnLegError = Arc::new(move |path, error| {
            let _ = failures.send(LegFailure {
                session_id: session_id.clone(),
                path: path.to_path_buf(),
                error,
            });
        });
        let files_tx = self.leg_files.clone();
        let files_session = id.clone();
        let on_file: OnLegFile = Arc::new(move || {
            let _ = files_tx.send(files_session.clone());
        });

        let dto = RecordingSessionDto {
            id,
            source_id: source_id.to_string(),
            preset_id: req.preset_id.clone().unwrap_or_default(),
            source_name: self
                .get_source(source_id)
                .map(|s| s.display_name().to_string()),
            preset_name: req.preset_name.clone(),
            started_at: Utc::now().to_rfc3339(),
            stopped_at: None,
            outputs: outputs
                .iter()
                .zip(legs)
                .map(|(o, (_, profile))| profile.session_output(&o.name, o.preview))
                .collect(),
            output_paths: legs.iter().map(|(p, _)| p.display().to_string()).collect(),
            dropped_frames: vec![0; legs.len()],
            files: Vec::new(),
            status: RecordingStatus::Active,
            error_message: None,
            clock: session_clock,
        };
        self.reserved.insert(
            dto.id.clone(),
            Reservation {
                source_id: source_id.to_string(),
                paths: legs.iter().map(|(p, _)| p.clone()).collect(),
            },
        );
        Ok(RecordingStart {
            monitor: Arc::clone(monitor),
            legs: legs.to_vec(),
            on_error,
            on_file,
            start_at,
            utc_offset_us,
            dto,
            outputs_key: crate::capacity::outputs_key(outputs),
        })
    }

    /// Add a recording whose legs have started, and release its reservation.
    /// If its source's monitor was replaced meanwhile, nothing would feed
    /// them: the legs come back with the error, to discard off the lock.
    pub fn finish_start_recording(
        &mut self,
        started: StartedRecording,
    ) -> Result<RecordingSessionDto, (anyhow::Error, Vec<RecordingLeg>)> {
        let StartedRecording { start, legs } = started;
        self.reserved.remove(&start.dto.id);
        let legs = legs.map_err(|e| (e, Vec::new()))?;
        let source_id = start.dto.source_id.clone();
        if !self.is_current(&source_id, &start.monitor) {
            return Err((
                anyhow::anyhow!("source {source_id} restarted while its recording started"),
                legs,
            ));
        }
        let dto = RecordingSessionDto {
            files: legs.iter().map(RecordingLeg::files).collect(),
            ..start.dto
        };
        info!(
            id = %dto.id,
            source = %source_id,
            legs = legs.len(),
            start_at = ?start.start_at,
            "recording started"
        );
        self.sessions.insert(
            dto.id.clone(),
            ActiveSession {
                legs,
                dto: dto.clone(),
                failed_legs: 0,
                outputs_key: start.outputs_key,
                started: Instant::now(),
            },
        );
        Ok(dto)
    }

    /// Active sessions, with their live dropped-frame counts.
    pub fn active_sessions(&self) -> Vec<RecordingSessionDto> {
        self.sessions
            .values()
            .map(|s| RecordingSessionDto {
                dropped_frames: s.dropped_frames(),
                files: s.files(),
                clock: s.clock(),
                ..s.dto.clone()
            })
            .collect()
    }

    /// Every active leg's files and how long its session has recorded.
    pub fn active_leg_files(&self) -> Vec<(Vec<String>, Duration)> {
        self.sessions
            .values()
            .flat_map(|s| s.legs.iter().map(|leg| (leg.files(), s.started.elapsed())))
            .collect()
    }

    /// What runs on this node, against its benchmarked capacity: sessions,
    /// recording replay buffers but `skip_buffers`, and channels but `skip_channels`.
    pub fn running_load(&self, skip_buffers: &[String], skip_channels: &[String]) -> RunningLoad {
        let feeds = self
            .sessions
            .values()
            .map(|s| (s.outputs_key.clone(), self.source_format(&s.dto.source_id)))
            .collect();
        let buffers = self
            .replays
            .iter()
            .filter(|(id, r)| Self::recording(r) && !skip_buffers.contains(id))
            .map(|(id, r)| {
                (
                    crate::capacity::replay_key(r.config.codec),
                    self.source_format(id),
                )
            })
            .collect();
        // Idle or playing alike: a channel's program and outputs run either way.
        let channels = self
            .sources
            .iter()
            .filter(|s| {
                s.playout().is_some()
                    && self.monitors.contains_key(s.id())
                    && !skip_channels.iter().any(|id| id == s.id())
            })
            .map(|s| {
                (
                    crate::capacity::channel_key(s.outputs()),
                    self.source_format(s.id()),
                )
            })
            .collect();
        RunningLoad {
            feeds,
            buffers,
            channels,
        }
    }

    /// Take a session out of the active set and hand back the detach work.
    ///
    /// Run the job (and the persist/notify after it) on a spawned task, not
    /// inline in the HTTP handler: a dropped handler future (client disconnect)
    /// cancels detach mid-flight, orphaning a branch that keeps recording and
    /// that no later stop can reach. A second call while the first runs gets
    /// `StopOutcome::Join` on the same result.
    pub fn begin_stop_recording(&mut self, session_id: &str) -> StopOutcome {
        if let Some(job) = self.take_session(session_id) {
            StopOutcome::Start(Box::new(job))
        } else if let Some(rx) = self.stopping.get(session_id) {
            StopOutcome::Join(rx.clone())
        } else {
            StopOutcome::NotFound
        }
    }

    /// Record a leg failure on its running session; its other legs keep
    /// recording. `None` if the session is no longer active.
    pub fn note_leg_failure(&mut self, failure: &LegFailure) -> Option<NotedFailure> {
        let session = self.sessions.get_mut(&failure.session_id)?;
        session.failed_legs += 1;
        let msg = format!("{}: {}", failure.path.display(), failure.error);
        let error = match session.dto.error_message.take() {
            Some(prev) => format!("{prev}; {msg}"),
            None => msg,
        };
        session.dto.error_message = Some(error.clone());
        Some(NotedFailure {
            source_id: session.dto.source_id.clone(),
            error,
            all_failed: session.failed_legs >= session.legs.len(),
        })
    }

    /// Clear the "stopping" marker once a [`StopJob`] has published its result.
    pub fn finish_stop(&mut self, session_id: &str) {
        self.stopping.remove(session_id);
    }

    pub fn monitor_config(&self) -> &MonitorSettingsDto {
        &self.config
    }

    /// Apply a monitor config to all running monitors without restarting
    /// them; GStreamer renegotiates the affected branches in place.
    pub fn apply_monitor_config(&mut self, config: MonitorSettingsDto) {
        self.config = config;
        for pipeline in self.monitors.values() {
            pipeline.reconfigure(&self.config);
        }
    }

    fn take_session(&mut self, session_id: &str) -> Option<StopJob> {
        let session = self.sessions.remove(session_id)?;
        // Nothing is fed to the legs once they're stopping, so this is final.
        let dto = RecordingSessionDto {
            dropped_frames: session.dropped_frames(),
            files: session.files(),
            clock: session.clock(),
            ..session.dto
        };
        let legs = session.legs;
        let (tx, rx) = watch::channel(None);
        self.stopping.insert(session_id.to_string(), rx);
        Some(StopJob { legs, dto, tx })
    }

    /// Take a source's monitor and what runs on it; its outputs and replay
    /// leg stop with `work`.
    fn disconnect(&mut self, source_id: &str, work: &mut Work) -> Option<Teardown> {
        if let Some(outputs) = self.outputs.remove(source_id) {
            work.drops.push(Box::new(outputs));
        }
        if let Some(leg) = self.replays.get_mut(source_id).and_then(|r| r.leg.take()) {
            work.drops.push(Box::new(leg));
        }
        let pipeline = self.monitors.remove(source_id)?;
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.dto.source_id == source_id)
            .map(|(id, _)| id.clone())
            .collect();
        let stops = session_ids
            .iter()
            .filter_map(|id| self.take_session(id))
            .collect();
        Some(Teardown {
            source_id: source_id.to_string(),
            pipeline,
            stops,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{
        ChannelConfig, ChromaSubsampling, Container, LiveVideoFormat, OutputAdvanced,
        PresetOutputInput, ReplayCodec, SourceConfig, VideoCodec,
    };
    use crate::pipeline::rtsp;
    use std::collections::HashSet;

    fn manager() -> SourceManager {
        gst::init().unwrap();
        let (failures, _) = mpsc::unbounded_channel();
        let (files, _) = mpsc::unbounded_channel();
        let mut mgr = SourceManager::new(
            MonitorSettingsDto::default(),
            (gst::SystemClock::obtain(), "test".into()),
            NdiMonitor::start(),
            Arc::new(LocalDevices::start()),
            Displays::none(),
            failures,
            files,
        );
        let root = std::env::temp_dir()
            .join("capture-room-manager-tests")
            .join(Uuid::new_v4().to_string());
        mgr.set_replay_root(root.join("buffers"), root.join("clips"));
        mgr
    }

    /// A channel `id` with an SRT output on `port` and an RTSP mount.
    fn channel(id: &str, name: &str, port: u16) -> ConfiguredSourceDto {
        ConfiguredSourceDto {
            id: id.into(),
            name: name.into(),
            config: SourceConfig::Channel(ChannelConfig {
                format: LiveVideoFormat {
                    width: 640,
                    height: 360,
                    fps_num: 30,
                    fps_den: 1,
                },
                audio_channels: 2,
                outputs: vec![
                    OutputConfig::Srt {
                        url: format!("srt://:{port}"),
                        latency_ms: 120,
                        bitrate_kbps: 2000,
                        passphrase: None,
                    },
                    OutputConfig::Rtsp {
                        path: None,
                        bitrate_kbps: 2000,
                    },
                ],
            }),
            created_at: String::new(),
        }
    }

    /// The same channel at another size: its monitor is rebuilt.
    fn resized(mut config: ConfiguredSourceDto) -> ConfiguredSourceDto {
        if let SourceConfig::Channel(c) = &mut config.config {
            c.format.width = 320;
            c.format.height = 180;
        }
        config
    }

    fn scan_now(mgr: &mut SourceManager, configs: &[ConfiguredSourceDto]) -> Vec<Teardown> {
        let (teardowns, work) = mgr.scan(configs);
        mgr.run_now(work);
        teardowns
    }

    fn leg_keys(mgr: &SourceManager, id: &str) -> Vec<Option<usize>> {
        mgr.outputs[id]
            .legs
            .iter()
            .map(|(_, leg)| leg.as_ref().ok().map(OutputLeg::key))
            .collect()
    }

    fn stop(teardowns: Vec<Teardown>) {
        for t in teardowns {
            let _ = t.pipeline.stop();
        }
    }

    /// A rename keeps the program running (and whatever records it), and
    /// restarts only the outputs named after the channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn rename_keeps_program() {
        let mut mgr = manager();
        assert!(scan_now(&mut mgr, &[channel("c", "One", 9311)]).is_empty());
        let monitor = mgr.monitors["c"].clone();
        let before = leg_keys(&mgr, "c");
        assert!(
            before.iter().all(Option::is_some),
            "{:?}",
            mgr.output_status("c")
        );

        let teardowns = scan_now(&mut mgr, &[channel("c", "Two", 9311)]);
        assert!(teardowns.is_empty(), "the program restarted");
        assert!(Arc::ptr_eq(&monitor, &mgr.monitors["c"]));
        assert_eq!(mgr.get_source("c").unwrap().display_name(), "Two");
        let after = leg_keys(&mgr, "c");
        assert_eq!(after[0], before[0], "the SRT output restarted");
        assert_ne!(after[1], before[1], "the RTSP mount kept the old name");
        assert_eq!(
            mgr.output_status("c")[1].label,
            format!("RTSP {}", rtsp::mount_path(None, "Two"))
        );

        let _ = monitor.stop();
    }

    /// Each source starts on its own thread, its outputs on its own monitor.
    #[tokio::test(flavor = "multi_thread")]
    async fn sources_start_side_by_side() {
        let mut mgr = manager();
        let configs = [channel("a", "A", 9312), channel("b", "B", 9313)];
        let (_, work) = mgr.scan(&configs);
        assert_eq!(work.jobs.len(), 2);
        assert!(!mgr.is_monitored("a"), "started under the lock");
        mgr.run_now(work);
        for id in ["a", "b"] {
            assert!(mgr.is_monitored(id), "{id} has no monitor");
            let status = mgr.output_status(id);
            assert!(status.iter().all(|o| o.error.is_none()), "{status:?}");
        }
        for monitor in mgr.monitors.values() {
            let _ = monitor.stop();
        }
    }

    /// Outputs started on a monitor that has since been replaced are turned
    /// away, not put on a channel that no longer feeds them.
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_outputs_rejected() {
        let mut mgr = manager();
        scan_now(&mut mgr, &[channel("c", "One", 9314)]);
        let (_, renamed) = mgr.scan(&[channel("c", "Two", 9314)]);
        let done = renamed.run();
        let (teardowns, rebuilt) = mgr.scan(&[resized(channel("c", "Two", 9314))]);
        assert!(!mgr.apply(done).is_empty(), "stale outputs were kept");
        assert!(!mgr.outputs.contains_key("c"));
        mgr.run_now(rebuilt);
        assert_eq!(mgr.output_status("c").len(), 2);
        stop(teardowns);
        let _ = mgr.monitors["c"].stop();
    }

    /// A replay leg that starts after its buffer was removed is turned away.
    #[tokio::test(flavor = "multi_thread")]
    async fn removed_replay_rejected() {
        let mut mgr = manager();
        scan_now(&mut mgr, &[channel("c", "One", 9315)]);
        let buffer = ReplayConfigInput {
            codec: ReplayCodec::Mjpeg,
            minutes: 1,
        };
        let added = mgr.set_replay("c", Some(buffer));
        let done = added.run();
        let removed = mgr.set_replay("c", None);
        mgr.run_now(removed);
        assert!(!mgr.apply(done).is_empty(), "the leg was kept");
        assert!(mgr.replays.is_empty());
        let _ = mgr.monitors["c"].stop();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recording_start_reserves_its_source() {
        let mut mgr = manager();
        scan_now(&mut mgr, &[channel("c", "One", 9316)]);
        let dir = mgr.replay_root.parent().unwrap().join("recordings");
        std::fs::create_dir_all(&dir).unwrap();
        let output = PresetOutputInput {
            name: "a".into(),
            codec: VideoCodec::H264,
            container: Container::Mkv,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: String::new(),
            preview: false,
            advanced: OutputAdvanced::default(),
        };
        let profile = RecordingProfile::from_output(&output).unwrap();
        let req = StartRecordingRequest {
            source_id: "c".into(),
            preset_id: None,
            preset_name: None,
            outputs: vec![output],
            start_at: None,
        };
        let start = mgr
            .begin_start_recording(&req, &[(dir.join("a.mkv"), profile.clone())], None)
            .unwrap();
        assert!(
            mgr.begin_start_recording(&req, &[(dir.join("b.mkv"), profile.clone())], None)
                .is_err(),
            "a second start took the source"
        );
        let started = start.run();

        // The source goes while its legs start.
        let teardowns = scan_now(&mut mgr, &[]);
        let Err((_, legs)) = mgr.finish_start_recording(started) else {
            panic!("a recording was added with no source");
        };
        assert_eq!(legs.len(), 1);
        legs.into_iter().for_each(RecordingLeg::discard);
        assert!(mgr.reserved.is_empty());
        assert!(mgr.active_sessions().is_empty());
        stop(teardowns);
    }

    #[test]
    fn ndi_senders_connected_by_subscription() {
        let mut mgr = manager();
        let own = OwnSenders {
            host: "studio".into(),
            names: HashSet::from(["Program".to_string()]),
        };
        let cam1 = NdiSource::named("CAM-1 (Camera 1)");
        let cam2 = NdiSource::named("CAM-2 (Camera 2)");

        assert!(mgr.wants_ndi(&cam1, &own));
        assert!(mgr.wants_ndi(&cam2, &own));
        assert!(!mgr.wants_ndi(&NdiSource::named("STUDIO (Program)"), &own));

        mgr.set_ndi_subscriptions(NdiSubscriptionsInput {
            connect: NdiConnect::Chosen,
            chosen: vec!["CAM-1 (Camera 1)".into()],
        });
        assert!(mgr.wants_ndi(&cam1, &own));
        assert!(!mgr.wants_ndi(&cam2, &own));

        // A replay buffer keeps its sender connected.
        let buffer = ReplayConfigInput {
            codec: ReplayCodec::Mjpeg,
            minutes: 1,
        };
        let work = mgr.set_replay(cam2.id(), Some(buffer));
        mgr.run_now(work);
        assert!(mgr.wants_ndi(&cam2, &own));
        let work = mgr.set_replay(cam2.id(), None);
        mgr.run_now(work);
        assert!(!mgr.wants_ndi(&cam2, &own));
    }
}
