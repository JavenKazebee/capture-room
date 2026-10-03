//! The node API: everything this machine has — sources, storage, recordings,
//! thumbnails, settings. Local only: it never forwards and never knows about
//! other nodes. Mounted at `/api/v1/node`; controllers reach a peer's copy
//! through `/api/v1/nodes/{id}/…`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path as AxumPath, State},
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use tracing::error;

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    NodeSettingsDto, NodeStatus, RecordingSessionDto, RecordingStatus, SourceDto, StartRecordingRequest,
    StorageVolumeDto, TestSourceConfigDto, TestSourceRequest, UpdateNodeSettingsRequest, WsEvent,
};
use crate::db;
use crate::pipeline::profile::RecordingProfile;
use crate::session;
use crate::sources::manager::{SourceManager, StopOutcome, StopResult};
use crate::sources::InputSource;
use crate::state::AppState;
use crate::ws;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/status", get(get_status))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/storage", get(get_storage))
        // Sources — static paths before dynamic {id}
        .route("/sources", get(get_sources))
        .route("/sources/scan", post(post_scan))
        .route("/sources/{id}", get(get_source))
        .route("/test-sources", get(get_test_configs).post(post_test_config))
        .route("/test-sources/{id}", axum::routing::put(put_test_config).delete(delete_test_config))
        .route("/recordings", get(get_recordings).post(post_recording))
        .route("/recordings/{id}", get(get_recording))
        .route("/recordings/{id}/stop", post(post_stop_recording))
        .route("/thumbnails/{source_id}", get(get_thumbnail))
        .route("/ws", get(ws_handler))
}

// ── /status ───────────────────────────────────────────────────────────────────

async fn get_status(State(state): State<Arc<AppState>>) -> Json<NodeStatus> {
    Json(NodeStatus {
        id: state.node_id.clone(),
        name: state.node_name(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        is_controller: state.is_controller().await,
    })
}

// ── /settings ─────────────────────────────────────────────────────────────────

async fn settings_dto(state: &AppState) -> NodeSettingsDto {
    NodeSettingsDto {
        node_id: state.node_id.clone(),
        node_name: state.node_name(),
        is_controller: state.is_controller().await,
        monitor: *state.source_manager.read().await.monitor_config(),
    }
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<NodeSettingsDto> {
    Json(settings_dto(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateNodeSettingsRequest>,
) -> ApiResult<Json<NodeSettingsDto>> {
    if let Some(name) = req.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        db::config_set(&state.db, "name", &name).await?;
        *state.node_name.write().unwrap() = name;
    }
    if let Some(monitor) = req.monitor {
        let monitor = monitor.clamped();
        db::monitor_settings_set(&state.db, &monitor).await?;
        state.source_manager.write().await.apply_monitor_config(monitor);
    }
    Ok(Json(settings_dto(&state).await))
}

// ── /storage ──────────────────────────────────────────────────────────────────

async fn get_storage() -> ApiResult<Json<Vec<StorageVolumeDto>>> {
    Ok(Json(tokio::task::spawn_blocking(crate::storage::list_volumes).await?))
}

// ── /sources ──────────────────────────────────────────────────────────────────

fn sources_list(mgr: &SourceManager) -> Vec<SourceDto> {
    mgr.sources().iter().map(|s| source_to_dto(mgr, s.as_ref())).collect()
}

async fn get_sources(State(state): State<Arc<AppState>>) -> Json<Vec<SourceDto>> {
    Json(sources_list(&*state.source_manager.read().await))
}

async fn get_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<SourceDto>> {
    let mgr = state.source_manager.read().await;
    let source = mgr.get_source(&id).ok_or(ApiError::NotFound("source not found"))?;
    Ok(Json(source_to_dto(&mgr, source)))
}

async fn post_scan(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<SourceDto>>> {
    session::rebuild_sources(&state).await?;
    Ok(Json(sources_list(&*state.source_manager.read().await)))
}

// ── /test-sources ─────────────────────────────────────────────────────────────

async fn get_test_configs(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<TestSourceConfigDto>>> {
    Ok(Json(db::test_sources_list(&state.db).await?))
}

async fn post_test_config(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TestSourceRequest>,
) -> ApiResult<(StatusCode, Json<TestSourceConfigDto>)> {
    let config = TestSourceConfigDto {
        id: uuid::Uuid::new_v4().to_string(),
        config: req,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db::test_source_insert(&state.db, &config).await?;
    rescan_after(&state, "create").await;
    Ok((StatusCode::CREATED, Json(config)))
}

async fn put_test_config(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<TestSourceRequest>,
) -> ApiResult<Json<TestSourceConfigDto>> {
    const NOT_FOUND: ApiError = ApiError::NotFound("test source not found");
    let existing = db::test_source_get(&state.db, &id).await?.ok_or(NOT_FOUND)?;
    let config = TestSourceConfigDto { config: req, ..existing };
    if !db::test_source_update(&state.db, &config).await? {
        return Err(NOT_FOUND);
    }
    rescan_after(&state, "update").await;
    Ok(Json(config))
}

async fn delete_test_config(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    if !db::test_source_delete(&state.db, &id).await? {
        return Err(ApiError::NotFound("test source not found"));
    }
    rescan_after(&state, "delete").await;
    Ok(StatusCode::NO_CONTENT)
}

// ── /recordings ───────────────────────────────────────────────────────────────

async fn get_recordings(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<RecordingSessionDto>>> {
    let active: Vec<RecordingSessionDto> =
        state.source_manager.read().await.active_sessions().into_iter().cloned().collect();
    let historical = db::sessions_list(&state.db)
        .await?
        .into_iter()
        .filter(|r| !active.iter().any(|a| a.id == r.id));
    Ok(Json(active.iter().cloned().chain(historical).collect()))
}

async fn get_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<RecordingSessionDto>> {
    let active = state
        .source_manager
        .read()
        .await
        .active_sessions()
        .into_iter()
        .find(|s| s.id == id)
        .cloned();
    let session = match active {
        Some(s) => Some(s),
        None => db::session_get(&state.db, &id).await?,
    };
    session.map(Json).ok_or(ApiError::NotFound("session not found"))
}

async fn post_recording(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRecordingRequest>,
) -> ApiResult<(StatusCode, Json<RecordingSessionDto>)> {
    if req.outputs.is_empty() {
        return Err(ApiError::BadRequest("at least one output is required"));
    }
    let legs = build_legs(&state, &req)?;
    for (path, _) in &legs {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
    }

    let preset_id = req.preset_id.clone().unwrap_or_default();
    let session = state.source_manager.write().await.start_recording(&req.source_id, &preset_id, &legs)?;

    if let Err(e) = db::session_insert(&state.db, &session).await {
        error!(error = %e, "persist session start");
    }
    state.emit(&WsEvent::RecordingStarted {
        session_id: session.id.clone(),
        source_id: session.source_id.clone(),
    });
    Ok((StatusCode::CREATED, Json(session)))
}

/// Wait for a stop's final DTO on `rx`, whether this request started the
/// stop or is joining one already in flight.
async fn await_stop_result(mut rx: StopResult) -> Option<RecordingSessionDto> {
    loop {
        if let Some(dto) = rx.borrow_and_update().clone() {
            return Some(dto);
        }
        if rx.changed().await.is_err() {
            return None;
        }
    }
}

async fn post_stop_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<RecordingSessionDto>> {
    let outcome = state.source_manager.write().await.begin_stop_recording(&id);
    let session = match outcome {
        StopOutcome::Start(job) => {
            let rx = job.tx.subscribe();
            // Spawned independently: if the caller's connection drops while
            // we're awaiting below, only this request's response is affected
            // — the stop keeps running to completion regardless.
            tokio::spawn(session::run_stop(Arc::clone(&state), *job, None));
            await_stop_result(rx).await
        }
        StopOutcome::Join(rx) => await_stop_result(rx).await,
        StopOutcome::NotFound => stop_orphaned(&state, &id).await?,
    };
    session.map(Json).ok_or(ApiError::NotFound("session not found"))
}

/// A session with no in-memory record, e.g. left `active` by a crash: mark it
/// stopped in the DB — but only if it isn't already, so a stray retry landing
/// here after everything settled doesn't stomp `stopped_at`.
async fn stop_orphaned(state: &AppState, id: &str) -> anyhow::Result<Option<RecordingSessionDto>> {
    let Some(mut session) = db::session_get(&state.db, id).await? else {
        return Ok(None);
    };
    if session.status == RecordingStatus::Active {
        let stopped_at = chrono::Utc::now().to_rfc3339();
        db::session_update_stop(&state.db, id, &stopped_at, RecordingStatus::Stopped, None).await?;
        session.stopped_at = Some(stopped_at);
        session.status = RecordingStatus::Stopped;
        session.error_message = None;
    }
    Ok(Some(session))
}

// ── /thumbnails/{source_id} ───────────────────────────────────────────────────

async fn get_thumbnail(
    State(state): State<Arc<AppState>>,
    AxumPath(source_id): AxumPath<String>,
) -> Response {
    let bytes = state.source_manager.read().await.thumbnail_bytes(&source_id);
    match bytes {
        Some(jpeg) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .body(Body::from(jpeg))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from("no thumbnail yet"))
            .unwrap(),
    }
}

// ── /ws (this node's events only) ─────────────────────────────────────────────

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let rx = state.node_tx.subscribe();
    upgrade.on_upgrade(move |socket| ws::handle(socket, rx))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn source_to_dto(mgr: &SourceManager, s: &dyn InputSource) -> SourceDto {
    SourceDto {
        id: s.id().to_string(),
        display_name: s.display_name().to_string(),
        source_type: s.source_type(),
        connected: mgr.is_monitored(s.id()),
        error: mgr.monitor_error(s.id()),
        timecode: s.timecode(),
        capabilities: s.capabilities(),
    }
}

/// After a test-source change: the change itself is saved, so a failed
/// rescan is logged rather than failing the request.
async fn rescan_after(state: &Arc<AppState>, change: &str) {
    if let Err(e) = session::rebuild_sources(state).await {
        error!(error = %e, "rebuild sources after {change}");
    }
}

/// Build `(resolved_path, RecordingProfile)` for every requested output leg.
fn build_legs(state: &AppState, req: &StartRecordingRequest) -> ApiResult<Vec<(PathBuf, RecordingProfile)>> {
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let datetime = now.format("%Y%m%d_%H%M%S").to_string();
    let node = state.node_name();

    req.outputs
        .iter()
        .map(|o| {
            let profile = RecordingProfile::from_output(o).map_err(ApiError::BadRequest)?;
            let path = o
                .path_template
                .replace("{source}", &req.source_id)
                .replace("{node}", &node)
                .replace("{date}", &date)
                .replace("{datetime}", &datetime)
                .replace("{output}", &o.name)
                .replace("{ext}", profile.file_extension());
            Ok((PathBuf::from(path), profile))
        })
        .collect()
}
