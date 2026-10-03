//! The node API: everything this machine has — sources, storage, recordings,
//! thumbnails, settings. Local only: it never forwards and never knows about
//! other nodes. Mounted at `/api/v1/node`; controllers reach a peer's copy
//! through `/api/v1/nodes/{id}/…`.

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
    CreateTestSourceRequest, MonitorSettingsDto, NodeSettingsDto, NodeStatus,
    RecordingSessionDto, SourceCapabilitiesDto, SourceDto, StartRecordingRequest,
    TestSourceConfigDto, TimecodeDto, UpdateNodeSettingsRequest, UpdateTestSourceRequest, WsEvent,
};
use crate::db;
use crate::pipeline::monitor::MonitorConfig;
use crate::pipeline::profile::RecordingProfile;
use crate::recording;
use crate::sources::manager::{SourceManager, StopOutcome, StopResult};
use crate::sources::{InputSource, Timecode};
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
        .route("/sources/{id}/connect", post(post_connect))
        .route("/sources/{id}/disconnect", post(post_disconnect))
        .route("/test-sources", get(get_test_configs).post(post_test_config))
        .route("/test-sources/{id}", axum::routing::put(put_test_config).delete(delete_test_config))
        .route("/recordings", get(get_recordings).post(post_recording))
        .route("/recordings/{id}", get(get_recording))
        .route("/recordings/{id}/stop", post(post_stop_recording))
        .route("/thumbnails/{source_id}", get(get_thumbnail))
        .route("/ws", get(ws_handler))
}

fn internal(e: impl ToString) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

// ── /api/v1/status ────────────────────────────────────────────────────────────

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
    let monitor = {
        let mgr = state.source_manager.read().await;
        let mc = mgr.monitor_config();
        MonitorSettingsDto {
            thumb_fps: mc.thumb_fps_num,
            thumb_width: mc.thumb_width,
            thumb_height: mc.thumb_height,
            level_interval_ms: mc.level_interval_ns / 1_000_000,
        }
    };
    NodeSettingsDto {
        node_id: state.node_id.clone(),
        node_name: state.node_name(),
        is_controller: state.is_controller().await,
        monitor,
    }
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<NodeSettingsDto> {
    Json(settings_dto(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateNodeSettingsRequest>,
) -> Response {
    if let Some(name) = req.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        if let Err(e) = db::config_set(&state.db, "name", &name).await {
            return internal(e);
        }
        *state.node_name.write().unwrap() = name;
    }
    if let Some(m) = req.monitor {
        if let Err(e) = apply_monitor_settings(&state, m).await {
            return internal(e);
        }
    }
    Json(settings_dto(&state).await).into_response()
}

async fn apply_monitor_settings(state: &AppState, req: MonitorSettingsDto) -> anyhow::Result<()> {
    // Clamp to reasonable ranges.
    let thumb_fps = req.thumb_fps.clamp(1, 30);
    let thumb_width = req.thumb_width.clamp(160, 1920);
    let thumb_height = req.thumb_height.clamp(90, 1080);
    let level_ms = req.level_interval_ms.clamp(50, 1000);

    db::config_set(&state.db, "monitor_thumb_fps", &thumb_fps.to_string()).await?;
    db::config_set(&state.db, "monitor_thumb_width", &thumb_width.to_string()).await?;
    db::config_set(&state.db, "monitor_thumb_height", &thumb_height.to_string()).await?;
    db::config_set(&state.db, "monitor_level_ms", &level_ms.to_string()).await?;

    state.source_manager.write().await.apply_monitor_config(MonitorConfig {
        thumb_fps_num: thumb_fps,
        thumb_fps_den: 1,
        thumb_width,
        thumb_height,
        level_interval_ns: level_ms * 1_000_000,
    });
    Ok(())
}

// ── /storage ──────────────────────────────────────────────────────────────────

async fn get_storage() -> Response {
    match tokio::task::spawn_blocking(crate::storage::list_volumes).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

// ── /sources ──────────────────────────────────────────────────────────────────

fn sources_list(mgr: &SourceManager) -> Vec<SourceDto> {
    mgr.sources().iter().map(|s| source_to_dto(mgr, s.as_ref())).collect()
}

async fn get_sources(State(state): State<Arc<AppState>>) -> Json<Vec<SourceDto>> {
    let mgr = state.source_manager.read().await;
    Json(sources_list(&mgr))
}

async fn get_source(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let mgr = state.source_manager.read().await;
    match mgr.get_source(&id) {
        Some(s) => Json(source_to_dto(&mgr, s)).into_response(),
        None => (StatusCode::NOT_FOUND, "source not found").into_response(),
    }
}

async fn post_scan(State(state): State<Arc<AppState>>) -> Response {
    if let Err(e) = rebuild_sources(&state).await {
        return internal(e);
    }
    let mgr = state.source_manager.read().await;
    Json(sources_list(&mgr)).into_response()
}

async fn post_connect(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let mut mgr = state.source_manager.write().await;
    match mgr.connect(&id) {
        Ok(()) => Json(mgr.get_source(&id).map(|s| source_to_dto(&mgr, s))).into_response(),
        Err(e) => internal(e),
    }
}

async fn post_disconnect(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let mut mgr = state.source_manager.write().await;
    if let Some(teardown) = mgr.disconnect(&id) {
        recording::spawn_teardowns(&state, vec![teardown]);
    }
    Json(mgr.get_source(&id).map(|s| source_to_dto(&mgr, s))).into_response()
}

// ── /test-sources ─────────────────────────────────────────────────────────────

async fn get_test_configs(State(state): State<Arc<AppState>>) -> Response {
    match db::test_sources_list(&state.db).await {
        Ok(rows) => {
            Json(rows.into_iter().map(row_to_config_dto).collect::<Vec<_>>()).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_test_config(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTestSourceRequest>,
) -> Response {
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
    if let Err(e) = rebuild_sources(&state).await {
        error!(error = %e, "rebuild sources after create");
    }
    (StatusCode::CREATED, Json(row_to_config_dto(row))).into_response()
}

async fn put_test_config(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UpdateTestSourceRequest>,
) -> Response {
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
    if let Err(e) = rebuild_sources(&state).await {
        error!(error = %e, "rebuild sources after update");
    }
    Json(row_to_config_dto(row)).into_response()
}

async fn delete_test_config(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match db::test_source_delete(&state.db, &id).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "test source not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
    if let Err(e) = rebuild_sources(&state).await {
        error!(error = %e, "rebuild sources after delete");
    }
    StatusCode::NO_CONTENT.into_response()
}

// ── /recordings ───────────────────────────────────────────────────────────────

async fn get_recordings(State(state): State<Arc<AppState>>) -> Response {
    let active: Vec<RecordingSessionDto> = {
        let mgr = state.source_manager.read().await;
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

    let all: Vec<RecordingSessionDto> = active.into_iter().chain(historical).collect();
    Json(all).into_response()
}

async fn get_recording(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    {
        let mgr = state.source_manager.read().await;
        if let Some(s) = mgr.active_sessions().into_iter().find(|s| s.id == id) {
            return Json(s.clone()).into_response();
        }
    }
    match db::session_get(&state.db, &id).await {
        Ok(Some(row)) => Json(session_row_to_dto(row)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => internal(e),
    }
}

async fn post_recording(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRecordingRequest>,
) -> Response {
    if req.outputs.is_empty() {
        return (StatusCode::BAD_REQUEST, "at least one output is required").into_response();
    }
    let legs = build_legs(&state, &req);

    for (path, _) in &legs {
        if let Some(parent) = Path::new(path).parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return internal(format!("create {}: {e}", parent.display()));
            }
        }
    }

    let preset_id = req.preset_id.clone().unwrap_or_default();
    let session = {
        let mut mgr = state.source_manager.write().await;
        match mgr.start_recording(&req.source_id, &preset_id, &legs) {
            Ok(s) => s,
            Err(e) => return internal(e),
        }
    };

    if let Err(e) = recording::persist_start(&state.db, &session).await {
        error!(error = %e, "persist session start");
    }

    state.emit(&WsEvent::RecordingStarted {
        session_id: session.id.clone(),
        source_id: session.source_id.clone(),
    });

    (StatusCode::CREATED, Json(session)).into_response()
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
) -> Response {
    let outcome = state.source_manager.write().await.begin_stop_recording(&id);

    let local = match outcome {
        StopOutcome::Start(job) => {
            let rx = job.tx.subscribe();
            // Spawned independently: if the caller's connection drops while
            // we're awaiting below, only this request's response is affected
            // — the detach work keeps running to completion regardless.
            tokio::spawn(recording::run_stop(Arc::clone(&state), job));
            await_stop_result(rx).await
        }
        StopOutcome::Join(rx) => await_stop_result(rx).await,
        StopOutcome::NotFound => {
            // Orphaned DB row (e.g. after a crash/restart) — mark stopped
            // directly, but only if it isn't already; a stray retry landing
            // here after everything settled shouldn't stomp stopped_at.
            match db::session_get(&state.db, &id).await {
                Ok(Some(row)) if row.status == "active" => {
                    let stopped_at = chrono::Utc::now().to_rfc3339();
                    if let Err(e) =
                        db::session_update_stop(&state.db, &id, &stopped_at, "stopped", None).await
                    {
                        error!(error = %e, "db stop orphaned session");
                    }
                    Some(session_row_to_dto(db::SessionRow {
                        stopped_at: Some(stopped_at),
                        status: "stopped".to_string(),
                        error_message: None,
                        ..row
                    }))
                }
                Ok(Some(row)) => Some(session_row_to_dto(row)),
                Ok(None) => None,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
                }
            }
        }
    };

    match local {
        Some(session) => Json(session).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
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
    let caps = s.capabilities();
    SourceDto {
        id: s.id().to_string(),
        display_name: s.display_name().to_string(),
        source_type: format!("{:?}", s.source_type()).to_lowercase(),
        is_available: s.is_available(),
        connected: mgr.is_monitored(s.id()),
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

fn session_row_to_dto(r: db::SessionRow) -> RecordingSessionDto {
    let output_paths: Vec<String> =
        serde_json::from_str(&r.output_paths).unwrap_or_default();
    RecordingSessionDto {
        id: r.id,
        source_id: r.source_id,
        preset_id: r.preset_id,
        started_at: r.started_at,
        stopped_at: r.stopped_at,
        output_paths,
        status: r.status,
        error_message: r.error_message,
    }
}

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

async fn rebuild_sources(state: &Arc<AppState>) -> anyhow::Result<()> {
    let configs = db::test_sources_list(&state.db)
        .await?
        .into_iter()
        .map(db_row_to_config)
        .collect::<Vec<_>>();
    let teardowns = state.source_manager.write().await.scan(&configs);
    recording::spawn_teardowns(state, teardowns);
    Ok(())
}

/// Build `(resolved_path, RecordingProfile)` for every requested output leg.
fn build_legs(state: &AppState, req: &StartRecordingRequest) -> Vec<(String, RecordingProfile)> {
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let datetime = now.format("%Y%m%d_%H%M%S").to_string();
    let node = state.node_name();

    req.outputs
        .iter()
        .map(|o| {
            let profile = RecordingProfile::from_preset(
                &o.codec,
                &o.container,
                o.resolution.as_deref(),
                o.framerate.as_deref(),
                o.bitrate_kbps.map(|b| b as u32),
            );
            let path = o
                .path_template
                .replace("{source}", &req.source_id)
                .replace("{node}", &node)
                .replace("{date}", &date)
                .replace("{datetime}", &datetime)
                .replace("{output}", &o.name)
                .replace("{ext}", profile.file_extension());
            (path, profile)
        })
        .collect()
}
