use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path as AxumPath, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::error;

use crate::api::types::{
    PatchRecordingRequest, PresetSyncRequest, RecordingSessionDto, SourceCapabilitiesDto,
    SourceDto, StartRecordingRequest, TimecodeDto, WsEvent,
};
use crate::controller::proxy;
use crate::db;
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
        .route("/api/v1/sources", get(get_sources))
        .route("/api/v1/sources/{id}", get(get_source))
        .route("/api/v1/sources/scan", post(post_scan))
        .route("/api/v1/recordings", get(get_recordings).post(post_recording))
        .route(
            "/api/v1/recordings/{id}",
            get(get_recording).patch(patch_recording),
        )
        .route("/api/v1/thumbnails/{source_id}", get(get_thumbnail))
        .route("/api/v1/presets", get(get_presets))
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
    let mut sources = state.sources.write().await;
    match sources.scan() {
        Ok(()) => {
            let dtos: Vec<Value> = sources
                .sources()
                .iter()
                .map(|s| source_value(s.as_ref(), &state.node_id))
                .collect();
            Json(dtos).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn source_to_dto(s: &dyn crate::sources::InputSource) -> SourceDto {
    let caps = s.capabilities();
    SourceDto {
        id: s.id().to_string(),
        display_name: s.display_name().to_string(),
        source_type: format!("{:?}", s.source_type()).to_lowercase(),
        is_available: s.is_available(),
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

async fn get_presets(State(state): State<Arc<AppState>>) -> Response {
    match db::presets_list(&state.db).await {
        Ok(rows) => {
            let dtos: Vec<crate::api::types::PresetCacheDto> = rows
                .into_iter()
                .map(|r| crate::api::types::PresetCacheDto {
                    id: r.id,
                    name: r.name,
                    data: serde_json::from_str(&r.data).unwrap_or(Value::Null),
                    version: r.version,
                    synced_at: r.synced_at,
                })
                .collect();
            Json(dtos).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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
    if let Ok(rows) = db::presets_list(&state.db).await {
        if let Some(row) = rows.into_iter().find(|r| r.id == preset_id) {
            if let Ok(v) = serde_json::from_str::<Value>(&row.data) {
                let codec = v["codec"].as_str().unwrap_or("h264").to_lowercase();
                return match codec.as_str() {
                    "h264" => RecordingProfile::h264_mov(&row.id),
                    _ => RecordingProfile::h264_mov(&row.id),
                };
            }
        }
    }
    RecordingProfile::h264_mov(preset_id)
}
