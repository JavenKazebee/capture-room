//! The node API: everything this machine has — sources, storage, recordings,
//! thumbnails, settings. Local only: it never forwards and never knows about
//! other nodes. Mounted at `/api/v1/node`; controllers reach a peer's copy
//! through `/api/v1/nodes/{id}/…`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use tracing::error;

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    ConfiguredSourceDto, ConfiguredSourceRequest, DirListingDto, NodeSettingsDto, NodeStatus, RecordingSessionDto,
    RecordingStatus, SourceConfig, SourceDto, StartRecordingRequest, StorageVolumeDto, UpdateNodeSettingsRequest,
    WsEvent,
};
use crate::db;
use crate::pipeline::profile::{plan_legs, PathVars, RecordingProfile};
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
        .route("/files", get(get_files))
        // Sources — static paths before dynamic {id}
        .route("/sources", get(get_sources))
        .route("/sources/scan", post(post_scan))
        .route("/sources/{id}", get(get_source))
        .route("/configured-sources", get(get_configured_sources).post(post_configured_source))
        .route(
            "/configured-sources/{id}",
            axum::routing::put(put_configured_source).delete(delete_configured_source),
        )
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
        encoders: crate::pipeline::profile::available_encoders(),
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
    state.emit(&WsEvent::NodeUpdated);
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

// ── /configured-sources ───────────────────────────────────────────────────────

async fn get_configured_sources(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<ConfiguredSourceDto>>> {
    Ok(Json(db::configured_sources_list(&state.db).await?))
}

/// Check a request, and probe a media file so the source knows its format.
async fn validated(mut req: ConfiguredSourceRequest) -> ApiResult<ConfiguredSourceRequest> {
    req.name = req.name.trim().to_string();
    if req.name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if let SourceConfig::File(cfg) = &mut req.config {
        cfg.path = cfg.path.trim().to_string();
        let path = cfg.path.clone();
        let media = tokio::task::spawn_blocking(move || crate::sources::file::probe(&path))
            .await?
            .map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
        cfg.media = Some(media);
    }
    Ok(req)
}

async fn post_configured_source(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConfiguredSourceRequest>,
) -> ApiResult<(StatusCode, Json<ConfiguredSourceDto>)> {
    let req = validated(req).await?;
    let source = ConfiguredSourceDto {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        config: req.config,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db::configured_source_insert(&state.db, &source).await?;
    rescan_after(&state, "create").await;
    Ok((StatusCode::CREATED, Json(source)))
}

async fn put_configured_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ConfiguredSourceRequest>,
) -> ApiResult<Json<ConfiguredSourceDto>> {
    const NOT_FOUND: ApiError = ApiError::NotFound("source not found");
    let existing = db::configured_source_get(&state.db, &id).await?.ok_or(NOT_FOUND)?;
    let req = validated(req).await?;
    let source = ConfiguredSourceDto { name: req.name, config: req.config, ..existing };
    if !db::configured_source_update(&state.db, &source).await? {
        return Err(NOT_FOUND);
    }
    rescan_after(&state, "update").await;
    Ok(Json(source))
}

async fn delete_configured_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    if !db::configured_source_delete(&state.db, &id).await? {
        return Err(ApiError::NotFound("source not found"));
    }
    rescan_after(&state, "delete").await;
    Ok(StatusCode::NO_CONTENT)
}

// ── /files ────────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct FilesQuery {
    path: Option<String>,
}

/// A directory's folders and media files, for picking a file source.
async fn get_files(Query(q): Query<FilesQuery>) -> ApiResult<Json<DirListingDto>> {
    let listing = tokio::task::spawn_blocking(move || crate::storage::list_dir(q.path.as_deref()))
        .await?
        .map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
    Ok(Json(listing))
}

// ── /recordings ───────────────────────────────────────────────────────────────

async fn get_recordings(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<RecordingSessionDto>>> {
    let active: Vec<RecordingSessionDto> =
        state.source_manager.read().await.active_sessions();
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
        .find(|s| s.id == id);
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
        return Err(ApiError::BadRequest("at least one output is required".into()));
    }
    let legs = build_legs(&state, &req).await?;
    for (path, _) in &legs {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| format!("create {}", parent.display()))?;
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
        db::session_update_stop(&state.db, id, &stopped_at, RecordingStatus::Stopped, None, None, None).await?;
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
/// Never picks a file that already exists: a template with `{take}` gets the
/// first take number none of whose files exist yet, any other gets the first
/// `_2`, `_3`, … suffix that doesn't.
async fn build_legs(state: &AppState, req: &StartRecordingRequest) -> ApiResult<Vec<(PathBuf, RecordingProfile)>> {
    let (source_name, format) = {
        let mgr = state.source_manager.read().await;
        let name = mgr.get_source(&req.source_id).map(|s| s.display_name().to_string());
        (name.unwrap_or_else(|| req.source_id.clone()), mgr.source_format(&req.source_id))
    };
    let mut vars = PathVars {
        source: req.source_id.clone(),
        source_name,
        node: state.node_name(),
        preset: req.preset_name.clone().unwrap_or_else(|| "default".into()),
        at: chrono::Local::now(),
        take: 1,
        suffix: 1,
        source_resolution: format.size,
        source_framerate: format.rate,
        source_audio: format.audio,
    };
    // Each step changes the path that exists (its take, or every file's
    // suffix), so this ends once it runs past the files on disk.
    loop {
        let legs = plan_legs(&req.outputs, Some(&vars)).map_err(|e| ApiError::BadRequest(e.into()))?;
        let Some(i) = legs.iter().position(|(path, _)| path.exists()) else {
            return Ok(legs);
        };
        let counter = if req.outputs[i].path_template.contains("{take}") { &mut vars.take } else { &mut vars.suffix };
        *counter = counter
            .checked_add(1)
            .ok_or_else(|| ApiError::BadRequest(format!("{} already exists", legs[i].0.display()).into()))?;
    }
}
