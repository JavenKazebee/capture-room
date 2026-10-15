//! Playout: the media library, channel transport and replay buffers. Part
//! of the node API (`/api/v1/node/…`). Channels themselves are configured
//! sources, created and edited through `/configured-sources`.

use std::collections::{HashMap, HashSet};
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
    AddMediaRequest, ChannelStatusDto, ClipEnd, CueRequest, LoadedClipDto, MediaItemDto,
    MediaOrigin, OutputStatusDto, PlayReplayClipRequest, PlaylistDto, PlaylistInput,
    PlaylistItemDto, ReplayBufferDto, ReplayClipDto, ReplayClipRequest, ReplayConfigInput,
    SaveReplayClipRequest, TransportAction, TransportRequest, UpdateReplayClipRequest, WsEvent,
};
use crate::benchmark;
use crate::db::{self, ReplayClipRow};
use crate::pipeline::replay::{self, ReplaySpan};
use crate::sources::channel::{
    check_rate, ClipRequest, PlaylistItem, Playout, PlayoutStatus, ReplayClip,
};
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
        .route("/channels/{id}/play", post(post_play))
        .route("/channels/{id}/transport", post(post_transport))
        .route("/channels/{id}/replay", post(post_replay))
        .route("/channels/{id}/replay-clip", post(post_replay_clip))
        .route("/replay", get(get_replay))
        .route("/replay/clips", get(get_clips).post(post_clip))
        .route(
            "/replay/clips/{id}",
            axum::routing::patch(patch_clip).delete(delete_clip),
        )
        .route("/replay/clips/{id}/export", post(post_clip_export))
        .route(
            "/replay/{source_id}",
            axum::routing::put(put_replay).delete(delete_replay),
        )
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
        if !super::node::session_files(&session)
            .iter()
            .flatten()
            .any(|f| *f == path)
        {
            return Err(ApiError::BadRequest(
                "the file isn't one of the session's".into(),
            ));
        }
    }
    let info = super::probe(path.clone()).await?;
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
async fn reply_to(
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

/// Play a playlist item now.
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
                .ok_or(ApiError::BadRequest("position_ms is required".into()))?;
            playout.seek(gst::ClockTime::from_mseconds(position)).await
        }
        TransportAction::Next => playout.next().await,
        TransportAction::Speed => {
            let rate = req
                .rate
                .ok_or(ApiError::BadRequest("rate is required".into()))?;
            playout.set_rate(rate).await
        }
        TransportAction::Step => playout.step(req.frames.unwrap_or(1)).await,
    };
    reply_to(&state, &id, &playout, result).await
}

// ── /replay ───────────────────────────────────────────────────────────────────

/// The longest replay buffer allowed, in minutes.
const MAX_REPLAY_MINUTES: u32 = 24 * 60;

async fn get_replay(State(state): State<Arc<AppState>>) -> Json<Vec<ReplayBufferDto>> {
    let utc = state.clock.utc_offset_us();
    Json(state.source_manager.read().await.replay_buffers(utc))
}

/// Give a source a replay buffer, or change its codec or length. A new
/// codec starts it afresh.
async fn put_replay(
    State(state): State<Arc<AppState>>,
    AxumPath(source_id): AxumPath<String>,
    Json(req): Json<ReplayConfigInput>,
) -> ApiResult<Json<ReplayBufferDto>> {
    if !(1..=MAX_REPLAY_MINUTES).contains(&req.minutes) {
        return Err(ApiError::BadRequest(
            format!("a replay buffer keeps 1 to {MAX_REPLAY_MINUTES} minutes").into(),
        ));
    }
    if !req.codec.available() {
        return Err(ApiError::BadRequest(
            format!(
                "this node can't record {:?} for replay (missing {})",
                req.codec,
                missing_elements(req.codec).join(", ")
            )
            .into(),
        ));
    }
    if state
        .source_manager
        .read()
        .await
        .get_source(&source_id)
        .is_none()
    {
        return Err(ApiError::NotFound("source not found"));
    }
    db::replay_buffer_set(&state.db, &source_id, &req).await?;
    // Like a recording, a buffer would compete with a benchmark and skew it.
    if benchmark::cancel(
        &state,
        None,
        "Cancelled: a replay buffer started on this node.",
    )
    .await
    {
        tracing::info!("benchmark cancelled for a replay buffer");
    }
    let utc = state.clock.utc_offset_us();
    let mut mgr = state.source_manager.write().await;
    mgr.set_replay(&source_id, Some(req));
    mgr.replay_buffers(utc)
        .into_iter()
        .find(|r| r.source_id == source_id)
        .map(Json)
        .ok_or(ApiError::NotFound("source not found"))
}

/// Remove a source's replay buffer, deleting what it recorded.
async fn delete_replay(
    State(state): State<Arc<AppState>>,
    AxumPath(source_id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    if !db::replay_buffer_delete(&state.db, &source_id).await? {
        return Err(ApiError::NotFound("no replay buffer for that source"));
    }
    state
        .source_manager
        .write()
        .await
        .set_replay(&source_id, None);
    Ok(StatusCode::NO_CONTENT)
}

fn missing_elements(codec: crate::api::types::ReplayCodec) -> Vec<&'static str> {
    let missing = |e: &&str| gst::ElementFactory::find(e).is_none();
    let mut list: Vec<&'static str> = codec.decoder().iter().copied().filter(missing).collect();
    let encoders = codec.encoders();
    if encoders.iter().all(missing) {
        list.extend(encoders);
    }
    list
}

/// Play (or cue) part of a source's replay buffer on a channel.
async fn post_replay(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<ReplayClipRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let rate = req.rate.unwrap_or(1.0);
    check_rate(rate).map_err(ApiError::bad)?;
    let (replay, name) = {
        let mgr = state.source_manager.read().await;
        let replay = mgr
            .replay_clip(
                &req.source_id,
                gst::ClockTime::from_useconds(req.from_us),
                req.to_us.map(gst::ClockTime::from_useconds),
            )
            .map_err(ApiError::bad)?;
        let name = mgr
            .get_source(&req.source_id)
            .map(|s| s.display_name().to_string())
            .unwrap_or_else(|| req.source_id.clone());
        (replay, name)
    };
    let path = replay
        .span
        .files
        .first()
        .and_then(|f| Path::new(f).parent())
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let request = ClipRequest {
        media_id: format!("replay:{}", req.source_id),
        name: format!("{name} (replay)"),
        path,
        in_point: replay.span.from,
        out_point: Some(replay.span.to),
        end: ClipEnd::Hold,
        rate,
        replay: Some(replay),
    };
    let result = playout.load(request, req.play).await;
    reply_to(&state, &id, &playout, result).await
}

// ── /replay/clips ─────────────────────────────────────────────────────────────

const CLIP_NOT_FOUND: ApiError = ApiError::NotFound("replay clip not found");

/// Where exported clips go: a folder in the default recording folder.
fn export_dir() -> std::path::PathBuf {
    crate::storage::default_folder().join("Replay clips")
}

fn clip_dto(row: ReplayClipRow) -> ReplayClipDto {
    let us = |v: i64| v.max(0) as u64;
    let base = us(row.base_us);
    ReplayClipDto {
        missing: !row.files.iter().all(|f| Path::new(f).is_file()),
        start_utc_us: row
            .utc_offset_us
            .map(|offset| (base + us(row.from_us)) as i64 + offset),
        from_us: base + us(row.from_us),
        to_us: base + us(row.to_us),
        id: row.id,
        source_id: row.source_id,
        source_name: row.source_name,
        name: row.name,
        codec: row.codec,
        bytes: us(row.bytes),
        created_at: row.created_at,
    }
}

/// A clip's in and out, as positions in its files.
fn clip_points(row: &ReplayClipRow) -> (gst::ClockTime, gst::ClockTime) {
    let t = |v: i64| gst::ClockTime::from_useconds(v.max(0) as u64);
    (t(row.from_us), t(row.to_us))
}

async fn clip_row(state: &AppState, id: &str) -> ApiResult<ReplayClipRow> {
    let row = db::replay_clip_get(&state.db, id)
        .await?
        .ok_or(CLIP_NOT_FOUND)?;
    if !row.files.iter().all(|f| Path::new(f).is_file()) {
        return Err(ApiError::Conflict("the clip's files are no longer on disk"));
    }
    Ok(row)
}

async fn get_clips(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<ReplayClipDto>>> {
    let rows = db::replay_clips_list(&state.db).await?;
    Ok(Json(rows.into_iter().map(clip_dto).collect()))
}

/// The angle and the time of day it starts, as "Cam 1 14:03:22".
fn default_clip_name(source: &str, start_utc_us: Option<i64>) -> String {
    match start_utc_us.and_then(chrono::DateTime::from_timestamp_micros) {
        Some(t) => format!(
            "{source} {}",
            t.with_timezone(&chrono::Local).format("%H:%M:%S")
        ),
        None => format!("{source} clip"),
    }
}

/// Save part of a source's replay buffer as a clip of its own.
async fn post_clip(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SaveReplayClipRequest>,
) -> ApiResult<(StatusCode, Json<ReplayClipDto>)> {
    if req.to_us <= req.from_us {
        return Err(ApiError::BadRequest(
            "the out point must be after the in point".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let utc_offset_us = state.clock.utc_offset_us();
    let (replay, source_name, dir) = {
        let mgr = state.source_manager.read().await;
        let replay = mgr
            .replay_clip(
                &req.source_id,
                gst::ClockTime::from_useconds(req.from_us),
                Some(gst::ClockTime::from_useconds(req.to_us)),
            )
            .map_err(ApiError::bad)?;
        let name = mgr
            .get_source(&req.source_id)
            .map(|s| s.display_name().to_string())
            .unwrap_or_else(|| req.source_id.clone());
        (replay, name, mgr.replay_clip_dir(&id))
    };
    // The span holds the buffer's files until they're linked.
    let span = replay.span;
    let (files, span) = tokio::task::spawn_blocking(move || {
        let files = replay::save(&span, &dir);
        (files, span)
    })
    .await?;
    let files = files.map_err(|e| ApiError::Internal(e.context("can't save the clip")))?;
    let bytes = files
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum::<u64>();
    let start_utc_us = utc_offset_us.map(|o| (span.base + span.from).useconds() as i64 + o);
    let name = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_clip_name(&source_name, start_utc_us));
    let row = ReplayClipRow {
        id,
        source_id: req.source_id,
        source_name,
        name,
        codec: span.codec,
        files: sqlx::types::Json(files),
        base_us: span.base.useconds() as i64,
        from_us: span.from.useconds() as i64,
        to_us: span.to.useconds() as i64,
        utc_offset_us,
        bytes: bytes as i64,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db::replay_clip_insert(&state.db, &row).await?;
    state.emit(&WsEvent::ReplayClipsUpdated);
    Ok((StatusCode::CREATED, Json(clip_dto(row))))
}

async fn patch_clip(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UpdateReplayClipRequest>,
) -> ApiResult<Json<ReplayClipDto>> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("a clip needs a name".into()));
    }
    if !db::replay_clip_rename(&state.db, &id, name).await? {
        return Err(CLIP_NOT_FOUND);
    }
    state.emit(&WsEvent::ReplayClipsUpdated);
    let row = db::replay_clip_get(&state.db, &id)
        .await?
        .ok_or(CLIP_NOT_FOUND)?;
    Ok(Json(clip_dto(row)))
}

/// Delete a saved clip and its files. One playing on a channel plays on
/// (its files stay readable while open); its exports stay.
async fn delete_clip(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    if !db::replay_clip_delete(&state.db, &id).await? {
        return Err(CLIP_NOT_FOUND);
    }
    let dir = state.source_manager.read().await.replay_clip_dir(&id);
    if dir.exists() {
        if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
            tracing::warn!(dir = ?dir, error = %e, "can't delete a replay clip's files");
        }
    }
    state.emit(&WsEvent::ReplayClipsUpdated);
    Ok(StatusCode::NO_CONTENT)
}

/// `name.mov` in `dir`, or `name_2.mov`, … : never an existing file, nor
/// one the library still lists (a deleted export shows there as missing).
fn free_path(dir: &Path, name: &str, listed: &HashSet<String>) -> std::path::PathBuf {
    let stem: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c == '\\' || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim().trim_start_matches('.');
    let stem = if stem.is_empty() { "Replay clip" } else { stem };
    let mut path = dir.join(format!("{stem}.mov"));
    let mut n = 2;
    while path.exists() || listed.contains(&*path.to_string_lossy()) {
        path = dir.join(format!("{stem}_{n}.mov"));
        n += 1;
    }
    path
}

/// Remux a saved clip into one .mov in the Replay clips folder, without
/// re-encoding, and add it to the media library.
async fn post_clip_export(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<(StatusCode, Json<MediaItemDto>)> {
    let row = clip_row(&state, &id).await?;
    let (from, to) = clip_points(&row);
    let dir = export_dir();
    let name = row.name.clone();
    let files = row.files.0.clone();
    let listed: HashSet<String> = db::media_list(&state.db)
        .await?
        .into_iter()
        .map(|m| m.path)
        .collect();
    let path = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        std::fs::create_dir_all(&dir)?;
        let out = free_path(&dir, &name, &listed);
        replay::export(&files, from, to, &out)?;
        Ok(out.to_string_lossy().into_owned())
    })
    .await?
    .map_err(|e| ApiError::Internal(e.context("export failed")))?;
    let probe_path = path.clone();
    let info = tokio::task::spawn_blocking(move || crate::sources::file::probe(&probe_path))
        .await?
        .map_err(|e| ApiError::Internal(e.context("the export can't be read")))?;
    let item = MediaItemDto {
        id: uuid::Uuid::new_v4().to_string(),
        path,
        name: row.name,
        info,
        origin: MediaOrigin::Replay,
        session_id: None,
        added_at: chrono::Utc::now().to_rfc3339(),
        missing: false,
    };
    if !db::media_insert(&state.db, &item).await? {
        return Err(ApiError::Conflict("that file is already in the library"));
    }
    state.emit(&WsEvent::MediaUpdated);
    Ok((StatusCode::CREATED, Json(item)))
}

/// Play (or cue) a saved replay clip on a channel.
async fn post_replay_clip(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PlayReplayClipRequest>,
) -> ApiResult<Json<ChannelStatusDto>> {
    let playout = playout(&state, &id).await?;
    let rate = req.rate.unwrap_or(1.0);
    check_rate(rate).map_err(ApiError::bad)?;
    let row = clip_row(&state, &req.clip_id).await?;
    let (from, to) = clip_points(&row);
    let path = state
        .source_manager
        .read()
        .await
        .replay_clip_dir(&row.id)
        .to_string_lossy()
        .into_owned();
    let replay = ReplayClip {
        source_id: row.source_id,
        clip_id: Some(row.id.clone()),
        span: ReplaySpan {
            files: row.files.0,
            codec: row.codec,
            base: gst::ClockTime::from_useconds(row.base_us.max(0) as u64),
            from,
            to,
            hold: None,
        },
    };
    let request = ClipRequest {
        media_id: format!("replay-clip:{}", row.id),
        name: row.name,
        path,
        in_point: from,
        out_point: Some(to),
        end: ClipEnd::Hold,
        rate,
        replay: Some(replay),
    };
    let result = playout.load(request, req.play).await;
    reply_to(&state, &id, &playout, result).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_path_skips_listed_paths() {
        let dir = std::env::temp_dir().join(format!("capture-room-free-{}", std::process::id()));
        let listed = HashSet::from([dir.join("Goal.mov").to_string_lossy().into_owned()]);
        assert_eq!(free_path(&dir, "Goal", &listed), dir.join("Goal_2.mov"));
        assert_eq!(free_path(&dir, "Save", &listed), dir.join("Save.mov"));
    }
}
