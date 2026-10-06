use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use chrono::Utc;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use uuid::Uuid;

use crate::api::types::{
    ChannelLevelDto, ConfiguredSourceDto, LinkState, MonitorSettingsDto, PresetOutputInput,
    RecordingSessionDto, RecordingStatus,
};
use crate::pipeline::monitor::{MonitorPipeline, SourceFormat};
use crate::pipeline::profile::RecordingProfile;
use crate::pipeline::recording::{self, OnLegError, OnLegFile, RecordingLeg};

use super::device::LocalDevices;
use super::ndi::NdiMonitor;
use super::{configured, InputSource};

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
}

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

// ── SourceManager ─────────────────────────────────────────────────────────────

/// Owns the sources, their monitor pipelines, and active recording sessions.
/// The single point of truth for all capture state on a node.
///
/// Nothing here awaits: the manager lives behind an `RwLock` that every API
/// call and the WS emitter share. Work that has to wait on GStreamer (draining
/// a recording to EOS) is handed back as a [`StopJob`] or [`Teardown`].
pub struct SourceManager {
    config: MonitorSettingsDto,
    sources: Vec<Box<dyn InputSource>>,
    monitors: HashMap<String, Arc<MonitorPipeline>>,
    /// Why a source's monitor couldn't be started, until a later start works
    /// or the source goes away. A running monitor reports its own errors.
    start_errors: HashMap<String, String>,
    sessions: HashMap<String, ActiveSession>, // session_id → session
    /// Sessions whose legs are draining. Lets a duplicate/retried
    /// stop join the in-flight result instead of reporting "already stopped".
    stopping: HashMap<String, StopResult>,
    ndi_monitor: NdiMonitor,
    devices: Arc<LocalDevices>,
    leg_failures: mpsc::UnboundedSender<LegFailure>,
    /// Session ids whose legs opened a new file (a split), so the file list
    /// can be saved.
    leg_files: mpsc::UnboundedSender<String>,
}

impl SourceManager {
    pub fn new(
        config: MonitorSettingsDto,
        ndi_monitor: NdiMonitor,
        devices: Arc<LocalDevices>,
        leg_failures: mpsc::UnboundedSender<LegFailure>,
        leg_files: mpsc::UnboundedSender<String>,
    ) -> Self {
        Self {
            config,
            sources: Vec::new(),
            monitors: HashMap::new(),
            start_errors: HashMap::new(),
            sessions: HashMap::new(),
            stopping: HashMap::new(),
            ndi_monitor,
            devices,
            leg_failures,
            leg_files,
        }
    }

    // ── Source access ─────────────────────────────────────────────────────────

    /// The node's capture devices.
    pub fn devices(&self) -> &Arc<LocalDevices> {
        &self.devices
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

    /// Why a source's monitor stopped producing, or couldn't start.
    /// A live source's link state, while its monitor runs.
    pub fn link(&self, source: &dyn InputSource) -> Option<LinkState> {
        self.is_monitored(source.id())
            .then(|| source.link())
            .flatten()
    }

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
    /// started — and is waiting for a rescan to (re)start it.
    pub fn needs_rescan(&self) -> bool {
        self.sources.iter().any(|s| !self.is_healthy(s.id()))
    }

    // ── Scan ──────────────────────────────────────────────────────────────────

    /// Rebuild the source list from configured sources and the NDI sources currently
    /// on the network. A source whose id and fingerprint are unchanged is kept
    /// as-is, monitor and recordings included — unless its monitor has failed
    /// or never started. Removed, changed or failed sources are torn down
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

        let mut old: HashMap<String, Box<dyn InputSource>> = self
            .sources
            .drain(..)
            .map(|s| (s.id().to_string(), s))
            .collect();
        let mut gone = Vec::new();
        let mut added = Vec::new();
        for candidate in candidates {
            let id = candidate.id().to_string();
            if self.sources.iter().any(|s| s.id() == id) {
                warn!(id = %id, "duplicate source id, ignoring");
                continue;
            }
            match old.remove(&id) {
                Some(existing)
                    if existing.fingerprint() == candidate.fingerprint()
                        && self.is_healthy(&id) =>
                {
                    self.sources.push(existing);
                    continue;
                }
                Some(_) => gone.push(id.clone()),
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

        info!(count = self.sources.len(), "source scan complete");
        teardowns
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

    /// Start a multi-leg recording session.
    /// `legs` is an ordered list of `(output_path, profile)` pairs, one per output leg.
    /// Start recording `source_id` with `legs`, one per entry of `outputs`
    /// (which supplies each leg's name and preview flag for the session).
    pub fn start_recording(
        &mut self,
        source_id: &str,
        preset_id: &str,
        preset_name: Option<&str>,
        outputs: &[PresetOutputInput],
        legs: &[(PathBuf, RecordingProfile)],
        outputs_key: String,
    ) -> Result<RecordingSessionDto> {
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
        let recording_legs = recording::start_legs(monitor, &id[..8], legs, &on_error, &on_file)?;

        let dto = RecordingSessionDto {
            id,
            source_id: source_id.to_string(),
            preset_id: preset_id.to_string(),
            source_name: self
                .get_source(source_id)
                .map(|s| s.display_name().to_string()),
            preset_name: preset_name.map(str::to_string),
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
        };

        info!(id = %dto.id, source = source_id, legs = legs.len(), "recording started");
        self.sessions.insert(
            dto.id.clone(),
            ActiveSession {
                legs: recording_legs,
                dto: dto.clone(),
                failed_legs: 0,
                outputs_key,
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

    /// Every active session's outputs key and its source's format, to add
    /// up against the node's benchmarked capacity.
    pub fn active_feeds(&self) -> Vec<(String, SourceFormat)> {
        self.sessions
            .values()
            .map(|s| (s.outputs_key.clone(), self.source_format(&s.dto.source_id)))
            .collect()
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
        let pipeline = Arc::new(MonitorPipeline::new(source, &self.config)?);
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
            ..session.dto
        };
        let legs = session.legs;
        let (tx, rx) = watch::channel(None);
        self.stopping.insert(session_id.to_string(), rx);
        Some(StopJob { legs, dto, tx })
    }

    /// Remove the monitor (and any active recordings) for a source.
    fn disconnect(&mut self, source_id: &str) -> Option<Teardown> {
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
