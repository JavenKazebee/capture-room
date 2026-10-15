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
    ChannelLevelDto, ConfiguredSourceDto, LinkState, MonitorSettingsDto, OutputConfig,
    OutputStatusDto, RecordingSessionDto, RecordingStatus, ReplayBufferDto, ReplayConfigInput,
    SessionClockDto, StartRecordingRequest, TransportState,
};
use crate::pipeline::monitor::{MonitorPipeline, SourceFormat};
use crate::pipeline::output::{self, OutputLeg, Program};
use crate::pipeline::profile::RecordingProfile;
use crate::pipeline::recording::{self, OnLegError, OnLegFile, RecordingLeg};
use crate::pipeline::replay::{Index, ReplayIndex, ReplayLeg};

use super::channel::ReplayClip;
use super::device::LocalDevices;
use super::ndi::NdiMonitor;
use super::{configured, InputSource};
use crate::display::Displays;

// ── Stop / teardown handoff ───────────────────────────────────────────────────

/// Resolves to a stopping session's final DTO once its legs have finished.
pub type StopResult = watch::Receiver<Option<RecordingSessionDto>>;

struct ActiveSession {
    legs: Vec<RecordingLeg>,
    dto: RecordingSessionDto,
    /// Legs that have failed while recording (each reports only once).
    failed_legs: usize,
    /// What the outputs encode (see [`crate::capacity::outputs_key`]), to
    /// count the session against the node's benchmarked capacity.
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
/// close their files. The caller runs it — see
/// [`SourceManager::begin_stop_recording`].
pub struct StopJob {
    pub legs: Vec<RecordingLeg>,
    pub dto: RecordingSessionDto,
    /// Publish the final DTO here, then call [`SourceManager::finish_stop`].
    pub tx: watch::Sender<Option<RecordingSessionDto>>,
}

/// A monitor removed from the manager. The caller stops its recordings, then
/// its pipeline — on a spawned task, like a [`StopJob`].
pub struct Teardown {
    pub source_id: String,
    pub pipeline: Arc<MonitorPipeline>,
    pub stops: Vec<StopJob>,
}

/// What runs on a node, each as its capacity key and its source's format
/// (see [`SourceManager::running_load`]).
pub struct RunningLoad {
    /// Recording sessions.
    pub feeds: Vec<(String, SourceFormat)>,
    /// Replay buffers that record.
    pub buffers: Vec<(String, SourceFormat)>,
    /// Playout channels whose program runs.
    pub channels: Vec<(String, SourceFormat)>,
}

impl RunningLoad {
    /// All of it, to pass to [`crate::capacity::Capacity::load`].
    pub fn iter(&self) -> impl Iterator<Item = (&str, SourceFormat)> + '_ {
        self.feeds
            .iter()
            .chain(&self.buffers)
            .chain(&self.channels)
            .map(|(key, format)| (key.as_str(), *format))
    }
}

/// A recording leg that failed while its session was running. Sent the moment
/// it happens; the receiver applies it with [`SourceManager::note_leg_failure`].
pub struct LegFailure {
    pub session_id: String,
    pub path: PathBuf,
    pub error: String,
}

/// A leg failure applied to its running session.
pub struct NotedFailure {
    pub source_id: String,
    /// The session's accumulated error message.
    pub error: String,
    /// Every leg has now failed, so nothing is being recorded.
    pub all_failed: bool,
}

pub enum StopOutcome {
    /// This call gets to do the work.
    Start(Box<StopJob>),
    /// A stop for this session is already in flight (started by another
    /// request or a teardown); wait on its result instead.
    Join(StopResult),
    /// Not active and not stopping. The caller falls back to the DB for an
    /// orphaned row (e.g. after a crash).
    NotFound,
}

/// A channel's output, sending, or its label and why it couldn't start.
type Leg = Result<OutputLeg, (String, String)>;

/// A playout channel's running outputs.
struct ChannelOutputs {
    /// Each output's [`output::identity`], with its leg.
    legs: Vec<(OutputConfig, Leg)>,
}

/// A source's replay buffer. Kept while its source comes and goes (an NDI
/// sender restarting), so what it recorded stays playable.
struct Replay {
    /// Recording, from this monitor. Dropped before the index: it writes
    /// into it.
    leg: Option<(ReplayLeg, Arc<MonitorPipeline>)>,
    config: ReplayConfigInput,
    index: ReplayIndex,
    /// Why it couldn't start recording.
    error: Option<String>,
}

fn keep(config: &ReplayConfigInput) -> gst::ClockTime {
    gst::ClockTime::from_seconds(u64::from(config.minutes) * 60)
}

// ── SourceManager ─────────────────────────────────────────────────────────────

/// Owns the sources, their monitor pipelines, and active recording sessions.
/// The single point of truth for all capture state on a node.
///
/// Nothing here awaits: the manager lives behind an `RwLock` that every API
/// call and the WS emitter share. Work that has to wait on GStreamer (draining
/// a recording to EOS) is handed back as a [`StopJob`] or [`Teardown`].
pub struct SourceManager {
    config: MonitorSettingsDto,
    /// The node clock: new monitors run on it.
    clock: gst::Clock,
    /// Its domain (see `ClockStatusDto::domain`), for synchronized starts.
    clock_domain: String,
    sources: Vec<Box<dyn InputSource>>,
    monitors: HashMap<String, Arc<MonitorPipeline>>,
    /// Why a source's monitor couldn't be started, until a later start works
    /// or the source goes away. A running monitor reports its own errors.
    start_errors: HashMap<String, String>,
    sessions: HashMap<String, ActiveSession>, // session_id → session
    /// Playout channels' outputs, by source id: their config, and each
    /// output's leg (or why it couldn't start).
    outputs: HashMap<String, ChannelOutputs>,
    /// Sessions whose legs are draining. Lets a duplicate/retried
    /// stop join the in-flight result instead of reporting "already stopped".
    stopping: HashMap<String, StopResult>,
    ndi_monitor: NdiMonitor,
    devices: Arc<LocalDevices>,
    /// The node's screens, for display outputs.
    displays: Displays,
    leg_failures: mpsc::UnboundedSender<LegFailure>,
    /// Session ids whose legs opened a new file (a split), so the file list
    /// can be saved.
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
            outputs: HashMap::new(),
            stopping: HashMap::new(),
            ndi_monitor,
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

    /// Where replay buffers and saved clips are kept (temporary folders
    /// until set). Set before any buffer is. Buffers are cleared on start;
    /// clips stay.
    pub fn set_replay_root(&mut self, buffers: PathBuf, clips: PathBuf) {
        self.replay_root = buffers;
        self.clip_root = clips;
    }

    /// The node's name, for NDI senders to see it by.
    pub fn set_node_name(&mut self, name: &str) {
        self.ndi_monitor.set_node_name(name);
    }

    /// Where replay buffers are kept.
    pub fn replay_root(&self) -> &Path {
        &self.replay_root
    }

    /// The folder a saved replay clip's files are in.
    pub fn replay_clip_dir(&self, clip_id: &str) -> PathBuf {
        self.clip_root.join(clip_id)
    }

    // ── Source access ─────────────────────────────────────────────────────────

    /// The node's capture devices.
    pub fn devices(&self) -> &Arc<LocalDevices> {
        &self.devices
    }

    /// Whether the NDI library loaded (see [`NdiMonitor`]).
    pub fn ndi_runtime_loaded(&self) -> bool {
        self.ndi_monitor.runtime_loaded()
    }

    /// The node's screens.
    pub fn displays(&self) -> &Displays {
        &self.displays
    }

    pub fn sources(&self) -> &[Box<dyn InputSource>] {
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

    /// Whether any source lacks a healthy monitor — it failed, or never
    /// started — or is idle on an old clock, and is waiting for a rescan to
    /// (re)start it.
    pub fn needs_rescan(&self) -> bool {
        self.sources
            .iter()
            .any(|s| !self.is_healthy(s.id()) || self.should_migrate(s.as_ref()))
            || self
                .replays
                .iter()
                .any(|(id, r)| self.monitors.contains_key(id) && !Self::recording(r))
    }

    // ── Clock ─────────────────────────────────────────────────────────────────

    /// The node clock changed: monitors started from now on use `clock`, and
    /// running ones move to it as they become idle (see [`Self::scan`]).
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
    /// nothing is recording it and, for a channel, nothing is playing.
    fn should_migrate(&self, source: &dyn InputSource) -> bool {
        let id = source.id();
        let stale = self
            .monitors
            .get(id)
            .is_some_and(|m| m.clock() != &self.clock);
        let recording = self.sessions.values().any(|s| s.dto.source_id == id);
        let playing = source
            .playout()
            .is_some_and(|p| p.status().state != TransportState::Idle);
        stale && !recording && !playing
    }

    // ── Scan ──────────────────────────────────────────────────────────────────

    /// Rebuild the source list from configured sources and the NDI sources currently
    /// on the network. A source whose id and fingerprint are unchanged is kept
    /// as-is, monitor and recordings included — unless its monitor has failed
    /// or never started, or is idle on an old clock. Removed, changed or failed sources are torn down
    /// (returned for the caller to run); everything else gets a fresh monitor.
    /// Candidates are only descriptions, so a scan that changes nothing builds
    /// no GStreamer elements.
    pub fn scan(&mut self, configs: &[ConfiguredSourceDto]) -> Vec<Teardown> {
        let mut candidates: Vec<Box<dyn InputSource>> = configs
            .iter()
            .map(|c| configured(c, &self.devices))
            .collect();
        candidates.extend(
            self.ndi_monitor
                .current_sources()
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn InputSource>),
        );

        // Outputs are left out of a channel's fingerprint, so a kept source
        // has its old config: take the wanted outputs from the candidates.
        let wanted_outputs: HashMap<String, (String, Vec<OutputConfig>, Program)> = candidates
            .iter()
            .filter(|c| c.playout().is_some())
            .filter_map(|c| {
                let name = c.display_name().to_string();
                let program = Program::from(&c.capabilities()?);
                Some((c.id().to_string(), (name, c.outputs().to_vec(), program)))
            })
            .collect();

        let mut old: HashMap<String, Box<dyn InputSource>> = self
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
                    existing.set_name(candidate.display_name());
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
            self.sources.push(candidate);
        }
        gone.extend(old.into_keys());

        // Tear down first: a changed source restarts under the same id.
        let teardowns = gone.iter().filter_map(|id| self.disconnect(id)).collect();
        for id in &gone {
            self.start_errors.remove(id);
        }
        for id in &added {
            self.outputs.remove(id);
        }
        for id in added {
            match self.start_monitor(&id) {
                Ok(()) => {
                    self.start_errors.remove(&id);
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    warn!(source = %id, error = %error, "failed to start monitor");
                    self.start_errors.insert(id, error);
                }
            }
        }

        self.outputs.retain(|id, _| wanted_outputs.contains_key(id));
        for (id, (name, configs, program)) in wanted_outputs {
            self.reconcile_outputs(&id, &name, configs, program);
        }

        self.sync_replays();
        info!(count = self.sources.len(), "source scan complete");
        teardowns
    }

    // ── Replay buffers ────────────────────────────────────────────────────────

    /// Give a source a replay buffer, change it, or with `None` remove it
    /// (deleting what it recorded). A new codec starts it afresh.
    pub fn set_replay(&mut self, source_id: &str, config: Option<ReplayConfigInput>) {
        let Some(config) = config else {
            if self.replays.remove(source_id).is_some() {
                info!(source = source_id, "replay buffer removed");
            }
            return;
        };
        match self.replays.get_mut(source_id) {
            Some(replay) => {
                if replay.config.codec != config.codec {
                    replay.leg = None;
                }
                replay.index.lock().unwrap().set_keep(keep(&config));
                replay.config = config;
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
                    },
                );
            }
        }
        self.sync_replays();
    }

    fn recording(replay: &Replay) -> bool {
        replay
            .leg
            .as_ref()
            .is_some_and(|(leg, _)| leg.error().is_none())
    }

    /// Start each replay buffer on its source's current monitor, unless it
    /// records from it already; stop the ones whose source has none.
    fn sync_replays(&mut self) {
        for (id, replay) in &mut self.replays {
            let Some(monitor) = self.monitors.get(id) else {
                replay.leg = None;
                continue;
            };
            let current = replay
                .leg
                .as_ref()
                .is_some_and(|(_, m)| Arc::ptr_eq(m, monitor));
            if current && Self::recording(replay) {
                continue;
            }
            replay.leg = None;
            match ReplayLeg::start(monitor, replay.config.codec, &replay.index, id) {
                Ok(leg) => {
                    replay.leg = Some((leg, Arc::clone(monitor)));
                    replay.error = None;
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    warn!(source = %id, error = %error, "replay buffer failed to start");
                    replay.error = Some(error);
                }
            }
        }
    }

    /// Every replay buffer and what it holds. `utc_offset_us` maps the node
    /// clock to UTC.
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

    /// Part of a source's replay buffer, from `from` to `to` (node clock),
    /// to play on a channel.
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

    /// Start a channel's outputs that changed or aren't running (a new
    /// monitor), on its current monitor, and stop the ones no longer wanted.
    /// Outputs whose identity is unchanged keep running, so renaming a
    /// channel only restarts the outputs named after it. A changed `program`
    /// restarts the monitor, so it's a new one then.
    fn reconcile_outputs(
        &mut self,
        id: &str,
        name: &str,
        configs: Vec<OutputConfig>,
        program: Program,
    ) {
        let wanted: Vec<OutputConfig> = configs.iter().map(|c| output::identity(c, name)).collect();
        let mut old = self.outputs.remove(id).map(|o| o.legs).unwrap_or_default();
        if old.len() == wanted.len() && old.iter().zip(&wanted).all(|((i, _), w)| i == w) {
            self.outputs
                .insert(id.to_string(), ChannelOutputs { legs: old });
            return;
        }
        // Keep the running legs still wanted (one per wanted output); stop
        // the rest before starting new ones: an NDI name can only be sent
        // once, and an SRT port bound once.
        let mut kept: Vec<Option<OutputLeg>> = wanted
            .iter()
            .map(|w| {
                let i = old.iter().position(|(identity, leg)| {
                    identity == w && leg.as_ref().is_ok_and(|l| l.error().is_none())
                })?;
                old.remove(i).1.ok()
            })
            .collect();
        drop(old);
        let Some(monitor) = self.monitors.get(id) else {
            return;
        };
        let legs = configs
            .iter()
            .zip(wanted)
            .zip(kept.iter_mut())
            .map(|((config, identity), kept)| {
                if let Some(leg) = kept.take() {
                    return (identity, Ok(leg));
                }
                let context = output::OutputContext {
                    displays: &self.displays,
                    devices: &self.devices,
                };
                let sink = output::sink(config, name, program, &context);
                let leg = OutputLeg::start(monitor, sink.as_ref()).map_err(|e| {
                    let error = format!("{e:#}");
                    warn!(channel = %id, error = %error, "output failed to start");
                    (sink.label(), error)
                });
                (identity, leg)
            })
            .collect();
        self.outputs.insert(id.to_string(), ChannelOutputs { legs });
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

    // ── Thumbnail / audio access ──────────────────────────────────────────────

    pub fn thumbnail_bytes(&self, source_id: &str) -> Option<Vec<u8>> {
        self.monitors.get(source_id)?.thumbnail.get()
    }

    /// Every monitored source's latest audio levels.
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

    // ── Recording ─────────────────────────────────────────────────────────────

    /// Start recording `req.source_id` with `legs`, one per entry of
    /// `req.outputs` (which supplies each leg's name and preview flag for the
    /// session). With `req.start_at` on this node's clock domain, the legs
    /// begin at the first frame due at or after it; on another domain they
    /// start now. `utc_offset_us` is the domain's mapping to UTC (see
    /// [`crate::clock::NodeClock::utc_offset_us`]), for timecode.
    pub fn start_recording(
        &mut self,
        req: &StartRecordingRequest,
        legs: &[(PathBuf, RecordingProfile)],
        utc_offset_us: Option<i64>,
    ) -> Result<RecordingSessionDto> {
        let source_id = req.source_id.as_str();
        let outputs = &req.outputs;
        if self.sessions.values().any(|s| s.dto.source_id == source_id) {
            bail!("source {source_id} already has an active recording");
        }
        let monitor = self
            .monitors
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("no monitor running for source {source_id}"))?;
        // Paths were picked to be free, but another start may have taken one
        // since (a template without `{source}`, two sources at once). Starting
        // under this lock opens the files, so the check can't be raced.
        // Recording into a file would truncate it; a failed start deletes it.
        for (path, _) in legs {
            let active = self
                .sessions
                .values()
                .any(|s| s.dto.output_paths.iter().any(|p| Path::new(p) == path));
            if active || path.exists() {
                bail!("{} already exists", path.display());
            }
        }

        // A source still on a previous clock (it was busy when the clock
        // changed) can't be placed on the shared one.
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
        let recording_legs = recording::start_legs(
            monitor,
            &id[..8],
            legs,
            &on_error,
            &on_file,
            start_at,
            utc_offset_us,
        )?;

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
            files: recording_legs.iter().map(RecordingLeg::files).collect(),
            status: RecordingStatus::Active,
            error_message: None,
            clock: session_clock,
        };

        info!(id = %dto.id, source = source_id, legs = legs.len(), ?start_at, "recording started");
        self.sessions.insert(
            dto.id.clone(),
            ActiveSession {
                legs: recording_legs,
                dto: dto.clone(),
                failed_legs: 0,
                outputs_key: crate::capacity::outputs_key(outputs),
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

    /// Every active leg's files, with how long its session has recorded —
    /// for measuring what recordings write.
    pub fn active_leg_files(&self) -> Vec<(Vec<String>, Duration)> {
        self.sessions
            .values()
            .flat_map(|s| s.legs.iter().map(|leg| (leg.files(), s.started.elapsed())))
            .collect()
    }

    /// What runs on this node, to add up against its benchmarked capacity:
    /// the recording sessions, the replay buffers that record but those of
    /// the sources `skip_buffers`, and the playout channels but
    /// `skip_channels`.
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
        // Idle or playing alike: a channel's program and outputs run either
        // way, and it can start playing at any moment.
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
    /// The caller must run the returned job — and the persist/notify that
    /// follows it — on a task spawned independently of the triggering HTTP
    /// request (e.g. via `tokio::spawn`), NOT inline in the request handler.
    /// If the handler awaits it inline, dropping the handler's future (which
    /// happens if the client disconnects — a page refresh, a retried request)
    /// silently cancels whatever branch detach was still in flight, orphaning
    /// that branch: still linked to the tee, still recording, and — since the
    /// session was already removed here — unreachable by any future stop call.
    ///
    /// A second call for the same `session_id` while the first is still
    /// running returns `StopOutcome::Join` with a receiver for the same
    /// result, instead of falling through to a DB-only path that would
    /// report "stopped" without the pipeline actually having stopped.
    pub fn begin_stop_recording(&mut self, session_id: &str) -> StopOutcome {
        if let Some(job) = self.take_session(session_id) {
            StopOutcome::Start(Box::new(job))
        } else if let Some(rx) = self.stopping.get(session_id) {
            StopOutcome::Join(rx.clone())
        } else {
            StopOutcome::NotFound
        }
    }

    /// Record a leg failure on its still-running session (the session's other
    /// legs keep recording). Returns `None` if the session is no longer active
    /// — a failed start or a stop already in progress reports the error itself.
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

    // ── Monitor config ────────────────────────────────────────────────────────

    pub fn monitor_config(&self) -> &MonitorSettingsDto {
        &self.config
    }

    /// Apply a new global monitor config to all running monitors without
    /// restarting any pipelines. GStreamer re-negotiates the affected branches
    /// in place, so audio and thumbnails remain uninterrupted.
    pub fn apply_monitor_config(&mut self, config: MonitorSettingsDto) {
        self.config = config;
        for pipeline in self.monitors.values() {
            pipeline.reconfigure(&self.config);
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn start_monitor(&mut self, source_id: &str) -> Result<()> {
        let source = self
            .get_source(source_id)
            .ok_or_else(|| anyhow::anyhow!("source {source_id} not found"))?;
        let pipeline = Arc::new(MonitorPipeline::new(source, &self.config, &self.clock)?);
        self.monitors.insert(source_id.to_string(), pipeline);
        info!(source = source_id, "monitor started");
        Ok(())
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

    /// Remove the monitor (and any active recordings) for a source.
    fn disconnect(&mut self, source_id: &str) -> Option<Teardown> {
        self.outputs.remove(source_id);
        if let Some(replay) = self.replays.get_mut(source_id) {
            replay.leg = None;
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
    use crate::api::types::{ChannelConfig, LiveVideoFormat, SourceConfig};
    use crate::pipeline::rtsp;

    fn manager() -> SourceManager {
        gst::init().unwrap();
        let (failures, _) = mpsc::unbounded_channel();
        let (files, _) = mpsc::unbounded_channel();
        SourceManager::new(
            MonitorSettingsDto::default(),
            (gst::SystemClock::obtain(), "test".into()),
            NdiMonitor::start(),
            Arc::new(LocalDevices::start()),
            Displays::none(),
            failures,
            files,
        )
    }

    fn channel(name: &str) -> ConfiguredSourceDto {
        ConfiguredSourceDto {
            id: "c".into(),
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
                        url: "srt://:9311".into(),
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

    fn leg_keys(mgr: &SourceManager) -> Vec<Option<usize>> {
        mgr.outputs["c"]
            .legs
            .iter()
            .map(|(_, leg)| leg.as_ref().ok().map(OutputLeg::key))
            .collect()
    }

    /// A rename keeps the program running (and whatever records it), and
    /// restarts only the outputs named after the channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn rename_keeps_program() {
        let mut mgr = manager();
        assert!(mgr.scan(&[channel("One")]).is_empty());
        let monitor = mgr.monitors["c"].clone();
        let before = leg_keys(&mgr);
        assert!(
            before.iter().all(Option::is_some),
            "{:?}",
            mgr.output_status("c")
        );

        let teardowns = mgr.scan(&[channel("Two")]);
        assert!(teardowns.is_empty(), "the program restarted");
        assert!(Arc::ptr_eq(&monitor, &mgr.monitors["c"]));
        assert_eq!(mgr.get_source("c").unwrap().display_name(), "Two");
        let after = leg_keys(&mgr);
        assert_eq!(after[0], before[0], "the SRT output restarted");
        assert_ne!(after[1], before[1], "the RTSP mount kept the old name");
        assert_eq!(
            mgr.output_status("c")[1].label,
            format!("RTSP {}", rtsp::mount_path(None, "Two"))
        );

        let _ = monitor.stop();
    }
}
