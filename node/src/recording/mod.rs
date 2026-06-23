use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Result};
use chrono::Utc;
use tracing::{error, info};
use uuid::Uuid;

use crate::api::types::RecordingSessionDto;
use crate::db::{self, SessionRow};
use crate::pipeline::{profile::RecordingProfile, Pipeline};
use crate::sources::registry::SourceRegistry;

struct ActiveSession {
    pipeline: Pipeline,
    session: RecordingSessionDto,
}

pub struct RecordingManager {
    active: HashMap<String, ActiveSession>,
}

impl RecordingManager {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    /// Start a recording for `source_id` using the given profile.
    ///
    /// Returns the new session DTO.  The caller is responsible for persisting
    /// the session to the DB and broadcasting the `recording.started` event.
    pub fn start(
        &mut self,
        sources: &SourceRegistry,
        source_id: &str,
        profile: &RecordingProfile,
        primary_path: &Path,
    ) -> Result<RecordingSessionDto> {
        if self.active.values().any(|s| s.session.source_id == source_id && s.session.status == "active") {
            bail!("source {source_id} already has an active recording");
        }

        let source = sources
            .get(source_id)
            .ok_or_else(|| anyhow::anyhow!("source {source_id} not found"))?;

        let pipeline = Pipeline::new(source, primary_path, profile, None)?;
        pipeline.start()?;

        let session = RecordingSessionDto {
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

        info!(id = %session.id, source = source_id, "recording started");
        self.active.insert(session.id.clone(), ActiveSession { pipeline, session: session.clone() });

        Ok(session)
    }

    /// Stop a recording session by ID.  Returns the updated session DTO.
    pub async fn stop(&mut self, session_id: &str) -> Result<RecordingSessionDto> {
        let active = self
            .active
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not active"))?;

        let mut session = active.session;

        match active.pipeline.stop(10).await {
            Ok(()) => {
                session.status = "stopped".to_string();
                session.stopped_at = Some(Utc::now().to_rfc3339());
                info!(id = %session.id, "recording stopped");
            }
            Err(e) => {
                error!(id = %session.id, error = %e, "pipeline stop error");
                session.status = "error".to_string();
                session.stopped_at = Some(Utc::now().to_rfc3339());
                session.error_message = Some(e.to_string());
            }
        }

        Ok(session)
    }

    pub fn active_sessions(&self) -> Vec<&RecordingSessionDto> {
        self.active.values().map(|s| &s.session).collect()
    }

    pub fn is_active(&self, session_id: &str) -> bool {
        self.active.contains_key(session_id)
    }

    /// Snapshot audio levels for a source that has an active session.
    pub fn audio_levels(
        &self,
        source_id: &str,
    ) -> Option<Vec<crate::api::types::ChannelLevelDto>> {
        let active = self
            .active
            .values()
            .find(|s| s.session.source_id == source_id)?;

        let state = active.pipeline.audio_meter.latest()?;
        Some(
            state
                .channels
                .iter()
                .map(|c| crate::api::types::ChannelLevelDto {
                    peak_db: c.peak_db,
                    rms_db: c.rms_db,
                })
                .collect(),
        )
    }

    /// Latest thumbnail bytes for a source that has an active session.
    pub fn thumbnail_bytes(&self, source_id: &str) -> Option<Vec<u8>> {
        self.active
            .values()
            .find(|s| s.session.source_id == source_id)?
            .pipeline
            .thumbnail
            .latest()
    }
}

impl Default for RecordingManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Persistence helpers (called by route handlers) ────────────────────────────

pub async fn persist_start(
    pool: &sqlx::SqlitePool,
    session: &RecordingSessionDto,
) -> Result<()> {
    db::session_insert(
        pool,
        &SessionRow {
            id: session.id.clone(),
            source_id: session.source_id.clone(),
            preset_id: session.preset_id.clone(),
            started_at: session.started_at.clone(),
            stopped_at: session.stopped_at.clone(),
            primary_path: session.primary_path.clone(),
            secondary_path: session.secondary_path.clone(),
            redundant_path: session.redundant_path.clone(),
            status: session.status.clone(),
            error_message: session.error_message.clone(),
        },
    )
    .await
}

pub async fn persist_stop(pool: &sqlx::SqlitePool, session: &RecordingSessionDto) -> Result<()> {
    db::session_update_stop(
        pool,
        &session.id,
        session.stopped_at.as_deref().unwrap_or(""),
        &session.status,
        session.error_message.as_deref(),
    )
    .await
}
