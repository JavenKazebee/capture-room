//! Finishing recordings: detaching branches, persisting the outcome and
//! broadcasting it. Always runs on spawned tasks — see
//! [`SourceManager::begin_stop_recording`](crate::sources::manager::SourceManager::begin_stop_recording).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::future::join_all;
use tracing::{error, info, warn};

use crate::api::types::{RecordingSessionDto, WsEvent};
use crate::db::{self, SessionRow};
use crate::sources::manager::{StopJob, Teardown};
use crate::state::AppState;

/// How long a leg may take to drain to EOS and close its file.
const EOS_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Detach a stopping session's branches, persist and broadcast the outcome,
/// and publish it to every request waiting on the stop.
pub async fn run_stop(state: Arc<AppState>, job: StopJob) {
    let StopJob { pipeline, branches, mut dto, tx } = job;

    // Detach every leg concurrently: unlinking a branch's tee pad is what
    // actually stops it recording, and detach_recording() waits for that
    // leg's EOS. Done sequentially, later legs would keep recording while
    // earlier ones finalize.
    let results =
        join_all(branches.into_iter().map(|b| pipeline.detach_recording(b, EOS_TIMEOUT))).await;

    dto.stopped_at = Some(chrono::Utc::now().to_rfc3339());
    match results.into_iter().find_map(Result::err) {
        None => {
            dto.status = "stopped".to_string();
            info!(id = %dto.id, "recording stopped");
        }
        Some(e) => {
            dto.status = "error".to_string();
            dto.error_message = Some(e.to_string());
            warn!(id = %dto.id, error = %e, "recording stopped with error");
        }
    }

    if let Err(e) = db::session_update_stop(
        &state.db,
        &dto.id,
        dto.stopped_at.as_deref().unwrap_or_default(),
        &dto.status,
        dto.error_message.as_deref(),
    )
    .await
    {
        error!(error = %e, "persist session stop");
    }

    state.emit(&match &dto.error_message {
        Some(error) => WsEvent::RecordingError {
            session_id: dto.id.clone(),
            source_id: dto.source_id.clone(),
            error: error.clone(),
        },
        None => WsEvent::RecordingStopped {
            session_id: dto.id.clone(),
            source_id: dto.source_id.clone(),
        },
    });

    let session_id = dto.id.clone();
    let _ = tx.send(Some(dto));
    state.source_manager.write().await.finish_stop(&session_id);
}

/// Run each teardown on its own task: finish the monitor's recordings, then
/// stop its pipeline (which also frees the source bin for a new monitor).
pub fn spawn_teardowns(state: &Arc<AppState>, teardowns: Vec<Teardown>) {
    for Teardown { source_id, pipeline, stops } in teardowns {
        let state = Arc::clone(state);
        tokio::spawn(async move {
            join_all(stops.into_iter().map(|job| run_stop(Arc::clone(&state), job))).await;
            match pipeline.stop() {
                Ok(()) => info!(source = %source_id, "monitor stopped"),
                Err(e) => warn!(source = %source_id, error = %e, "error stopping monitor pipeline"),
            }
        });
    }
}
