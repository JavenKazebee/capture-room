//! Playout: the media library and channel transport. Part of the node API
//! (`/api/v1/node/…`). Channels themselves are configured sources, created
//! and edited through `/configured-sources`.

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use gstreamer as gst;

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    AddMediaRequest, ChannelStatusDto, LoadClipRequest, LoadedClipDto, MediaItemDto, MediaOrigin,
    OutputStatusDto, TransportAction, TransportRequest, WsEvent,
};
use crate::db;
use crate::sources::channel::{ClipRequest, Playout, PlayoutStatus};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/media", get(get_media).post(post_media))
        .route("/media/{id}", axum::routing::delete(delete_media))
        .route("/channels/{id}", get(get_channel))
        .route("/channels/{id}/load", post(post_load))
        .route("/channels/{id}/transport", post(post_transport))
}

// ── /media ────────────────────────────────────────────────────────────────────

const MEDIA_NOT_FOUND: ApiError = ApiError::NotFound("media not found");

async fn get_media(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<MediaItemDto>>> {
    let mut items = db::media_list(&state.db).await?;
    for item in &mut items {
        item.missing = !Path::new(&item.path).is_file();
    }
    Ok(Json(items))
}

async fn post_media(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddMediaRequest>,
) -> ApiResult<(StatusCode, Json<MediaItemDto>)> {
    let path = req.path.trim().to_string();
    if let Some(session_id) = &req.session_id {
        let session = db::session_get(&state.db, session_id)
            .await?
            .ok_or(ApiError::NotFound("session not found"))?;
        if !session.files.iter().flatten().any(|f| *f == path) {
            return Err(ApiError::BadRequest(
                "the file isn't one of the session's".into(),
            ));
        }
    }
    let probe_path = path.clone();
    let info = tokio::task::spawn_blocking(move || crate::sources::file::probe(&probe_path))
        .await?
        .map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
    let name = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| {
            Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| path.clone());
    let item = MediaItemDto {
        id: uuid::Uuid::new_v4().to_string(),
        path,
        name,
        info,
        origin: if req.session_id.is_some() {
            MediaOrigin::Recording
        } else {
            MediaOrigin::Import
        },
        session_id: req.session_id,
        added_at: chrono::Utc::now().to_rfc3339(),
        missing: false,
    };
    if !db::media_insert(&state.db, &item).await? {
        return Err(ApiError::Conflict("that file is already in the library"));
    }
    state.emit(&WsEvent::MediaUpdated);
    Ok((StatusCode::CREATED, Json(item)))
}

/// Remove an entry from the library. The file stays on disk.
async fn delete_media(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    if !db::media_delete(&state.db, &id).await? {
        return Err(MEDIA_NOT_FOUND);
    }
    state.emit(&WsEvent::MediaUpdated);
    Ok(StatusCode::NO_CONTENT)
}

// ── /channels ─────────────────────────────────────────────────────────────────

async fn playout(state: &AppState, id: &str) -> ApiResult<Arc<Playout>> {
    state
        .source_manager
        .read()
        .await
        .get_source(id)
        .and_then(|s| s.playout())
        .cloned()
        .ok_or(ApiError::NotFound("channel not found"))
}

async fn channel_status(state: &AppState, id: &str, playout: &Playout) -> ChannelStatusDto {
    let outputs = state.source_manager.read().await.output_status(id);
    status_dto(playout.status(), outputs)
}

pub fn status_dto(status: PlayoutStatus, outputs: Vec<OutputStatusDto>) -> ChannelStatusDto {
    ChannelStatusDto {
        state: status.state,
        clip: status.clip.map(|c| LoadedClipDto {
            media_id: c.media_id,
            name: c.name,
            in_ms: c.in_point.mseconds(),
            out_ms: c.out_point.map(|t| t.mseconds()),
            end: c.end,
        }),
        position_ms: status.position.map(|t| t.mseconds()),
        duration_ms: status.duration.map(|t| t.mseconds()),
        error: status.error,
        outputs,
    }
}

/// Answer with the channel's status, and tell everyone.
async fn reply(state: &AppState, id: &str, playout: &Playout) -> Json<ChannelStatusDto> {
    let status = channel_status(state, id, playout).await;
    state.emit(&WsEvent::ChannelState {
        source_id: id.to_string(),
        status: status.clone(),
    });
    Json(status)
}

async fn get_channel(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    Ok(Json(channel_status(&state, &id, &playout).await))
}

async fn post_load(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<LoadClipRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let media = db::media_get(&state.db, &req.media_id)
        .await?
        .ok_or(MEDIA_NOT_FOUND)?;
    if !Path::new(&media.path).is_file() {
        return Err(ApiError::BadRequest(
            format!("{} is no longer on disk", media.path).into(),
        ));
    }
    let in_ms = req.in_ms.unwrap_or(0);
    if let Some(duration) = media.info.duration_ms.filter(|&d| in_ms >= d) {
        return Err(ApiError::BadRequest(
            format!("the in point is past the end ({duration} ms)").into(),
        ));
    }
    if req.out_ms.is_some_and(|out| out <= in_ms) {
        return Err(ApiError::BadRequest(
            "the out point must be after the in point".into(),
        ));
    }
    let clip = ClipRequest {
        media_id: media.id,
        name: media.name,
        path: media.path,
        in_point: gst::ClockTime::from_mseconds(in_ms),
        out_point: req.out_ms.map(gst::ClockTime::from_mseconds),
        end: req.end,
    };
    let result = playout.load(clip).await;
    let reply = reply(&state, &id, &playout).await;
    result.map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
    Ok(reply)
}

async fn post_transport(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<TransportRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let result = match req.action {
        TransportAction::Play => playout.play().await,
        TransportAction::Pause => playout.pause().await,
        TransportAction::Stop => {
            playout.stop().await;
            Ok(())
        }
        TransportAction::Seek => {
            let position = req
                .position_ms
                .ok_or(ApiError::BadRequest("position_ms is required".into()))?;
            playout.seek(gst::ClockTime::from_mseconds(position)).await
        }
    };
    let reply = reply(&state, &id, &playout).await;
    result.map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
    Ok(reply)
}
