use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::Utc;
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
    branch: RecordingBranch,
    pub dto: RecordingSessionDto,
}

// ── SourceManager ─────────────────────────────────────────────────────────────

/// Owns the source registry, per-source monitor pipelines, and active recording
/// sessions. This is the single point of truth for all capture state on a node.
pub struct SourceManager {
    pub config: MonitorConfig,
    registry: SourceRegistry,
    monitors: HashMap<String, ActiveMonitor>,
    sessions: HashMap<String, ActiveSession>, // session_id → session
    ndi_monitor: NdiMonitor,
}

impl SourceManager {
    pub fn new(config: MonitorConfig, ndi_monitor: NdiMonitor) -> Self {
        Self {
            config,
            registry: SourceRegistry::new(),
            monitors: HashMap::new(),
            sessions: HashMap::new(),
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

    pub async fn start_recording(
        &mut self,
        source_id: &str,
        profile: &RecordingProfile,
        primary_path: &Path,
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

        let branch = monitor.pipeline.attach_recording(primary_path, profile).await?;

        let dto = RecordingSessionDto {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.to_string(),
            preset_id: profile.id.clone(),
            started_at: Utc::now().to_rfc3339(),
            stopped_at: None,
            primary_path: primary_path.to_string_lossy().into_owned(),
            secondary_path: None,
            redundant_path: None,
            status: "active".to_string(),
            error_message: None,
        };

        info!(id = %dto.id, source = source_id, "recording started");
        self.sessions.insert(
            dto.id.clone(),
            ActiveSession {
                source_id: source_id.to_string(),
                branch,
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
        let result = if let Some(m) = monitor {
            m.pipeline.detach_recording(session.branch, 10).await
        } else {
            // Monitor was torn down while recording — branch elements are already
            // in NULL state from stop_monitor, so nothing more to do.
            Ok(())
        };

        match result {
            Ok(()) => {
                dto.status = "stopped".to_string();
                dto.stopped_at = Some(Utc::now().to_rfc3339());
                info!(id = %dto.id, "recording stopped");
            }
            Err(e) => {
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

    pub fn is_active(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
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

    /// Remove the session from the active map and return the info needed to
    /// await EOS. The caller must call `pipeline.detach_recording(branch, 10)`
    /// outside of any write lock so the WS emitter stays unblocked.
    pub fn begin_stop_recording(
        &mut self,
        session_id: &str,
    ) -> Result<(RecordingSessionDto, Option<(Arc<MonitorPipeline>, RecordingBranch)>)> {
        let session = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not active"))?;
        let pending = self
            .monitors
            .get(&session.source_id)
            .map(|m| (Arc::clone(&m.pipeline), session.branch));
        Ok((session.dto, pending))
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
