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
use crate::sources::manager::{LegFailure, StopJob, StopOutcome, Teardown};
use crate::state::AppState;

/// How long a leg may take to drain to EOS and close its file.
const EOS_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to check for failed monitors to restart. Each rescan that
/// leaves a source still failing doubles the wait, up to the max; the wait
/// resets once every source is healthy.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_MAX_INTERVAL: Duration = Duration::from_secs(30);

/// Drain a stopping session's legs, persist and broadcast the outcome, and
/// publish it to every request waiting on the stop. `source_error` is set when
/// the session is ending because its source's monitor failed.
pub async fn run_stop(state: Arc<AppState>, job: StopJob, source_error: Option<String>) {
    let StopJob { legs, mut dto, tx } = job;

    // Stop every leg concurrently, so all of them stop recording at the same
    // moment rather than each waiting for the previous one to finalize.
    let results = join_all(legs.into_iter().map(|leg| leg.stop(EOS_TIMEOUT))).await;

    dto.stopped_at = Some(chrono::Utc::now().to_rfc3339());
    // A failed source is the root cause of anything its legs report. Each
    // leg's error names its output path, so all of them are kept.
    let leg_errors: Vec<String> = results
        .into_iter()
        .filter_map(Result::err)
        .map(|e| e.to_string())
        .collect();
    let error = source_error
        .map(|e| format!("source failed: {e}"))
        .or_else(|| (!leg_errors.is_empty()).then(|| leg_errors.join("; ")));
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

    if let Err(e) = db::session_update_stop(&state.db, &dto).await {
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
    for Teardown {
        source_id,
        pipeline,
        stops,
    } in teardowns
    {
        let state = Arc::clone(state);
        tokio::spawn(async move {
            let source_error = pipeline.error();
            join_all(
                stops
                    .into_iter()
                    .map(|job| run_stop(Arc::clone(&state), job, source_error.clone())),
            )
            .await;
            match pipeline.stop() {
                Ok(()) => info!(source = %source_id, "monitor stopped"),
                Err(e) => warn!(source = %source_id, error = %e, "error stopping monitor pipeline"),
            }
        });
    }
}

/// Rescan sources: rebuild the list from the configured sources and the NDI
/// sources on the network, tearing down whatever went away, changed or failed.
pub async fn rebuild_sources(state: &Arc<AppState>) -> anyhow::Result<()> {
    let configs = db::configured_sources_list(&state.db).await?;
    let teardowns = state.source_manager.write().await.scan(&configs);
    spawn_teardowns(state, teardowns);
    Ok(())
}

/// Restart failed monitors (e.g. an NDI sender that dropped out), or ones that
/// failed to start, by rescanning whenever a source lacks a healthy monitor. A
/// rescan ends a failed source's recordings, so their files are finalized
/// rather than left waiting for frames that never come; a sender that has left
/// the network is removed instead.
pub fn spawn_monitor_recovery(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut wait = RECOVERY_INTERVAL;
        loop {
            tokio::time::sleep(wait).await;
            if !state.source_manager.read().await.needs_rescan() {
                wait = RECOVERY_INTERVAL;
                continue;
            }
            wait = (wait * 2).min(RECOVERY_MAX_INTERVAL);
            info!(next_check = ?wait, "restarting failed monitors");
            if let Err(e) = rebuild_sources(&state).await {
                error!(error = %e, "rescan for failed monitors");
            }
        }
    });
}

/// Move monitors onto the node clock when it changes. Idle ones are rebuilt
/// now; busy ones (recording, playing out) are left for the monitor recovery
/// loop to rebuild once they're idle.
pub fn spawn_clock_follower(state: Arc<AppState>) {
    let mut changed = state.clock.subscribe();
    tokio::spawn(async move {
        while changed.changed().await.is_ok() {
            let rescan = {
                let mut mgr = state.source_manager.write().await;
                mgr.set_clock(state.clock.current());
                mgr.needs_rescan()
            };
            if rescan {
                info!("moving idle monitors to the new clock");
                if let Err(e) = rebuild_sources(&state).await {
                    error!(error = %e, "rescan for the new clock");
                }
            }
            state.emit(&WsEvent::NodeUpdated);
        }
    });
}

/// Report recording legs that fail mid-recording as they happen, rather than
/// only when the session is stopped. The session's other legs keep recording;
/// once every leg has failed nothing is being recorded, so the session is
/// stopped (and finishes as `error`).
pub fn spawn_leg_failure_reporter(
    state: Arc<AppState>,
    mut failures: mpsc::UnboundedReceiver<LegFailure>,
) {
    tokio::spawn(async move {
        while let Some(failure) = failures.recv().await {
            let session_id = failure.session_id.clone();
            let (noted, stop) = {
                let mut mgr = state.source_manager.write().await;
                let noted = mgr.note_leg_failure(&failure);
                let stop = match &noted {
                    Some(n) if n.all_failed => Some(mgr.begin_stop_recording(&session_id)),
                    _ => None,
                };
                (noted, stop)
            };
            let Some(noted) = noted else { continue };
            warn!(session = %session_id, path = ?failure.path, error = %failure.error, "recording leg failed");
            state.emit(&WsEvent::RecordingLegFailed {
                session_id: session_id.clone(),
                source_id: noted.source_id,
                error: noted.error,
            });
            if let Some(StopOutcome::Start(job)) = stop {
                warn!(session = %session_id, "every output failed, stopping recording");
                tokio::spawn(run_stop(Arc::clone(&state), *job, None));
            }
        }
    });
}

/// Save a running session's file list whenever a leg opens a new file (a
/// split), so the Recordings tab lists every file even after a crash. Stops
/// save the final list themselves.
pub fn spawn_leg_file_recorder(
    state: Arc<AppState>,
    mut sessions: mpsc::UnboundedReceiver<String>,
) {
    tokio::spawn(async move {
        while let Some(session_id) = sessions.recv().await {
            let files = state
                .source_manager
                .read()
                .await
                .active_sessions()
                .into_iter()
                .find(|s| s.id == session_id)
                .map(|s| s.files);
            let Some(files) = files else { continue };
            if let Err(e) = db::session_update_files(&state.db, &session_id, &files).await {
                error!(session = %session_id, error = %e, "persist session files");
            }
        }
    });
}
