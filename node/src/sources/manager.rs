use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::Utc;
use futures_util::future::join_all;
use tokio::sync::watch;
use tracing::{info, warn};
use uuid::Uuid;

use crate::api::types::{ChannelLevelDto, RecordingSessionDto};
use crate::pipeline::monitor::{MonitorConfig, MonitorPipeline, RecordingBranch};
use crate::pipeline::profile::RecordingProfile;

use super::ndi::NdiMonitor;
use super::registry::SourceRegistry;
use super::test::TestSourceConfig;
use super::{ConnectionMode, InputSource};

// ── Internal types ────────────────────────────────────────────────────────────

struct ActiveMonitor {
    pipeline: Arc<MonitorPipeline>,
}

struct ActiveSession {
    source_id: String,
    branches: Vec<RecordingBranch>,
    pub dto: RecordingSessionDto,
}

/// Result of attempting to stop a session. The caller is responsible for
/// actually detaching branches (see `begin_stop_recording`'s doc comment)
/// so that work happens outside any lock and, critically, outside the
/// lifetime of the HTTP request that triggered it.
pub enum StopOutcome {
    /// This call is the one that gets to do the work. Detach `pending`'s
    /// branches (if any), then report the result via `tx` and call
    /// `finish_stop` once done.
    Start {
        dto: RecordingSessionDto,
        pending: Option<(Arc<MonitorPipeline>, Vec<RecordingBranch>)>,
        tx: watch::Sender<Option<RecordingSessionDto>>,
    },
    /// A stop for this session is already in flight (started by another
    /// request). Await `changed()`/`borrow()` on this receiver for the
    /// final DTO instead of doing any work.
    Join(watch::Receiver<Option<RecordingSessionDto>>),
    /// No in-memory record at all — not active, not stopping. Caller should
    /// fall back to checking the DB for an orphaned row (e.g. after a crash).
    NotFound,
}

// ── SourceManager ─────────────────────────────────────────────────────────────

/// Owns the source registry, per-source monitor pipelines, and active recording
/// sessions. This is the single point of truth for all capture state on a node.
pub struct SourceManager {
    pub config: MonitorConfig,
    registry: SourceRegistry,
    monitors: HashMap<String, ActiveMonitor>,
    sessions: HashMap<String, ActiveSession>, // session_id → session
    /// Sessions whose branches are being detached by a task spawned outside
    /// any lock. Lets a duplicate/retried stop request join the in-flight
    /// result instead of being treated as "already fully stopped" — see
    /// `begin_stop_recording`.
    stopping: HashMap<String, watch::Receiver<Option<RecordingSessionDto>>>,
    ndi_monitor: NdiMonitor,
}

impl SourceManager {
    pub fn new(config: MonitorConfig, ndi_monitor: NdiMonitor) -> Self {
        Self {
            config,
            registry: SourceRegistry::new(),
            monitors: HashMap::new(),
            sessions: HashMap::new(),
            stopping: HashMap::new(),
            ndi_monitor,
        }
    }

    // ── Registry access ───────────────────────────────────────────────────────

    pub fn sources(&self) -> &[Box<dyn InputSource>] {
        self.registry.sources()
    }

    pub fn get_source(&self, id: &str) -> Option<&dyn InputSource> {
        self.registry.get(id)
    }

    pub fn is_monitored(&self, source_id: &str) -> bool {
        self.monitors.contains_key(source_id)
    }

    // ── Scan ──────────────────────────────────────────────────────────────────

    /// Replace the source list from test configs and a fresh NDI device scan,
    /// start monitors for new Auto sources, and tear down monitors for removed sources.
    pub async fn scan(&mut self, configs: &[TestSourceConfig]) -> Result<()> {
        let old_ids: HashSet<String> = self
            .registry
            .sources()
            .iter()
            .map(|s| s.id().to_string())
            .collect();

        let ndi_sources: Vec<Box<dyn InputSource>> = self
            .ndi_monitor
            .current_sources()
            .into_iter()
            .map(|s| Box::new(s) as Box<dyn InputSource>)
            .collect();

        self.registry.scan(configs, ndi_sources)?;

        let new_ids: HashSet<String> = self
            .registry
            .sources()
            .iter()
            .map(|s| s.id().to_string())
            .collect();

        // Tear down monitors for removed sources.
        for removed in old_ids.difference(&new_ids) {
            self.stop_monitor(removed).await;
        }

        // Start monitors for newly discovered Auto sources.
        for added in new_ids.difference(&old_ids) {
            if let Some(src) = self.registry.get(added) {
                if src.connection_mode() == ConnectionMode::Auto {
                    if let Err(e) = self.start_monitor(added) {
                        warn!(source = %added, error = %e, "failed to start monitor");
                    }
                }
            }
        }

        Ok(())
    }

    // ── Manual connect / disconnect ───────────────────────────────────────────

    /// Start the monitor pipeline for a Manual source.
    pub fn connect(&mut self, source_id: &str) -> Result<()> {
        if self.monitors.contains_key(source_id) {
            return Ok(()); // already connected
        }
        self.start_monitor(source_id)
    }

    /// Stop the monitor pipeline (and any active recording) for a source.
    pub async fn disconnect(&mut self, source_id: &str) {
        self.stop_monitor(source_id).await;
    }

    // ── Thumbnail / audio access ──────────────────────────────────────────────

    pub fn thumbnail_bytes(&self, source_id: &str) -> Option<Vec<u8>> {
        self.monitors.get(source_id)?.pipeline.thumbnail.latest()
    }

    pub fn audio_levels(&self, source_id: &str) -> Option<Vec<ChannelLevelDto>> {
        let state = self
            .monitors
            .get(source_id)?
            .pipeline
            .audio_meter
            .latest()?;
        Some(
            state
                .channels
                .iter()
                .map(|c| ChannelLevelDto {
                    peak_db: c.peak_db,
                    rms_db: c.rms_db,
                })
                .collect(),
        )
    }

    /// Iterate over all monitored source IDs and their audio levels.
    pub fn all_audio_levels(&self) -> Vec<(String, Vec<ChannelLevelDto>)> {
        self.monitors
            .keys()
            .filter_map(|id| self.audio_levels(id).map(|lvl| (id.clone(), lvl)))
            .collect()
    }

    // ── Recording ─────────────────────────────────────────────────────────────

    /// Start a multi-leg recording session.
    /// `legs` is an ordered list of `(output_path, profile)` pairs, one per output leg.
    pub async fn start_recording(
        &mut self,
        source_id: &str,
        preset_id: &str,
        legs: &[(String, RecordingProfile)],
    ) -> Result<RecordingSessionDto> {
        if self
            .sessions
            .values()
            .any(|s| s.source_id == source_id && s.dto.status == "active")
        {
            bail!("source {source_id} already has an active recording");
        }

        let monitor = self
            .monitors
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("no monitor running for source {source_id}"))?;

        let leg_refs: Vec<(&Path, &RecordingProfile)> = legs
            .iter()
            .map(|(p, prof)| (Path::new(p.as_str()), prof))
            .collect();

        let branches = monitor.pipeline.attach_recording_legs(&leg_refs).await?;

        let output_paths: Vec<String> = legs.iter().map(|(p, _)| p.clone()).collect();

        let dto = RecordingSessionDto {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.to_string(),
            preset_id: preset_id.to_string(),
            started_at: Utc::now().to_rfc3339(),
            stopped_at: None,
            output_paths,
            status: "active".to_string(),
            error_message: None,
        };

        info!(id = %dto.id, source = source_id, legs = legs.len(), "recording started");
        self.sessions.insert(
            dto.id.clone(),
            ActiveSession {
                source_id: source_id.to_string(),
                branches,
                dto: dto.clone(),
            },
        );

        Ok(dto)
    }

    pub async fn stop_recording(&mut self, session_id: &str) -> Result<RecordingSessionDto> {
        let session = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not active"))?;

        let mut dto = session.dto;

        let monitor = self.monitors.get(&session.source_id);
        // Detach every leg concurrently: unlinking a branch's tee pad is what
        // actually stops it recording, and detach_recording() blocks on that
        // leg's EOS before returning. Doing this sequentially left later legs
        // linked (and still recording) for the entire time earlier legs spent
        // finalizing.
        let last_err: Option<anyhow::Error> = if let Some(m) = monitor {
            let pipeline = Arc::clone(&m.pipeline);
            join_all(session.branches.into_iter().map(|branch| {
                let pipeline = Arc::clone(&pipeline);
                async move { pipeline.detach_recording(branch, 10).await }
            }))
            .await
            .into_iter()
            .find_map(|r| r.err())
        } else {
            // Monitor was torn down — branch elements are already NULL, nothing to do.
            None
        };

        match last_err {
            None => {
                dto.status = "stopped".to_string();
                dto.stopped_at = Some(Utc::now().to_rfc3339());
                info!(id = %dto.id, "recording stopped");
            }
            Some(e) => {
                dto.status = "error".to_string();
                dto.stopped_at = Some(Utc::now().to_rfc3339());
                dto.error_message = Some(e.to_string());
            }
        }

        Ok(dto)
    }

    pub fn active_sessions(&self) -> Vec<&RecordingSessionDto> {
        self.sessions.values().map(|s| &s.dto).collect()
    }

    // ── Monitor config ────────────────────────────────────────────────────────

    pub fn monitor_config(&self) -> &MonitorConfig {
        &self.config
    }

    /// Update the in-memory config without restarting any pipelines.
    /// Used when a peer node receives a fan-out settings push from the
    /// aggregator — the WS notification rate adjusts immediately; pipeline
    /// fps/resolution/interval take effect on the next process start.
    pub fn set_config(&mut self, config: MonitorConfig) {
        self.config = config;
    }

    /// Apply a new global monitor config to all running monitors without
    /// restarting any pipelines. GStreamer re-negotiates the affected branches
    /// in place, so audio and thumbnails remain uninterrupted.
    pub fn apply_monitor_config(&mut self, config: MonitorConfig) {
        self.config = config;
        for monitor in self.monitors.values() {
            monitor.pipeline.reconfigure(&self.config);
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn start_monitor(&mut self, source_id: &str) -> Result<()> {
        let source = self
            .registry
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("source {source_id} not found"))?;

        let pipeline = Arc::new(MonitorPipeline::new(source, &self.config)?);
        self.monitors
            .insert(source_id.to_string(), ActiveMonitor { pipeline });
        info!(source = source_id, "monitor started");
        Ok(())
    }

    /// Remove the session from the active map and hand back what's needed to
    /// detach its branches. The caller must run that detach work — and the
    /// persist/notify that follows it — on a task spawned independently of
    /// the triggering HTTP request (e.g. via `tokio::spawn`), NOT inline in
    /// the request handler. If the handler awaits it inline, dropping the
    /// handler's future (which happens if the client disconnects — a page
    /// refresh, a retried request) silently cancels whatever branch detach
    /// was still in flight, orphaning that branch: still linked to the tee,
    /// still recording, and — since the session was already removed here —
    /// unreachable by any future stop call.
    ///
    /// A second call for the same `session_id` while the first is still
    /// running returns `StopOutcome::Join` with a receiver for the same
    /// result, instead of falling through to a DB-only path that would
    /// report "stopped" without the pipeline actually having stopped.
    pub fn begin_stop_recording(&mut self, session_id: &str) -> StopOutcome {
        if let Some(session) = self.sessions.remove(session_id) {
            let pending = self
                .monitors
                .get(&session.source_id)
                .map(|m| (Arc::clone(&m.pipeline), session.branches));
            let (tx, rx) = watch::channel(None);
            self.stopping.insert(session_id.to_string(), rx);
            StopOutcome::Start { dto: session.dto, pending, tx }
        } else if let Some(rx) = self.stopping.get(session_id) {
            StopOutcome::Join(rx.clone())
        } else {
            StopOutcome::NotFound
        }
    }

    /// Clear the "stopping" marker once the detach task has reported its
    /// final result through the `tx` handed out by `begin_stop_recording`.
    pub fn finish_stop(&mut self, session_id: &str) {
        self.stopping.remove(session_id);
    }

    async fn stop_monitor(&mut self, source_id: &str) {
        // Stop any active recordings on this monitor first.
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.source_id == source_id)
            .map(|(id, _)| id.clone())
            .collect();

        for session_id in session_ids {
            if let Err(e) = self.stop_recording(&session_id).await {
                warn!(session = %session_id, error = %e, "error stopping recording during monitor teardown");
            }
        }

        if let Some(m) = self.monitors.remove(source_id) {
            if let Err(e) = m.pipeline.stop() {
                warn!(source = source_id, error = %e, "error stopping monitor pipeline");
            } else {
                info!(source = source_id, "monitor stopped");
            }
        }
    }
}
