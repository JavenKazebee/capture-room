//! The node API: everything this machine has — sources, storage, recordings,
//! thumbnails, settings. Local only: it never forwards and never knows about
//! other nodes. Mounted at `/api/v1/node`; controllers reach a peer's copy
//! through `/api/v1/nodes/{id}/…`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path as AxumPath, Query, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::{error, info};

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    BenchmarkRequest, BenchmarkRunDto, CapacityCheckDto, CapacityCheckRequest, ConfiguredSourceDto,
    ConfiguredSourceRequest, DeviceDto, DirListingDto, NodeCapacityDto, NodeSettingsDto,
    NodeStatus, RecordingSessionDto, RecordingStatus, RecordingsQuery, SourceConfig, SourceDto,
    StartRecordingRequest, StorageVolumeDto, UpdateNodeSettingsRequest, VolumeCheckDto, WsEvent,
};
use crate::benchmark;
use crate::capacity::{self, Capacity};
use crate::db;
use crate::pipeline::profile::{plan_legs, PathVars, RecordingProfile};
use crate::session;
use crate::sources::manager::{SourceManager, StopOutcome, StopResult};
use crate::sources::InputSource;
use crate::state::AppState;
use crate::storage;
use crate::ws;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/status", get(get_status))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/storage", get(get_storage))
        .route("/files", get(get_files))
        .route("/devices", get(get_devices))
        // Sources — static paths before dynamic {id}
        .route("/sources", get(get_sources))
        .route("/sources/scan", post(post_scan))
        .route("/sources/{id}", get(get_source))
        .route(
            "/configured-sources",
            get(get_configured_sources).post(post_configured_source),
        )
        .route(
            "/configured-sources/{id}",
            axum::routing::put(put_configured_source).delete(delete_configured_source),
        )
        .route("/recordings", get(get_recordings).post(post_recording))
        .route(
            "/recordings/{id}",
            get(get_recording).delete(delete_recording),
        )
        .route(
            "/recordings/{id}/outputs/{output}/files/{file}",
            get(get_recording_file).delete(delete_recording_file),
        )
        .route("/recordings/{id}/stop", post(post_stop_recording))
        .route("/benchmarks", get(get_benchmarks).post(post_benchmark))
        .route(
            "/benchmarks/{id}",
            get(get_benchmark).delete(delete_benchmark),
        )
        .route("/benchmarks/{id}/cancel", post(post_cancel_benchmark))
        .route("/capacity", get(get_capacity))
        .route("/capacity/check", post(post_capacity_check))
        .route("/thumbnails/{source_id}", get(get_thumbnail))
        .route("/ws", get(ws_handler))
        .merge(super::playout::router())
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
        source_types: crate::plugins::source_types(),
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
    if let Some(name) = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
    {
        db::config_set(&state.db, "name", &name).await?;
        *state.node_name.write().unwrap() = name;
    }
    if let Some(monitor) = req.monitor {
        let monitor = monitor.clamped();
        db::monitor_settings_set(&state.db, &monitor).await?;
        state
            .source_manager
            .write()
            .await
            .apply_monitor_config(monitor);
    }
    state.emit(&WsEvent::NodeUpdated);
    Ok(Json(settings_dto(&state).await))
}

// ── /storage ──────────────────────────────────────────────────────────────────

/// Volumes, with what this node's active recordings write to each and the
/// recording time that leaves.
async fn get_storage(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<StorageVolumeDto>>> {
    let legs = state.source_manager.read().await.active_leg_files();
    let volumes = tokio::task::spawn_blocking(move || {
        let mut volumes = storage::list_volumes();
        storage::apply_write_rates(&mut volumes, &storage::leg_write_rates(&legs));
        volumes
    })
    .await?;
    Ok(Json(volumes))
}

// ── /sources ──────────────────────────────────────────────────────────────────

fn sources_list(mgr: &SourceManager) -> Vec<SourceDto> {
    mgr.sources()
        .iter()
        .map(|s| source_to_dto(mgr, s.as_ref()))
        .collect()
}

async fn get_sources(State(state): State<Arc<AppState>>) -> Json<Vec<SourceDto>> {
    Json(sources_list(&*state.source_manager.read().await))
}

async fn get_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<SourceDto>> {
    let mgr = state.source_manager.read().await;
    let source = mgr
        .get_source(&id)
        .ok_or(ApiError::NotFound("source not found"))?;
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
/// `id` is the source being replaced, if any.
async fn validated(
    state: &AppState,
    id: Option<&str>,
    mut req: ConfiguredSourceRequest,
) -> ApiResult<ConfiguredSourceRequest> {
    req.name = req.name.trim().to_string();
    if req.name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let devices = state.source_manager.read().await.devices().clone();
    let bad = |e: String| ApiError::BadRequest(e.into());
    match &mut req.config {
        SourceConfig::Stream(cfg) => {
            cfg.url = cfg.url.trim().to_string();
            crate::sources::stream::validate(cfg, crate::plugins::has).map_err(bad)?;
            crate::sources::device::validate_audio(&cfg.audio, &devices).map_err(bad)?;
        }
        SourceConfig::Device(cfg) => {
            crate::sources::device::validate(cfg, &devices).map_err(bad)?;
        }
        SourceConfig::Whip(cfg) => {
            crate::sources::whip::validate(cfg).map_err(bad)?;
            crate::sources::device::validate_audio(&cfg.audio, &devices).map_err(bad)?;
        }
        SourceConfig::Channel(cfg) => {
            let others = db::configured_sources_list(&state.db).await?;
            let others: Vec<_> = others
                .iter()
                .filter(|o| Some(o.id.as_str()) != id)
                .filter_map(|o| match &o.config {
                    SourceConfig::Channel(c) => Some((o.name.as_str(), c)),
                    _ => None,
                })
                .collect();
            crate::sources::channel::validate(cfg, &req.name, &others).map_err(bad)?;
        }
        _ => {}
    }
    if let Some(port) = listen_port(&req.config) {
        let others = db::configured_sources_list(&state.db).await?;
        if let Some(other) = others
            .iter()
            .find(|o| Some(o.id.as_str()) != id && listen_port(&o.config) == Some(port))
        {
            return Err(ApiError::BadRequest(
                format!("port {port} is already used by {}", other.name).into(),
            ));
        }
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
    let req = validated(&state, None, req).await?;
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
    let existing = db::configured_source_get(&state.db, &id)
        .await?
        .ok_or(NOT_FOUND)?;
    let req = validated(&state, Some(&id), req).await?;
    let source = ConfiguredSourceDto {
        name: req.name,
        config: req.config,
        ..existing
    };
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

/// The local port a configured source listens on, if any.
fn listen_port(config: &SourceConfig) -> Option<u16> {
    match config {
        SourceConfig::Stream(cfg) => crate::sources::stream::listen_port(cfg),
        SourceConfig::Whip(cfg) => Some(cfg.port),
        _ => None,
    }
}

// ── /devices ──────────────────────────────────────────────────────────────────

/// Capture devices on the node, for picking one in a device source.
async fn get_devices(State(state): State<Arc<AppState>>) -> Json<Vec<DeviceDto>> {
    let devices = state.source_manager.read().await.devices().clone();
    Json(devices.list())
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

const SESSION_NOT_FOUND: ApiError = ApiError::NotFound("session not found");

/// Sessions newest first, a page at a time. The first page (no `before`)
/// also carries every active session, with its live counts.
async fn get_recordings(
    State(state): State<Arc<AppState>>,
    Query(q): Query<RecordingsQuery>,
) -> ApiResult<Json<Vec<RecordingSessionDto>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let active: Vec<RecordingSessionDto> = match q.before {
        None => state.source_manager.read().await.active_sessions(),
        Some(_) => Vec::new(),
    };
    let historical = db::sessions_list(&state.db, q.before.as_deref(), limit)
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
    session.map(Json).ok_or(SESSION_NOT_FOUND)
}

/// Serializes edits to finished sessions' files, so two deletes can't both
/// read a file list and each write back a copy missing only their own file.
static FILE_EDITS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn is_active(state: &AppState, id: &str) -> bool {
    state
        .source_manager
        .read()
        .await
        .active_sessions()
        .iter()
        .any(|s| s.id == id)
}

#[derive(Deserialize)]
struct DeleteRecordingQuery {
    /// Present (any value) to delete the session's files from disk too.
    files: Option<String>,
}

/// Remove a finished session from history; with `?files`, delete its files
/// from disk first. If any can't be deleted the session stays, listing just
/// the files still on disk, so none drop out of sight.
async fn delete_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<DeleteRecordingQuery>,
) -> ApiResult<StatusCode> {
    if is_active(&state, &id).await {
        return Err(ApiError::Conflict("stop the recording before removing it"));
    }
    let _edit = FILE_EDITS.lock().await;
    if q.files.is_some() {
        let mut session = db::session_get(&state.db, &id)
            .await?
            .ok_or(SESSION_NOT_FOUND)?;
        let mut files = session_files(&session);
        let mut failed = Vec::new();
        for output in &mut files {
            let mut kept = Vec::new();
            for path in output.drain(..) {
                if let Err(e) = remove_recording_file(&path).await {
                    failed.push(format!("{path} ({e})"));
                    kept.push(path);
                }
            }
            *output = kept;
        }
        if !failed.is_empty() {
            db::session_update_files(&state.db, &id, &files).await?;
            session.files = files;
            state.emit(&WsEvent::RecordingUpdated {
                session: Box::new(session),
            });
            return Err(anyhow::anyhow!("could not delete {}", failed.join(", ")).into());
        }
        info!(session = %id, "deleted recording files");
    }
    if !db::session_delete(&state.db, &id).await? {
        return Err(SESSION_NOT_FOUND);
    }
    state.emit(&WsEvent::RecordingRemoved { session_id: id });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DeleteFileQuery {
    /// The file's path as the client saw it. Indexes shift as files are
    /// deleted, so a stale index is refused rather than deleting another file.
    path: String,
}

/// Delete one of a finished session's files from disk and from its list. The
/// session stays in history, even once it has no files left.
async fn delete_recording_file(
    State(state): State<Arc<AppState>>,
    AxumPath((id, output, file)): AxumPath<(String, usize, usize)>,
    Query(q): Query<DeleteFileQuery>,
) -> ApiResult<Json<RecordingSessionDto>> {
    if is_active(&state, &id).await {
        return Err(ApiError::Conflict(
            "stop the recording before deleting its files",
        ));
    }
    let _edit = FILE_EDITS.lock().await;
    let mut session = db::session_get(&state.db, &id)
        .await?
        .ok_or(SESSION_NOT_FOUND)?;
    let mut files = session_files(&session);
    let list = files
        .get_mut(output)
        .filter(|l| file < l.len())
        .ok_or(ApiError::NotFound("no such file in this session"))?;
    if list[file] != q.path {
        return Err(ApiError::Conflict(
            "this session's files changed: refresh and try again",
        ));
    }
    let path = list.remove(file);
    remove_recording_file(&path)
        .await
        .with_context(|| format!("delete {path}"))?;
    info!(session = %id, path = %path, "deleted recording file");
    db::session_update_files(&state.db, &id, &files).await?;
    session.files = files;
    state.emit(&WsEvent::RecordingUpdated {
        session: Box::new(session.clone()),
    });
    Ok(Json(session))
}

/// Delete one recorded file; one already gone counts as deleted.
async fn remove_recording_file(path: &str) -> std::io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[derive(Deserialize)]
struct FileQuery {
    /// Present (any value) to download rather than play.
    download: Option<String>,
}

/// One of a finished session's files, by output and file index (a split
/// output has several), with range requests so browsers can seek. Only the
/// files a session recorded can be read here.
async fn get_recording_file(
    State(state): State<Arc<AppState>>,
    AxumPath((id, output, file)): AxumPath<(String, usize, usize)>,
    Query(q): Query<FileQuery>,
    req: Request,
) -> ApiResult<Response> {
    if is_active(&state, &id).await {
        return Err(ApiError::Conflict(
            "still recording: files can be played once the recording stops",
        ));
    }
    let session = db::session_get(&state.db, &id)
        .await?
        .ok_or(SESSION_NOT_FOUND)?;
    let path = session_file(&session, output, file)
        .ok_or(ApiError::NotFound("no such file in this session"))?;
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(ApiError::NotFound("file is no longer on disk"));
    }

    // A playable .mov is served as MP4 (the same ISO base media format):
    // Chrome and Firefox won't play `video/quicktime`.
    let playable = session.outputs.get(output).is_some_and(|o| o.playable);
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4") => "video/mp4",
        Some("mov") if playable => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    };
    let mime: mime_guess::Mime = mime.parse()?;
    let mut resp = ServeFile::new_with_mime(&path, &mime)
        .oneshot(req)
        .await?
        .into_response();
    if q.download.is_some() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Ok(v) = HeaderValue::from_str(&content_disposition(&name)) {
            resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(resp)
}

/// Each output's files: those it recorded, or its one planned path for
/// sessions from before files were kept. An output can have none: a leg that
/// never opened a file, or one whose files were all deleted.
fn session_files(session: &RecordingSessionDto) -> Vec<Vec<String>> {
    if session.files.is_empty() {
        session
            .output_paths
            .iter()
            .map(|p| vec![p.clone()])
            .collect()
    } else {
        session.files.clone()
    }
}

/// The path of a session's `file`th file from output `output` (see [`session_files`]).
fn session_file(session: &RecordingSessionDto, output: usize, file: usize) -> Option<PathBuf> {
    let path = if session.files.is_empty() {
        session.output_paths.get(output).filter(|_| file == 0)?
    } else {
        session.files.get(output)?.get(file)?
    };
    Some(PathBuf::from(path))
}

/// `attachment` with the file's name, percent-encoded (RFC 6266/5987) so any
/// name survives, plus a plain ASCII fallback.
fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && c != '"' && c != '\\') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

async fn post_recording(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRecordingRequest>,
) -> ApiResult<(StatusCode, Json<RecordingSessionDto>)> {
    if req.outputs.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one output is required".into(),
        ));
    }
    let legs = build_legs(&state, &req).await?;
    check_free_space(&legs).await?;
    for (path, _) in &legs {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create {}", parent.display()))?;
        }
    }
    // Real recordings win: a benchmark would compete with this one (and its
    // result would be skewed by it).
    if benchmark::cancel(&state, None, "Cancelled: a recording started on this node.").await {
        info!("benchmark cancelled for a recording");
    }

    let preset_id = req.preset_id.clone().unwrap_or_default();
    let key = capacity::outputs_key(&req.outputs);
    let session = state.source_manager.write().await.start_recording(
        &req.source_id,
        &preset_id,
        req.preset_name.as_deref(),
        &req.outputs,
        &legs,
        key,
    )?;

    if let Err(e) = db::session_insert(&state.db, &session).await {
        error!(error = %e, "persist session start");
    }
    state.emit(&WsEvent::RecordingStarted {
        session_id: session.id.clone(),
        source_id: session.source_id.clone(),
    });
    Ok((StatusCode::CREATED, Json(session)))
}

/// Refuse to record onto a volume that's all but full: the leg would fail
/// within moments anyway.
async fn check_free_space(legs: &[(PathBuf, RecordingProfile)]) -> ApiResult<()> {
    let paths: Vec<PathBuf> = legs.iter().map(|(p, _)| p.clone()).collect();
    let full = tokio::task::spawn_blocking(move || {
        let volumes = storage::list_volumes();
        paths.iter().find_map(|p| {
            let v = &volumes[storage::volume_of(p, &volumes)?];
            (v.available_bytes < storage::MIN_FREE_TO_RECORD)
                .then(|| (v.mount_point.clone(), v.available_bytes))
        })
    })
    .await?;
    match full {
        Some((mount, free)) => Err(ApiError::BadRequest(
            format!(
                "{mount} is nearly full ({:.1} GB free); free up space or record somewhere else",
                free as f64 / 1e9
            )
            .into(),
        )),
        None => Ok(()),
    }
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
    session
        .map(Json)
        .ok_or(ApiError::NotFound("session not found"))
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
        db::session_update_stop(
            &state.db,
            id,
            &stopped_at,
            RecordingStatus::Stopped,
            None,
            None,
            None,
        )
        .await?;
        session.stopped_at = Some(stopped_at);
        session.status = RecordingStatus::Stopped;
        session.error_message = None;
    }
    Ok(Some(session))
}

// ── /benchmarks ───────────────────────────────────────────────────────────────

async fn get_benchmarks(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<BenchmarkRunDto>>> {
    Ok(Json(db::benchmarks_list(&state.db).await?))
}

async fn get_benchmark(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<BenchmarkRunDto>> {
    db::benchmark_get(&state.db, &id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound("benchmark not found"))
}

async fn post_benchmark(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BenchmarkRequest>,
) -> ApiResult<(StatusCode, Json<BenchmarkRunDto>)> {
    Ok((
        StatusCode::CREATED,
        Json(benchmark::start(&state, req).await?),
    ))
}

/// Cancel a running benchmark; answers once it has torn down.
async fn post_cancel_benchmark(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<BenchmarkRunDto>> {
    if !benchmark::cancel(&state, Some(&id), "Cancelled.").await {
        return Err(ApiError::Conflict("that benchmark isn't running"));
    }
    get_benchmark(State(state), AxumPath(id)).await
}

async fn delete_benchmark(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    if benchmark::running_id(&state).as_deref() == Some(id.as_str()) {
        return Err(ApiError::Conflict(
            "cancel the benchmark before deleting it",
        ));
    }
    if !db::benchmark_delete(&state.db, &id).await? {
        return Err(ApiError::NotFound("benchmark not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── /capacity ─────────────────────────────────────────────────────────────────

async fn get_capacity(State(state): State<Arc<AppState>>) -> ApiResult<Json<NodeCapacityDto>> {
    let runs = db::benchmarks_list(&state.db).await?;
    let capacity = Capacity::from_runs(&runs);
    let feeds = state.source_manager.read().await.active_feeds();
    let load = capacity.load(feeds.iter().map(|(key, format)| (key.as_str(), *format)));
    let running =
        benchmark::running_id(&state).and_then(|id| runs.into_iter().find(|r| r.id == id));
    Ok(Json(NodeCapacityDto {
        profiles: capacity.profiles(),
        active_feeds: feeds.len() as u32,
        load: load.load,
        unknown_feeds: load.unknown,
        running,
    }))
}

/// Would recording these sources with these outputs fit on this node — its
/// benchmarked capacity, and the space on the volumes they'd write to?
async fn post_capacity_check(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CapacityCheckRequest>,
) -> ApiResult<Json<CapacityCheckDto>> {
    let capacity = Capacity::from_runs(&db::benchmarks_list(&state.db).await?);
    let key = capacity::outputs_key(&req.outputs);
    let (active, formats, active_legs) = {
        let mgr = state.source_manager.read().await;
        let formats: Vec<_> = req
            .source_ids
            .iter()
            .map(|id| mgr.source_format(id))
            .collect();
        (mgr.active_feeds(), formats, mgr.active_leg_files())
    };
    let active_iter = || active.iter().map(|(k, f)| (k.as_str(), *f));
    let before = capacity.load(active_iter());
    let after = capacity.load(active_iter().chain(formats.iter().map(|f| (key.as_str(), *f))));

    // What the new legs would write, and where: measured by a benchmark of
    // these outputs where there is one, else estimated from their settings.
    let mut new_writes = Vec::new();
    for (source_id, format) in req.source_ids.iter().zip(&formats) {
        let start = StartRecordingRequest {
            source_id: source_id.clone(),
            preset_id: None,
            preset_name: None,
            outputs: req.outputs.clone(),
        };
        let legs = build_legs(&state, &start).await?;
        let measured = capacity
            .lookup(&key, *format)
            .map(|m| m.output_bytes_per_sec)
            .filter(|rates| rates.len() == legs.len());
        for (i, (path, profile)) in legs.into_iter().enumerate() {
            let rate = measured
                .as_ref()
                .map_or_else(|| profile.estimated_bytes_per_sec(), |rates| rates[i]);
            new_writes.push((path, rate));
        }
    }
    let volumes = tokio::task::spawn_blocking(move || {
        let mut volumes = storage::list_volumes();
        let mut writes = storage::leg_write_rates(&active_legs);
        writes.extend(new_writes.iter().cloned());
        storage::apply_write_rates(&mut volumes, &writes);
        let mut touched: Vec<usize> = new_writes
            .iter()
            .filter_map(|(p, _)| storage::volume_of(p, &volumes))
            .collect();
        touched.sort_unstable();
        touched.dedup();
        touched
            .into_iter()
            .map(|i| {
                let v = &volumes[i];
                VolumeCheckDto {
                    mount_point: v.mount_point.clone(),
                    available_bytes: v.available_bytes,
                    write_bytes_per_sec: v.write_bytes_per_sec,
                    seconds_left: v.seconds_left,
                }
            })
            .collect()
    })
    .await?;

    Ok(Json(CapacityCheckDto {
        verdict: after.verdict(),
        load_before: before.load,
        load_after: after.load,
        unknown_feeds: after.unknown,
        estimated: after.estimated,
        volumes,
    }))
}

// ── /thumbnails/{source_id} ───────────────────────────────────────────────────

async fn get_thumbnail(
    State(state): State<Arc<AppState>>,
    AxumPath(source_id): AxumPath<String>,
) -> Response {
    let bytes = state
        .source_manager
        .read()
        .await
        .thumbnail_bytes(&source_id);
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

async fn ws_handler(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
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
        link: mgr.link(s),
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
async fn build_legs(
    state: &AppState,
    req: &StartRecordingRequest,
) -> ApiResult<Vec<(PathBuf, RecordingProfile)>> {
    let (source_name, format) = {
        let mgr = state.source_manager.read().await;
        let name = mgr
            .get_source(&req.source_id)
            .map(|s| s.display_name().to_string());
        (
            name.unwrap_or_else(|| req.source_id.clone()),
            mgr.source_format(&req.source_id),
        )
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
        let legs =
            plan_legs(&req.outputs, Some(&vars)).map_err(|e| ApiError::BadRequest(e.into()))?;
        let Some(i) = legs.iter().position(|(path, _)| path.exists()) else {
            return Ok(legs);
        };
        let counter = if req.outputs[i].path_template.contains("{take}") {
            &mut vars.take
        } else {
            &mut vars.suffix
        };
        *counter = counter.checked_add(1).ok_or_else(|| {
            ApiError::BadRequest(format!("{} already exists", legs[i].0.display()).into())
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(output_paths: &[&str], files: Vec<Vec<&str>>) -> RecordingSessionDto {
        RecordingSessionDto {
            id: "s".into(),
            source_id: "src".into(),
            preset_id: String::new(),
            source_name: None,
            preset_name: None,
            started_at: String::new(),
            stopped_at: None,
            outputs: Vec::new(),
            output_paths: output_paths.iter().map(|p| p.to_string()).collect(),
            dropped_frames: Vec::new(),
            files: files
                .into_iter()
                .map(|f| f.into_iter().map(String::from).collect())
                .collect(),
            status: RecordingStatus::Stopped,
            error_message: None,
        }
    }

    #[test]
    fn session_file_reads_only_the_sessions_files() {
        let s = session(
            &["/a.mov", "/b_{segment}.mp4"],
            vec![vec!["/a.mov"], vec!["/b_001.mp4", "/b_002.mp4"]],
        );
        assert_eq!(session_file(&s, 0, 0), Some(PathBuf::from("/a.mov")));
        assert_eq!(session_file(&s, 1, 1), Some(PathBuf::from("/b_002.mp4")));
        assert_eq!(session_file(&s, 1, 2), None);
        assert_eq!(session_file(&s, 2, 0), None);
    }

    #[test]
    fn session_file_falls_back_to_the_planned_path() {
        let s = session(&["/a.mov"], Vec::new());
        assert_eq!(session_file(&s, 0, 0), Some(PathBuf::from("/a.mov")));
        assert_eq!(session_file(&s, 0, 1), None);
    }

    #[test]
    fn an_output_without_files_has_none() {
        // All of its files deleted: the planned path mustn't stand in for them.
        let s = session(&["/a.mov", "/b.mov"], vec![vec![], vec!["/b.mov"]]);
        assert_eq!(session_file(&s, 0, 0), None);
        assert_eq!(
            session_files(&s),
            vec![Vec::<String>::new(), vec!["/b.mov".to_string()]]
        );
        assert_eq!(session_file(&s, 1, 0), Some(PathBuf::from("/b.mov")));
    }

    #[test]
    fn content_disposition_encodes_any_name() {
        assert_eq!(
            content_disposition("cam 1.mov"),
            "attachment; filename=\"cam 1.mov\"; filename*=UTF-8''cam%201.mov"
        );
        assert_eq!(
            content_disposition("é\"x.mp4"),
            "attachment; filename=\"__x.mp4\"; filename*=UTF-8''%C3%A9%22x.mp4"
        );
    }
}
