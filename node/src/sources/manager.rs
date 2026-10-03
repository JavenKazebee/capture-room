use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::Utc;
use gstreamer::prelude::*;
use tokio::sync::watch;
use tracing::{info, warn};
use uuid::Uuid;

use crate::api::types::{
    ChannelLevelDto, MonitorSettingsDto, RecordingSessionDto, RecordingStatus, TestSourceConfigDto,
};
use crate::pipeline::monitor::MonitorPipeline;
use crate::pipeline::profile::RecordingProfile;
use crate::pipeline::recording::{self, RecordingLeg};

use super::ndi::NdiMonitor;
use super::test::TestSource;
use super::InputSource;

// ── Stop / teardown handoff ───────────────────────────────────────────────────

/// Resolves to a stopping session's final DTO once its legs have finished.
pub type StopResult = watch::Receiver<Option<RecordingSessionDto>>;

struct ActiveSession {
    legs: Vec<RecordingLeg>,
    dto: RecordingSessionDto,
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
    sessions: HashMap<String, ActiveSession>, // session_id → session
    /// Sessions whose legs are draining. Lets a duplicate/retried
    /// stop join the in-flight result instead of reporting "already stopped".
    stopping: HashMap<String, StopResult>,
    ndi_monitor: NdiMonitor,
}

impl SourceManager {
    pub fn new(config: MonitorSettingsDto, ndi_monitor: NdiMonitor) -> Self {
        Self {
            config,
            sources: Vec::new(),
            monitors: HashMap::new(),
            sessions: HashMap::new(),
            stopping: HashMap::new(),
            ndi_monitor,
        }
    }

    // ── Source access ─────────────────────────────────────────────────────────

    pub fn sources(&self) -> &[Box<dyn InputSource>] {
        &self.sources
    }

    pub fn get_source(&self, id: &str) -> Option<&dyn InputSource> {
        self.sources.iter().find(|s| s.id() == id).map(|s| s.as_ref())
    }

    pub fn is_monitored(&self, source_id: &str) -> bool {
        self.monitors.contains_key(source_id)
    }

    /// Why a source's monitor stopped producing, if it has.
    pub fn monitor_error(&self, source_id: &str) -> Option<String> {
        self.monitors.get(source_id)?.error()
    }

    // ── Scan ──────────────────────────────────────────────────────────────────

    /// Rebuild the source list from test configs and the NDI sources currently
    /// on the network. A source whose id and fingerprint are unchanged is kept
    /// as-is, monitor and recordings included. Removed or changed sources are
    /// torn down (returned for the caller to run); new or changed sources
    /// get a monitor.
    pub fn scan(&mut self, configs: &[TestSourceConfigDto]) -> Vec<Teardown> {
        let mut candidates: Vec<Box<dyn InputSource>> = Vec::new();
        for cfg in configs {
            match TestSource::new(cfg.clone()) {
                Ok(src) => candidates.push(Box::new(src)),
                Err(e) => warn!(id = %cfg.id, error = %e, "failed to create test source"),
            }
        }
        candidates.extend(
            self.ndi_monitor
                .current_sources()
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn InputSource>),
        );

        let mut old: HashMap<String, Box<dyn InputSource>> =
            self.sources.drain(..).map(|s| (s.id().to_string(), s)).collect();
        let mut gone = Vec::new();
        let mut added = Vec::new();
        for candidate in candidates {
            let id = candidate.id().to_string();
            if self.sources.iter().any(|s| s.id() == id) {
                warn!(id = %id, "duplicate source id, ignoring");
                continue;
            }
            match old.remove(&id) {
                Some(existing) if existing.fingerprint() == candidate.fingerprint() => {
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
        let teardowns = gone.iter().filter_map(|id| self.take_monitor(id)).collect();
        for id in added {
            if let Err(e) = self.start_monitor(&id) {
                warn!(source = %id, error = %e, "failed to start monitor");
            }
        }

        info!(count = self.sources.len(), "source scan complete");
        teardowns
    }

    // ── Manual connect / disconnect ───────────────────────────────────────────

    /// Start the monitor pipeline for a source.
    pub fn connect(&mut self, source_id: &str) -> Result<()> {
        if self.monitors.contains_key(source_id) {
            return Ok(()); // already connected
        }
        self.start_monitor(source_id)
    }

    /// Remove the monitor (and any active recordings) for a source.
    pub fn disconnect(&mut self, source_id: &str) -> Option<Teardown> {
        self.take_monitor(source_id)
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

    // ── Recording ─────────────────────────────────────────────────────────────

    /// Start a multi-leg recording session.
    /// `legs` is an ordered list of `(output_path, profile)` pairs, one per output leg.
    pub fn start_recording(
        &mut self,
        source_id: &str,
        preset_id: &str,
        legs: &[(String, RecordingProfile)],
    ) -> Result<RecordingSessionDto> {
        if self.sessions.values().any(|s| s.dto.source_id == source_id) {
            bail!("source {source_id} already has an active recording");
        }
        let monitor = self
            .monitors
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("no monitor running for source {source_id}"))?;

        let id = Uuid::new_v4().to_string();
        let leg_refs: Vec<(PathBuf, &RecordingProfile)> =
            legs.iter().map(|(p, prof)| (PathBuf::from(p), prof)).collect();
        let recording_legs = recording::start_legs(monitor, &id[..8], &leg_refs)?;

        let dto = RecordingSessionDto {
            id,
            source_id: source_id.to_string(),
            preset_id: preset_id.to_string(),
            started_at: Utc::now().to_rfc3339(),
            stopped_at: None,
            output_paths: legs.iter().map(|(p, _)| p.clone()).collect(),
            status: RecordingStatus::Active,
            error_message: None,
        };

        info!(id = %dto.id, source = source_id, legs = legs.len(), "recording started");
        self.sessions.insert(dto.id.clone(), ActiveSession { legs: recording_legs, dto: dto.clone() });
        Ok(dto)
    }

    pub fn active_sessions(&self) -> Vec<&RecordingSessionDto> {
        self.sessions.values().map(|s| &s.dto).collect()
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
        // The bin stays in its old pipeline until that teardown finishes.
        if source.gst_src_element().parent().is_some() {
            bail!("source {source_id} is still disconnecting, try again shortly");
        }

        let pipeline = Arc::new(MonitorPipeline::new(source, &self.config)?);
        self.monitors.insert(source_id.to_string(), pipeline);
        info!(source = source_id, "monitor started");
        Ok(())
    }

    fn take_session(&mut self, session_id: &str) -> Option<StopJob> {
        let ActiveSession { legs, dto } = self.sessions.remove(session_id)?;
        let (tx, rx) = watch::channel(None);
        self.stopping.insert(session_id.to_string(), rx);
        Some(StopJob { legs, dto, tx })
    }

    fn take_monitor(&mut self, source_id: &str) -> Option<Teardown> {
        let pipeline = self.monitors.remove(source_id)?;
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.dto.source_id == source_id)
            .map(|(id, _)| id.clone())
            .collect();
        let stops = session_ids.iter().filter_map(|id| self.take_session(id)).collect();
        Some(Teardown { source_id: source_id.to_string(), pipeline, stops })
    }
}
