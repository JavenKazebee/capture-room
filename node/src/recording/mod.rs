use anyhow::Result;

use crate::api::types::RecordingSessionDto;
use crate::db::{self, SessionRow};

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

pub async fn persist_stop(
    pool: &sqlx::SqlitePool,
    session: &RecordingSessionDto,
) -> Result<()> {
    db::session_update_stop(
        pool,
        &session.id,
        session.stopped_at.as_deref().unwrap_or(""),
        &session.status,
        session.error_message.as_deref(),
    )
    .await
}
