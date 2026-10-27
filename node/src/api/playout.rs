//! Channel transport: a channel's playlist, cueing and play/pause/stop.
//! Part of the node API (`/api/v1/node/…`). Channels themselves are
//! configured sources, created and edited through `/configured-sources`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    routing::{get, post},
    Json, Router,
};
use gstreamer as gst;

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    ChannelStatusDto, CueRequest, LoadedClipDto, MediaItemDto, OutputStatusDto, PlaylistDto,
    PlaylistInput, PlaylistItemDto, TransportAction, TransportRequest, WsEvent,
};
use crate::db;
use crate::sources::channel::{ClipRequest, PlaylistItem, Playout, PlayoutStatus};
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/channels/{id}", get(get_channel))
        .route(
            "/channels/{id}/playlist",
            get(get_playlist).put(put_playlist),
        )
        .route("/channels/{id}/cue", post(post_cue))
        .route("/channels/{id}/play", post(post_play))
        .route("/channels/{id}/transport", post(post_transport))
}

/// A channel's transport, with its playlist read in.
pub(super) async fn playout(state: &AppState, id: &str) -> ApiResult<Arc<Playout>> {
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
pub(super) async fn reload_playlist(
    state: &AppState,
    id: &str,
    playout: &Arc<Playout>,
) -> ApiResult<()> {
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
                start: None,
                end: r.end_action,
                rate: 1.0,
                replay: None,
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
            rate: c.rate,
            replay_start_us: c.replay.as_ref().map(|r| r.span.base.useconds()),
            replay_clip_id: c.replay.as_ref().and_then(|r| r.clip_id.clone()),
            replay_source_id: c.replay.map(|r| r.source_id),
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

/// Answer an action on the channel: its status, sent to everyone whether or
/// not the action worked, or the action's error as a 400.
pub(super) async fn reply_to(
    state: &AppState,
    id: &str,
    playout: &Playout,
    result: anyhow::Result<()>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let reply = reply(state, id, playout).await;
    result.map_err(ApiError::bad)?;
    Ok(reply)
}

/// Answer with the channel's status, and tell everyone.
async fn reply(state: &AppState, id: &str, playout: &Playout) -> Json<ChannelStatusDto> {
    let status = channel_status(state, id, playout).await;
    state.emit(&WsEvent::ChannelState {
        source_id: id.to_string(),
        status: Box::new(status.clone()),
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
        let m = media
            .get(&item.media_id)
            .ok_or_else(|| ApiError::bad(format!("item {n}: not in the media library")))?;
        if let Some(duration) = m.info.duration_ms.filter(|&d| item.in_ms >= d) {
            return Err(ApiError::bad(format!(
                "item {n}: the in point is past the end ({duration} ms)"
            )));
        }
        if item.out_ms.is_some_and(|out| out <= item.in_ms) {
            return Err(ApiError::bad(format!(
                "item {n}: the out point must be after the in point"
            )));
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

/// Cue a playlist item: loaded, paused at its in point, showing its first
/// frame.
async fn post_cue(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<CueRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let result = playout.cue_item(&req.item_id).await;
    reply_to(&state, &id, &playout, result).await
}

async fn post_play(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<CueRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let result = playout.play_item(&req.item_id).await;
    reply_to(&state, &id, &playout, result).await
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
                .ok_or_else(|| ApiError::bad("position_ms is required"))?;
            playout.seek(gst::ClockTime::from_mseconds(position)).await
        }
        TransportAction::Next => playout.next().await,
        TransportAction::Speed => {
            let rate = req.rate.ok_or_else(|| ApiError::bad("rate is required"))?;
            playout.set_rate(rate).await
        }
        TransportAction::Step => playout.step(req.frames.unwrap_or(1)).await,
    };
    reply_to(&state, &id, &playout, result).await
}
