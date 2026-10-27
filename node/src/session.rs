//! Session and monitor lifecycle work that waits on GStreamer, so always runs
//! on spawned tasks, off the source manager's lock: finishing recordings,
//! running the [`Work`] the manager plans, reporting failed legs, restarting
//! failed monitors.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::api::types::{RecordingStatus, WsEvent};
use crate::db;
use crate::sources::manager::{LegFailure, StopJob, StopOutcome, Teardown, Work};
use crate::state::AppState;

/// How long a leg may take to drain to EOS and close its file.
const EOS_TIMEOUT: Duration = Duration::from_secs(10);

/// Failed-monitor check interval; doubles while a source keeps failing, up
/// to the max, and resets once all are healthy.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_MAX_INTERVAL: Duration = Duration::from_secs(30);

/// Drain a stopping session's legs, persist and broadcast the outcome, and
/// answer every request waiting on the stop. `source_error`: the session ends
/// because its monitor failed.
pub async fn run_stop(state: Arc<AppState>, job: StopJob, source_error: Option<String>) {
    let StopJob { legs, mut dto, tx } = job;

    // Concurrently, so all legs stop at the same moment.
    let results = join_all(legs.into_iter().map(|leg| leg.stop(EOS_TIMEOUT))).await;

    dto.stopped_at = Some(chrono::Utc::now().to_rfc3339());
    // A failed source is the root cause of anything its legs report.
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

/// What the node is doing that an update or a quit would interrupt.
pub struct Activity {
    pub recordings: usize,
    /// Channels playing a clip.
    pub playing: usize,
}

pub async fn activity(state: &AppState) -> Activity {
    let mgr = state.source_manager.read().await;
    let playing = mgr
        .sources()
        .iter()
        .filter_map(|s| s.playout())
        .filter(|p| p.status().state == crate::api::types::TransportState::Playing)
        .count();
    Activity {
        recordings: mgr.active_sessions().len(),
        playing,
    }
}

pub async fn stop_all_recordings(state: &Arc<AppState>) {
    let ids: Vec<String> = state
        .source_manager
        .read()
        .await
        .active_sessions()
        .into_iter()
        .map(|s| s.id)
        .collect();
    join_all(ids.into_iter().map(|id| async move {
        let outcome = state.source_manager.write().await.begin_stop_recording(&id);
        match outcome {
            StopOutcome::Start(job) => run_stop(Arc::clone(state), *job, None).await,
            StopOutcome::Join(mut rx) => {
                while rx.borrow_and_update().is_none() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            }
            StopOutcome::NotFound => {}
        }
    }))
    .await;
}

/// Quit: finish the recordings, so their files are whole, then exit with
/// `code` (0 leaves the service stopped; anything else restarts it).
pub async fn quit(state: &Arc<AppState>, code: i32) -> ! {
    stop_all_recordings(state).await;
    info!(code, "quitting");
    std::process::exit(code)
}

/// Quit cleanly on Ctrl+C, when the service manager stops the node, or (on
/// Windows) when the user signs out or the machine shuts down.
pub fn spawn_quit_on_signal(state: Arc<AppState>) {
    #[cfg(windows)]
    let mut asked = {
        let (tx, rx) = mpsc::unbounded_channel();
        crate::service::windows_quit::watch(move || {
            let _ = tx.send(());
        });
        rx
    };
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let (Ok(mut term), Ok(mut int)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
            ) else {
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        #[cfg(windows)]
        {
            use tokio::signal::windows::{
                ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown,
            };
            // For a node run in a console. Windows doesn't send a process
            // with windows the sign-out and shutdown events, so those (and
            // `service stop`) come through `windows_quit`.
            match (
                ctrl_c(),
                ctrl_break(),
                ctrl_close(),
                ctrl_logoff(),
                ctrl_shutdown(),
            ) {
                (Ok(mut c), Ok(mut brk), Ok(mut close), Ok(mut logoff), Ok(mut shutdown)) => {
                    tokio::select! {
                        _ = c.recv() => {}
                        _ = brk.recv() => {}
                        _ = close.recv() => {}
                        _ = logoff.recv() => {}
                        _ = shutdown.recv() => {}
                        Some(()) = asked.recv() => {}
                    }
                }
                _ => {
                    if asked.recv().await.is_none() {
                        return;
                    }
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        info!("stopping: finishing recordings first");
        quit(&state, 0).await;
    });
}

/// Run each teardown on its own task: finish the monitor's recordings, then
/// stop its pipeline, freeing the source bin for a new monitor.
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

/// One rebuild at a time: one with an older config list would otherwise bring
/// back a source another request just deleted, and two would each start a
/// changed source.
static REBUILD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Rebuild the source list, tearing down whatever went away, changed or
/// failed.
pub async fn rebuild_sources(state: &Arc<AppState>) -> anyhow::Result<()> {
    let state = Arc::clone(state);
    // Spawned so a dropped request can't leave the work started but not applied.
    tokio::spawn(async move {
        let _rebuild = REBUILD.lock().await;
        let configs = db::configured_sources_list(&state.db).await?;
        let (teardowns, work) = state.source_manager.write().await.scan(&configs);
        spawn_teardowns(&state, teardowns);
        run_work(&state, work).await
    })
    .await?
}

/// Run what the source manager planned off its lock, then apply the result
/// under a second, short one.
pub async fn run_work(state: &Arc<AppState>, work: Work) -> anyhow::Result<()> {
    if work.is_empty() {
        return Ok(());
    }
    let state = Arc::clone(state);
    // Spawned so a dropped request can't leave the work started but not applied.
    tokio::spawn(async move {
        let done = tokio::task::spawn_blocking(move || work.run()).await?;
        let rejected = state.source_manager.write().await.apply(done);
        if !rejected.is_empty() {
            tokio::task::spawn_blocking(move || drop(rejected)).await?;
        }
        Ok(())
    })
    .await?
}

/// Rescan whenever a source lacks a healthy monitor. This also ends a failed
/// source's recordings, so files are finalised rather than waiting for frames
/// that never come.
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

/// Move monitors onto the node clock when it changes: idle ones now, busy
/// ones by the recovery loop once idle.
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

/// Report legs that fail mid-recording as they happen. Other legs keep
/// recording; once all have failed the session is stopped as `error`.
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

/// Persist a running session's file list whenever a leg opens a new file, so
/// it survives a crash.
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
