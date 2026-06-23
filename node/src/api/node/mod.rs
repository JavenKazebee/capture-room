use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path as AxumPath, Query, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::error;

use crate::api::types::{
    CreateTestSourceRequest, PatchRecordingRequest, PresetCreateRequest, PresetDto,
    PresetSyncRequest, RecordingSessionDto, SourceCapabilitiesDto, SourceDto, StartRecordingRequest,
    TestSourceConfigDto, TimecodeDto, UpdateTestSourceRequest, WsEvent,
};
use crate::controller::{proxy, sync};
use crate::db::{self, PresetRow};
use crate::pipeline::profile::RecordingProfile;
use crate::recording;
use crate::sources::Timecode;
use crate::state::{AppState, Role};
use crate::ws;

// ── Embedded UI ───────────────────────────────────────────────────────────────

#[derive(RustEmbed)]
#[folder = "../ui/dist"]
struct UiAssets;

fn serve_asset(path: &str) -> Option<Response> {
    let content = UiAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(content.data.into_owned()))
            .unwrap(),
    )
}

async fn serve_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    serve_asset(path)
        .or_else(|| serve_asset("index.html"))
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/settings", get(get_settings).put(put_settings))
        .route("/api/v1/nodes", get(get_nodes))
        // Sources — static paths before dynamic {id}
        .route("/api/v1/sources", get(get_sources))
        .route("/api/v1/sources/scan", post(post_scan))
        .route("/api/v1/sources/test", get(get_test_configs).post(post_test_config))
        .route(
            "/api/v1/sources/test/{id}",
            put(put_test_config).delete(delete_test_config),
        )
        .route("/api/v1/sources/{id}", get(get_source))
        .route("/api/v1/recordings", get(get_recordings).post(post_recording))
        .route(
            "/api/v1/recordings/{id}",
            get(get_recording).patch(patch_recording),
        )
        .route("/api/v1/thumbnails/{source_id}", get(get_thumbnail))
        .route("/api/v1/presets", get(get_presets).post(post_preset))
        .route("/api/v1/presets/{id}", put(put_preset).delete(delete_preset))
        .route("/api/v1/presets/sync", post(post_presets_sync))
        .route("/ws", get(ws_handler))
        .fallback(serve_ui)
}

// ── Composite-ID helpers ─────────────────────────────────────────────────────
//
// Every instance presents IDs as `{node_id}:{local}` using its OWN node_id.
// Inbound IDs are split back to their local part for handling.

fn composite(node_id: &str, local: &str) -> String {
    format!("{node_id}:{local}")
}

/// `(node_id, local)` — node_id is `None` for a bare (un-prefixed) id.
fn split_id(id: &str) -> (Option<&str>, &str) {
    match id.split_once(':') {
        Some((node, local)) => (Some(node), local),
        None => (None, id),
    }
}

fn source_value(s: &dyn crate::sources::InputSource, node_id: &str) -> Value {
    let mut v = serde_json::to_value(source_to_dto(s)).unwrap();
    v["id"] = json!(composite(node_id, s.id()));
    v["node_id"] = json!(node_id);
    v
}

fn session_value(dto: &RecordingSessionDto, node_id: &str) -> Value {
    let mut v = serde_json::to_value(dto).unwrap();
    v["source_id"] = json!(composite(node_id, &dto.source_id));
    v["node_id"] = json!(node_id);
    v
}

// ── /api/v1/status ────────────────────────────────────────────────────────────

async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(crate::api::types::NodeStatus {
        id: state.node_id.clone(),
        name: state.node_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        mode: state.role.as_str().to_string(),
    })
}

// ── /api/v1/settings ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct SettingsDto {
    node_id: String,
    node_name: String,
    /// Role currently in effect (fixed at startup).
    role: String,
    /// Role persisted in config; takes effect on next restart.
    persisted_role: Option<String>,
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Response {
    let persisted = db::config_get(&state.db, "role").await.ok().flatten();
    Json(SettingsDto {
        node_id: state.node_id.clone(),
        node_name: state.node_name.clone(),
        role: state.role.as_str().to_string(),
        persisted_role: persisted,
    })
    .into_response()
}

#[derive(Deserialize)]
struct RoleUpdate {
    role: String,
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RoleUpdate>,
) -> Response {
    let role = match Role::parse(&req.role) {
        Some(r) => r,
        None => {
            return (StatusCode::BAD_REQUEST, "role must be 'node' or 'aggregator'")
                .into_response()
        }
    };
    if let Err(e) = db::config_set(&state.db, "role", role.as_str()).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    Json(json!({
        "persisted_role": role.as_str(),
        "restart_required": role != state.role,
    }))
    .into_response()
}

// ── /api/v1/nodes (aggregator view of peers, plus self) ──────────────────────

#[derive(Serialize)]
struct NodeDto {
    id: String,
    name: String,
    url: String,
    version: String,
    healthy: bool,
    uptime_secs: u64,
    is_self: bool,
}

async fn get_nodes(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut dtos = vec![NodeDto {
        id: state.node_id.clone(),
        name: state.node_name.clone(),
        url: String::new(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        healthy: true,
        uptime_secs: state.started_at.elapsed().as_secs(),
        is_self: true,
    }];

    if state.role.is_aggregator() {
        let peers = state.peers.read().await;
        for n in peers.all() {
            dtos.push(NodeDto {
                id: n.id.clone(),
                name: n.name.clone(),
                url: n.url.clone(),
                version: n.version.clone(),
                healthy: n.healthy,
                uptime_secs: n.uptime_secs,
                is_self: false,
            });
        }
    }

    Json(dtos)
}

// ── /api/v1/sources ───────────────────────────────────────────────────────────

async fn get_sources(State(state): State<Arc<AppState>>) -> Response {
    let mut all: Vec<Value> = {
        let sources = state.sources.read().await;
        sources
            .sources()
            .iter()
            .map(|s| source_value(s.as_ref(), &state.node_id))
            .collect()
    };

    if state.role.is_aggregator() {
        all.extend(proxy::fan_out_sources(&state).await);
    }

    Json(all).into_response()
}

async fn get_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let (node, local) = split_id(&id);

    // Remote source on an aggregator → can't fetch a single peer source cheaply
    // without another endpoint; fall back to scanning the merged list.
    if let Some(n) = node {
        if n != state.node_id {
            if state.role.is_aggregator() {
                let merged = proxy::fan_out_sources(&state).await;
                if let Some(found) = merged.into_iter().find(|s| s["id"] == json!(id)) {
                    return Json(found).into_response();
                }
            }
            return (StatusCode::NOT_FOUND, "source not found").into_response();
        }
    }

    let sources = state.sources.read().await;
    match sources.get(local) {
        Some(s) => Json(source_value(s, &state.node_id)).into_response(),
        None => (StatusCode::NOT_FOUND, "source not found").into_response(),
    }
}

async fn post_scan(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let configs = match db::test_sources_list(&state.db).await {
        Ok(rows) => rows.into_iter().map(db_row_to_config).collect::<Vec<_>>(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let local: Vec<Value> = {
        let mut sources = state.sources.write().await;
        match sources.scan(&configs) {
            Ok(()) => sources
                .sources()
                .iter()
                .map(|s| source_value(s.as_ref(), &state.node_id))
                .collect(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    let mut all = local;
    if state.role.is_aggregator() {
        all.extend(proxy::fan_out_sources(&state).await);
    }
    Json(all).into_response()
}

// ── /api/v1/sources/test — test source config authoring ──────────────────────

async fn get_test_configs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TestSourceQuery>,
) -> Response {
    if let Some(target) = params.node_id.filter(|id| id != &state.node_id) {
        return proxy::get_test_configs(&state, &target).await;
    }
    match db::test_sources_list(&state.db).await {
        Ok(rows) => Json(rows.into_iter().map(row_to_config_dto).collect::<Vec<_>>()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct TestSourceQuery {
    node_id: Option<String>,
}

async fn post_test_config(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TestSourceQuery>,
    Json(req): Json<CreateTestSourceRequest>,
) -> Response {
    if let Some(target) = params.node_id.filter(|id| id != &state.node_id) {
        return proxy::create_test_source(&state, &target, &req).await;
    }
    let row = db::TestSourceRow {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        pattern: req.pattern,
        width: req.width as i64,
        height: req.height as i64,
        fps_num: req.fps_num as i64,
        fps_den: req.fps_den as i64,
        audio_signal: req.audio_signal,
        frequency: req.frequency,
        channels: req.channels as i64,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = db::test_source_insert(&state.db, &row).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    // Rebuild the source registry so the new source is immediately available.
    let _ = rebuild_registry(&state).await;
    (StatusCode::CREATED, Json(row_to_config_dto(row))).into_response()
}

async fn put_test_config(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(params): Query<TestSourceQuery>,
    Json(req): Json<UpdateTestSourceRequest>,
) -> Response {
    if let Some(target) = params.node_id.filter(|nid| nid != &state.node_id) {
        return proxy::update_test_source(&state, &target, &id, &req).await;
    }
    let existing = match db::test_source_get(&state.db, &id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "test source not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let row = db::TestSourceRow {
        name: req.name,
        pattern: req.pattern,
        width: req.width as i64,
        height: req.height as i64,
        fps_num: req.fps_num as i64,
        fps_den: req.fps_den as i64,
        audio_signal: req.audio_signal,
        frequency: req.frequency,
        channels: req.channels as i64,
        ..existing
    };
    match db::test_source_update(&state.db, &row).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "test source not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    let _ = rebuild_registry(&state).await;
    Json(row_to_config_dto(row)).into_response()
}

async fn delete_test_config(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(params): Query<TestSourceQuery>,
) -> Response {
    if let Some(target) = params.node_id.filter(|nid| nid != &state.node_id) {
        return proxy::delete_test_source(&state, &target, &id).await;
    }
    match db::test_source_delete(&state.db, &id).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "test source not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    let _ = rebuild_registry(&state).await;
    StatusCode::NO_CONTENT.into_response()
}

// ── /api/v1/sources/{id}/connect|disconnect ───────────────────────────────────

async fn post_connect(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let local = split_id(&id).1.to_string();
    let mut sources = state.sources.write().await;
    match sources.connect(&local) {
        Ok(true) => {
            let dto = sources.get(&local).map(|s| source_value(s, &state.node_id));
            Json(dto).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "source not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_disconnect(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let local = split_id(&id).1.to_string();
    let mut sources = state.sources.write().await;
    if sources.disconnect(&local) {
        let dto = sources.get(&local).map(|s| source_value(s, &state.node_id));
        Json(dto).into_response()
    } else {
        (StatusCode::NOT_FOUND, "source not found").into_response()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn row_to_config_dto(row: db::TestSourceRow) -> TestSourceConfigDto {
    TestSourceConfigDto {
        id: row.id,
        name: row.name,
        pattern: row.pattern,
        width: row.width as u32,
        height: row.height as u32,
        fps_num: row.fps_num as u32,
        fps_den: row.fps_den as u32,
        audio_signal: row.audio_signal,
        frequency: row.frequency,
        channels: row.channels as u32,
        created_at: row.created_at,
    }
}

fn db_row_to_config(row: db::TestSourceRow) -> crate::sources::test::TestSourceConfig {
    use crate::sources::test::{AudioTestSignal, TestSourceConfig, VideoTestPattern};
    TestSourceConfig {
        id: row.id,
        name: row.name,
        pattern: VideoTestPattern::from_db(&row.pattern),
        width: row.width as u32,
        height: row.height as u32,
        fps_num: row.fps_num as u32,
        fps_den: row.fps_den as u32,
        audio_signal: AudioTestSignal::from_db(&row.audio_signal),
        frequency: row.frequency,
        channels: row.channels as u32,
    }
}

async fn rebuild_registry(state: &AppState) -> anyhow::Result<()> {
    let configs = db::test_sources_list(&state.db)
        .await?
        .into_iter()
        .map(db_row_to_config)
        .collect::<Vec<_>>();
    state.sources.write().await.scan(&configs)
}

fn source_to_dto(s: &dyn crate::sources::InputSource) -> SourceDto {
    let caps = s.capabilities();
    SourceDto {
        id: s.id().to_string(),
        display_name: s.display_name().to_string(),
        source_type: format!("{:?}", s.source_type()).to_lowercase(),
        is_available: s.is_available(),
        connected: s.is_connected(),
        timecode: s.timecode().map(timecode_to_dto),
        capabilities: SourceCapabilitiesDto {
            video_formats: caps.video_formats,
            max_width: caps.max_width,
            max_height: caps.max_height,
            max_framerate: [caps.max_framerate.0, caps.max_framerate.1],
            audio_channels: caps.audio_channels,
            audio_sample_rates: caps.audio_sample_rates,
        },
    }
}

fn timecode_to_dto(tc: Timecode) -> TimecodeDto {
    TimecodeDto {
        display: tc.to_string(),
        hours: tc.hours,
        minutes: tc.minutes,
        seconds: tc.seconds,
        frames: tc.frames,
        drop_frame: tc.drop_frame,
        framerate: [tc.framerate.0, tc.framerate.1],
    }
}

// ── /api/v1/recordings ────────────────────────────────────────────────────────

async fn get_recordings(State(state): State<Arc<AppState>>) -> Response {
    let active: Vec<RecordingSessionDto> = {
        let mgr = state.recordings.read().await;
        mgr.active_sessions().into_iter().cloned().collect()
    };

    let historical = match db::sessions_list(&state.db).await {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| !active.iter().any(|a| a.id == r.id))
            .map(session_row_to_dto)
            .collect::<Vec<_>>(),
        Err(e) => {
            error!(error = %e, "db sessions_list");
            vec![]
        }
    };

    let mut all: Vec<Value> = active
        .iter()
        .chain(historical.iter())
        .map(|s| session_value(s, &state.node_id))
        .collect();

    if state.role.is_aggregator() {
        all.extend(proxy::fan_out_recordings(&state).await);
    }

    Json(all).into_response()
}

async fn get_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    {
        let mgr = state.recordings.read().await;
        if let Some(s) = mgr.active_sessions().into_iter().find(|s| s.id == id) {
            return Json(session_value(s, &state.node_id)).into_response();
        }
    }
    match db::session_get(&state.db, &id).await {
        Ok(Some(row)) => {
            return Json(session_value(&session_row_to_dto(row), &state.node_id)).into_response()
        }
        Ok(None) => {}
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }

    if state.role.is_aggregator() {
        if let Some(body) = proxy::find_recording(&state, &id).await {
            return Json(body).into_response();
        }
    }

    (StatusCode::NOT_FOUND, "session not found").into_response()
}

async fn post_recording(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRecordingRequest>,
) -> Response {
    // Route by the node prefix on the source id.
    if let Some(node) = split_id(&req.source_id).0 {
        if node != state.node_id {
            if state.role.is_aggregator() {
                return proxy::start_recording(&state, node, &req).await;
            }
            return (StatusCode::NOT_FOUND, "unknown node for source").into_response();
        }
    }

    let local_source = split_id(&req.source_id).1.to_string();
    let profile = build_profile_for_preset(&state, &req.preset_id).await;

    let primary_path = req.primary_path.clone().unwrap_or_else(|| {
        let dt = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        format!("/tmp/capture-room/{}_{}_{}.mov", local_source, dt, req.preset_id)
    });

    if let Some(parent) = Path::new(&primary_path).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    let session = {
        let sources = state.sources.read().await;
        let mut mgr = state.recordings.write().await;
        match mgr.start(&sources, &local_source, &profile, Path::new(&primary_path)) {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    if let Err(e) = recording::persist_start(&state.db, &session).await {
        error!(error = %e, "persist session start");
    }

    ws::send(
        &state.ws_tx,
        &WsEvent::RecordingStarted {
            session_id: session.id.clone(),
            source_id: composite(&state.node_id, &session.source_id),
        },
    );

    (StatusCode::CREATED, Json(session_value(&session, &state.node_id))).into_response()
}

async fn patch_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PatchRecordingRequest>,
) -> Response {
    if req.action != "stop" {
        return (StatusCode::BAD_REQUEST, "unknown action").into_response();
    }

    // Try to stop locally (live pipeline or orphaned DB row).
    let local = {
        let mut mgr = state.recordings.write().await;
        if mgr.is_active(&id) {
            match mgr.stop(&id).await {
                Ok(s) => Some(s),
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
                }
            }
        } else {
            drop(mgr);
            match db::session_get(&state.db, &id).await {
                Ok(Some(row)) => {
                    let stopped_at = chrono::Utc::now().to_rfc3339();
                    if let Err(e) =
                        db::session_update_stop(&state.db, &id, &stopped_at, "stopped", None).await
                    {
                        error!(error = %e, "db stop orphaned session");
                    }
                    Some(RecordingSessionDto {
                        id: row.id,
                        source_id: row.source_id,
                        preset_id: row.preset_id,
                        started_at: row.started_at,
                        stopped_at: Some(stopped_at),
                        primary_path: row.primary_path,
                        secondary_path: row.secondary_path,
                        redundant_path: row.redundant_path,
                        status: "stopped".to_string(),
                        error_message: None,
                    })
                }
                Ok(None) => None,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
                }
            }
        }
    };

    if let Some(session) = local {
        if let Err(e) = recording::persist_stop(&state.db, &session).await {
            error!(error = %e, "persist session stop");
        }
        let event = if session.status == "error" {
            WsEvent::RecordingError {
                session_id: session.id.clone(),
                source_id: composite(&state.node_id, &session.source_id),
                error: session.error_message.clone().unwrap_or_default(),
            }
        } else {
            WsEvent::RecordingStopped {
                session_id: session.id.clone(),
                source_id: composite(&state.node_id, &session.source_id),
            }
        };
        ws::send(&state.ws_tx, &event);
        return Json(session_value(&session, &state.node_id)).into_response();
    }

    // Not ours — fan out to peers if we aggregate.
    if state.role.is_aggregator() {
        if let Some(body) = proxy::stop_recording(&state, &id, &req).await {
            return Json(body).into_response();
        }
    }

    (StatusCode::NOT_FOUND, "session not found").into_response()
}

// ── /api/v1/thumbnails/{source_id} ───────────────────────────────────────────

async fn get_thumbnail(
    State(state): State<Arc<AppState>>,
    AxumPath(source_id): AxumPath<String>,
) -> Response {
    let (node, local) = split_id(&source_id);

    if let Some(n) = node {
        if n != state.node_id {
            if state.role.is_aggregator() {
                return proxy::thumbnail(&state, n, local).await;
            }
            return (StatusCode::NOT_FOUND, "unknown node for source").into_response();
        }
    }

    let bytes = state.recordings.read().await.thumbnail_bytes(local);
    match bytes {
        Some(jpeg) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .body(Body::from(jpeg))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "no thumbnail yet").into_response(),
    }
}

// ── /api/v1/presets ───────────────────────────────────────────────────────────

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        "preset authoring requires the control station role",
    )
        .into_response()
}

async fn get_presets(State(state): State<Arc<AppState>>) -> Response {
    match db::presets_full_list(&state.db).await {
        Ok(rows) => {
            let dtos: Vec<PresetDto> = rows.iter().map(sync::preset_row_to_dto).collect();
            Json(dtos).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_preset(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PresetCreateRequest>,
) -> Response {
    if !state.role.is_aggregator() {
        return forbidden();
    }
    let now = chrono::Utc::now().to_rfc3339();
    let row = PresetRow {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        codec: req.codec,
        container: req.container,
        resolution: req.resolution,
        framerate: req.framerate,
        bitrate_kbps: req.bitrate_kbps,
        quality: req.quality,
        output_template: req.output_template,
        secondary_output_template: req.secondary_output_template,
        redundant_output_template: req.redundant_output_template,
        created_at: now.clone(),
        updated_at: now,
        version: 1,
    };

    if let Err(e) = db::preset_insert(&state.db, &row).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = sync::sync_presets_to_nodes(&state).await {
        error!(error = %e, "preset sync after create");
    }
    (StatusCode::CREATED, Json(sync::preset_row_to_dto(&row))).into_response()
}

async fn put_preset(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PresetCreateRequest>,
) -> Response {
    if !state.role.is_aggregator() {
        return forbidden();
    }
    let now = chrono::Utc::now().to_rfc3339();
    // created_at/version are ignored by preset_update (it bumps version itself).
    let row = PresetRow {
        id: id.clone(),
        name: req.name,
        codec: req.codec,
        container: req.container,
        resolution: req.resolution,
        framerate: req.framerate,
        bitrate_kbps: req.bitrate_kbps,
        quality: req.quality,
        output_template: req.output_template,
        secondary_output_template: req.secondary_output_template,
        redundant_output_template: req.redundant_output_template,
        created_at: String::new(),
        updated_at: now,
        version: 0,
    };

    match db::preset_update(&state.db, &row).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "preset not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    if let Err(e) = sync::sync_presets_to_nodes(&state).await {
        error!(error = %e, "preset sync after update");
    }
    match db::preset_get_full(&state.db, &id).await {
        Ok(Some(updated)) => Json(sync::preset_row_to_dto(&updated)).into_response(),
        _ => StatusCode::OK.into_response(),
    }
}

async fn delete_preset(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !state.role.is_aggregator() {
        return forbidden();
    }
    match db::preset_delete(&state.db, &id).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "preset not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    if let Err(e) = sync::sync_presets_to_nodes(&state).await {
        error!(error = %e, "preset sync after delete");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn post_presets_sync(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PresetSyncRequest>,
) -> Response {
    let rows: Vec<db::PresetCacheRow> = req
        .presets
        .into_iter()
        .map(|p| db::PresetCacheRow {
            id: p.id,
            name: p.name,
            data: p.data.to_string(),
            version: p.version,
            synced_at: p.synced_at,
        })
        .collect();

    match db::presets_replace(&state.db, &rows).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── /ws ───────────────────────────────────────────────────────────────────────

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let rx = state.ws_tx.subscribe();
    upgrade.on_upgrade(move |socket| ws::handle(socket, rx))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn session_row_to_dto(r: db::SessionRow) -> RecordingSessionDto {
    RecordingSessionDto {
        id: r.id,
        source_id: r.source_id,
        preset_id: r.preset_id,
        started_at: r.started_at,
        stopped_at: r.stopped_at,
        primary_path: r.primary_path,
        secondary_path: r.secondary_path,
        redundant_path: r.redundant_path,
        status: r.status,
        error_message: r.error_message,
    }
}

async fn build_profile_for_preset(state: &AppState, preset_id: &str) -> RecordingProfile {
    // Recordings resolve presets from the synced cache (full preset JSON in `data`).
    if let Ok(rows) = db::presets_list(&state.db).await {
        if let Some(row) = rows.into_iter().find(|r| r.id == preset_id) {
            if let Ok(p) = serde_json::from_str::<PresetDto>(&row.data) {
                return RecordingProfile::from_preset(
                    p.id,
                    p.name,
                    &p.codec,
                    &p.container,
                    p.resolution.as_deref(),
                    p.framerate.as_deref(),
                    p.bitrate_kbps.map(|b| b as u32),
                    p.quality,
                    p.output_template,
                );
            }
        }
    }
    // Built-in fallback (the always-available "default" preset, or anything
    // not yet synced).
    RecordingProfile::h264_mov(preset_id)
}
