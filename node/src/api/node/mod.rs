use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use tracing::error;

use crate::api::types::{
    PatchRecordingRequest, PresetSyncRequest, RecordingSessionDto, SourceDto,
    SourceCapabilitiesDto, TimecodeDto, StartRecordingRequest, WsEvent,
};
use crate::db;
use crate::pipeline::profile::RecordingProfile;
use crate::recording;
use crate::sources::Timecode;
use crate::state::AppState;
use crate::ws;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/sources", get(get_sources))
        .route("/api/v1/sources/{id}", get(get_source))
        .route("/api/v1/sources/scan", post(post_scan))
        .route("/api/v1/recordings", get(get_recordings).post(post_recording))
        .route("/api/v1/recordings/{id}", get(get_recording).patch(patch_recording))
        .route("/api/v1/thumbnails/{source_id}", get(get_thumbnail))
        .route("/api/v1/presets", get(get_presets))
        .route("/api/v1/presets/sync", post(post_presets_sync))
        .route("/ws", get(ws_handler))
}

// ── /api/v1/status ────────────────────────────────────────────────────────────

async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    Json(crate::api::types::NodeStatus {
        id: state.node_id.clone(),
        name: state.node_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: uptime,
        mode: "node".to_string(),
    })
}

// ── /api/v1/sources ───────────────────────────────────────────────────────────

async fn get_sources(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let sources = state.sources.read().await;
    let dtos: Vec<SourceDto> = sources
        .sources()
        .iter()
        .map(|s| source_to_dto(s.as_ref()))
        .collect();
    Json(dtos)
}

async fn get_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let sources = state.sources.read().await;
    match sources.get(&id) {
        Some(s) => Json(source_to_dto(s)).into_response(),
        None => (StatusCode::NOT_FOUND, "source not found").into_response(),
    }
}

async fn post_scan(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut sources = state.sources.write().await;
    match sources.scan() {
        Ok(()) => {
            let dtos: Vec<SourceDto> = sources
                .sources()
                .iter()
                .map(|s| source_to_dto(s.as_ref()))
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
    // Merge active (in-memory) with recent historical (DB).
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

    let mut all = active;
    all.extend(historical);
    Json(all).into_response()
}

async fn get_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    // Check active first
    {
        let mgr = state.recordings.read().await;
        if let Some(s) = mgr.active_sessions().into_iter().find(|s| s.id == id) {
            return Json(s.clone()).into_response();
        }
    }
    // Fall back to DB
    match db::session_get(&state.db, &id).await {
        Ok(Some(row)) => Json(session_row_to_dto(row)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_recording(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRecordingRequest>,
) -> Response {
    // Look up a cached preset to build a RecordingProfile.
    // For now we build a default h264/mov profile using the preset_id as the name.
    // When the presets layer is complete, we'll decode the cached JSON blob.
    let profile = build_profile_for_preset(&state, &req.preset_id).await;

    let primary_path = req.primary_path.unwrap_or_else(|| {
        let dt = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        format!("/tmp/capture-room/{}_{}_{}.mov", req.source_id, dt, req.preset_id)
    });

    // Ensure the output directory exists
    if let Some(parent) = Path::new(&primary_path).parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    let session = {
        let sources = state.sources.read().await;
        let mut mgr = state.recordings.write().await;
        match mgr.start(&sources, &req.source_id, &profile, Path::new(&primary_path)) {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    // Persist to DB
    if let Err(e) = recording::persist_start(&state.db, &session).await {
        error!(error = %e, "persist session start");
    }

    // Broadcast event
    ws::send(
        &state.ws_tx,
        &WsEvent::RecordingStarted {
            session_id: session.id.clone(),
            source_id: session.source_id.clone(),
        },
    );

    (StatusCode::CREATED, Json(session)).into_response()
}

async fn patch_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PatchRecordingRequest>,
) -> Response {
    if req.action != "stop" {
        return (StatusCode::BAD_REQUEST, "unknown action").into_response();
    }

    let session = {
        let mut mgr = state.recordings.write().await;
        match mgr.stop(&id).await {
            Ok(s) => s,
            Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
        }
    };

    if let Err(e) = recording::persist_stop(&state.db, &session).await {
        error!(error = %e, "persist session stop");
    }

    let event = if session.status == "error" {
        WsEvent::RecordingError {
            session_id: session.id.clone(),
            source_id: session.source_id.clone(),
            error: session.error_message.clone().unwrap_or_default(),
        }
    } else {
        WsEvent::RecordingStopped {
            session_id: session.id.clone(),
            source_id: session.source_id.clone(),
        }
    };
    ws::send(&state.ws_tx, &event);

    Json(session).into_response()
}

// ── /api/v1/thumbnails/{source_id} ───────────────────────────────────────────

async fn get_thumbnail(
    State(state): State<Arc<AppState>>,
    AxumPath(source_id): AxumPath<String>,
) -> Response {
    let bytes = state.recordings.read().await.thumbnail_bytes(&source_id);
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
                    data: serde_json::from_str(&r.data).unwrap_or(serde_json::Value::Null),
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
    // Try to load from cached presets DB.  If not found, fall back to a default.
    if let Ok(rows) = db::presets_list(&state.db).await {
        if let Some(row) = rows.into_iter().find(|r| r.id == preset_id) {
            // Attempt to decode the JSON blob into a RecordingProfile.
            // The controller serialises profiles into the cache; for now we
            // look for a known "codec" field to pick the right variant.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&row.data) {
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
