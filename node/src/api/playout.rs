//! Playout: the media library and channel transport. Part of the node API
//! (`/api/v1/node/…`). Channels themselves are configured sources, created
//! and edited through `/configured-sources`.

use std::collections::HashMap;
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
    AddMediaRequest, ChannelStatusDto, CueRequest, LoadedClipDto, MediaItemDto, MediaOrigin,
    OutputStatusDto, PlaylistDto, PlaylistInput, PlaylistItemDto, TransportAction,
    TransportRequest, WsEvent,
};
use crate::db;
use crate::sources::channel::{ClipRequest, PlaylistItem, Playout, PlayoutStatus};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/media", get(get_media).post(post_media))
        .route("/media/{id}", axum::routing::delete(delete_media))
        .route("/channels/{id}", get(get_channel))
        .route(
            "/channels/{id}/playlist",
            get(get_playlist).put(put_playlist),
        )
        .route("/channels/{id}/cue", post(post_cue))
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
    // Its playlist items go with it.
    let channels = db::playlists_using(&state.db, &id).await?;
    if !db::media_delete(&state.db, &id).await? {
        return Err(MEDIA_NOT_FOUND);
    }
    for channel in channels {
        if let Ok(playout) = playout(&state, &channel).await {
            reload_playlist(&state, &channel, &playout).await?;
        }
    }
    state.emit(&WsEvent::MediaUpdated);
    Ok(StatusCode::NO_CONTENT)
}

// ── /channels ─────────────────────────────────────────────────────────────────

/// A channel's transport, with its playlist read in.
async fn playout(state: &AppState, id: &str) -> ApiResult<Arc<Playout>> {
    let playout = state
        .source_manager
        .read()
        .await
        .get_source(id)
        .and_then(|s| s.playout())
        .cloned()
        .ok_or(ApiError::NotFound("channel not found"))?;
    if !playout.playlist_loaded() {
        reload_playlist(state, id, &playout).await?;
    }
    Ok(playout)
}

/// Hand a channel's stored playlist to its transport.
async fn reload_playlist(state: &AppState, id: &str, playout: &Arc<Playout>) -> ApiResult<()> {
    let (rows, loop_playlist) = db::playlist_get(&state.db, id).await?;
    let items = rows
        .into_iter()
        .map(|r| PlaylistItem {
            id: r.id,
            clip: ClipRequest {
                media_id: r.media_id,
                name: r.name,
                path: r.path,
                in_point: gst::ClockTime::from_mseconds(r.in_ms as u64),
                out_point: r.out_ms.map(|v| gst::ClockTime::from_mseconds(v as u64)),
                end: r.end_action,
            },
        })
        .collect();
    playout.set_playlist(items, loop_playlist).await;
    Ok(())
}

fn playlist_dto(playout: &Playout, durations: &HashMap<String, Option<u64>>) -> PlaylistDto {
    let (items, loop_playlist, rev) = playout.playlist();
    PlaylistDto {
        items: items
            .into_iter()
            .map(|i| PlaylistItemDto {
                missing: !Path::new(&i.clip.path).is_file(),
                duration_ms: durations.get(&i.clip.media_id).copied().flatten(),
                id: i.id,
                media_id: i.clip.media_id,
                name: i.clip.name,
                in_ms: i.clip.in_point.mseconds(),
                out_ms: i.clip.out_point.map(|t| t.mseconds()),
                end: i.clip.end,
            })
            .collect(),
        loop_playlist,
        rev,
    }
}

async fn media_durations(state: &AppState) -> ApiResult<HashMap<String, Option<u64>>> {
    Ok(db::media_list(&state.db)
        .await?
        .into_iter()
        .map(|m| (m.id, m.info.duration_ms))
        .collect())
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
        item_id: status.item_id,
        next_id: status.next_id,
        next_ready: status.next_ready,
        playlist_rev: status.playlist_rev,
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

async fn get_playlist(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<PlaylistDto>> {
    let playout = playout(&state, &id).await?;
    Ok(Json(playlist_dto(
        &playout,
        &media_durations(&state).await?,
    )))
}

/// Replace a channel's playlist with the list given, in order.
async fn put_playlist(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PlaylistInput>,
) -> ApiResult<Json<PlaylistDto>> {
    let playout = playout(&state, &id).await?;
    let media: HashMap<String, MediaItemDto> = db::media_list(&state.db)
        .await?
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let mut rows = Vec::with_capacity(req.items.len());
    for (n, item) in req.items.into_iter().enumerate() {
        let n = n + 1;
        let m = media.get(&item.media_id).ok_or_else(|| {
            ApiError::BadRequest(format!("item {n}: not in the media library").into())
        })?;
        if let Some(duration) = m.info.duration_ms.filter(|&d| item.in_ms >= d) {
            return Err(ApiError::BadRequest(
                format!("item {n}: the in point is past the end ({duration} ms)").into(),
            ));
        }
        if item.out_ms.is_some_and(|out| out <= item.in_ms) {
            return Err(ApiError::BadRequest(
                format!("item {n}: the out point must be after the in point").into(),
            ));
        }
        let id = item
            .id
            .filter(|i| !i.is_empty() && !rows.iter().any(|r: &(String, _, _, _, _)| &r.0 == i))
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        rows.push((id, item.media_id, item.in_ms, item.out_ms, item.end));
    }
    db::playlist_set(&state.db, &id, &rows, req.loop_playlist).await?;
    reload_playlist(&state, &id, &playout).await?;
    // The next item may have changed.
    let _ = reply(&state, &id, &playout).await;
    let durations = media
        .into_iter()
        .map(|(id, m)| (id, m.info.duration_ms))
        .collect();
    Ok(Json(playlist_dto(&playout, &durations)))
}

/// Cue a playlist item: loaded, paused at its in point.
async fn post_cue(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<CueRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let result = playout.cue_item(&req.item_id).await;
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
        TransportAction::Next => playout.next().await,
    };
    let reply = reply(&state, &id, &playout).await;
    result.map_err(|e| ApiError::BadRequest(format!("{e:#}").into()))?;
    Ok(reply)
}
