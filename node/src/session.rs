//! Session and monitor lifecycle work that has to wait on GStreamer or run in
//! the background: finishing recordings (detaching legs, persisting the
//! outcome and broadcasting it), reporting legs that fail mid-recording, and
//! restarting failed monitors. Always runs on spawned tasks — see
//! [`SourceManager::begin_stop_recording`](crate::sources::manager::SourceManager::begin_stop_recording).

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::api::types::{RecordingStatus, WsEvent};
use crate::db;
use crate::sources::manager::{LegFailure, StopJob, Teardown};
use crate::state::AppState;

/// How long a leg may take to drain to EOS and close its file.
const EOS_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to check for failed monitors to restart.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(5);

/// Drain a stopping session's legs, persist and broadcast the outcome, and
/// publish it to every request waiting on the stop. `source_error` is set when
/// the session is ending because its source's monitor failed.
pub async fn run_stop(state: Arc<AppState>, job: StopJob, source_error: Option<String>) {
    let StopJob { legs, mut dto, tx } = job;

    // Stop every leg concurrently, so all of them stop recording at the same
    // moment rather than each waiting for the previous one to finalize.
    let results = join_all(legs.into_iter().map(|leg| leg.stop(EOS_TIMEOUT))).await;

    dto.stopped_at = Some(chrono::Utc::now().to_rfc3339());
    // A failed source is the root cause of anything its legs report.
    let error = source_error
        .map(|e| format!("source failed: {e}"))
        .or_else(|| results.into_iter().find_map(Result::err).map(|e| e.to_string()));
    match error {
        None => {
            dto.status = RecordingStatus::Stopped;
            info!(id = %dto.id, "recording stopped");
        }
        Some(e) => {
            warn!(id = %dto.id, error = %e, "recording stopped with error");
            dto.status = RecordingStatus::Error;
            dto.error_message = Some(e);
        }
    }

    if let Err(e) = db::session_update_stop(
        &state.db,
        &dto.id,
        dto.stopped_at.as_deref().unwrap_or_default(),
        dto.status,
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
            let source_error = pipeline.error();
            join_all(stops.into_iter().map(|job| run_stop(Arc::clone(&state), job, source_error.clone()))).await;
            match pipeline.stop() {
                Ok(()) => info!(source = %source_id, "monitor stopped"),
                Err(e) => warn!(source = %source_id, error = %e, "error stopping monitor pipeline"),
            }
        });
    }
}

/// Rescan sources: rebuild the list from the stored test configs and the NDI
/// sources on the network, tearing down whatever went away, changed or failed.
pub async fn rebuild_sources(state: &Arc<AppState>) -> anyhow::Result<()> {
    let configs = db::test_sources_list(&state.db).await?;
    let teardowns = state.source_manager.write().await.scan(&configs);
    spawn_teardowns(state, teardowns);
    Ok(())
}

/// Restart failed monitors (e.g. an NDI sender that dropped out) by rescanning
/// whenever one has failed. A rescan ends the failed source's recordings, so
/// their files are finalized rather than left waiting for frames that never
/// come; a sender that has left the network is removed instead.
pub fn spawn_monitor_recovery(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RECOVERY_INTERVAL);
        loop {
            interval.tick().await;
            if !state.source_manager.read().await.has_failed_monitor() {
                continue;
            }
            info!("restarting failed monitors");
            if let Err(e) = rebuild_sources(&state).await {
                error!(error = %e, "rescan for failed monitors");
            }
        }
    });
}

/// Report recording legs that fail mid-recording as they happen, rather than
/// only when the session is stopped. The session's other legs keep recording.
pub fn spawn_leg_failure_reporter(state: Arc<AppState>, mut failures: mpsc::UnboundedReceiver<LegFailure>) {
    tokio::spawn(async move {
        while let Some(failure) = failures.recv().await {
            let noted = state.source_manager.write().await.note_leg_failure(&failure);
            let Some((source_id, error)) = noted else { continue };
            warn!(session = %failure.session_id, path = ?failure.path, error = %failure.error, "recording leg failed");
            state.emit(&WsEvent::RecordingLegFailed { session_id: failure.session_id, source_id, error });
        }
    });
}
