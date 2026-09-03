use anyhow::Result;

use crate::api::types::RecordingSessionDto;
use crate::db::{self, SessionRow};

pub async fn persist_start(
    pool: &sqlx::SqlitePool,
    session: &RecordingSessionDto,
) -> Result<()> {
    let output_paths_json = serde_json::to_string(&session.output_paths)
        .unwrap_or_else(|_| "[]".to_string());

    db::session_insert(
        pool,
        &SessionRow {
            id: session.id.clone(),
            source_id: session.source_id.clone(),
            preset_id: session.preset_id.clone(),
            started_at: session.started_at.clone(),
            stopped_at: session.stopped_at.clone(),
            output_paths: output_paths_json,
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
