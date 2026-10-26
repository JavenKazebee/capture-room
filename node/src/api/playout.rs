//! Playout: the media library, channel transport and replay buffers. Part
//! of the node API (`/api/v1/node/…`). Channels themselves are configured
//! sources, created and edited through `/configured-sources`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, Query, Request, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use gstreamer as gst;

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    AddMediaRequest, ChannelStatusDto, ClipEnd, CueRequest, LoadedClipDto, MediaItemDto,
    MediaOrigin, OutputStatusDto, PlayReplayClipRequest, PlaylistDto, PlaylistInput,
    PlaylistItemDto, ReplayBufferDto, ReplayClipDto, ReplayClipGroupDto, ReplayClipRequest,
    ReplayConfigInput, SaveReplayClipGroupRequest, SaveReplayClipRequest, TransportAction,
    TransportRequest, UpdateReplayClipRequest, WsEvent,
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
        .route("/media/{id}/file", get(get_media_file))
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
        .route("/replay/clip-groups", post(post_clip_group))
        .route(
            "/replay/clip-groups/{id}",
            axum::routing::patch(patch_clip_group).delete(delete_clip_group),
        )
        .route(
            "/replay/clip-groups/{id}/export",
            post(post_clip_group_export),
        )
        .route(
            "/replay/{source_id}",
            axum::routing::put(put_replay).delete(delete_replay),
        )
}

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
            return Err(ApiError::bad("the file isn't one of the session's"));
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
    let origin = if req.session_id.is_some() {
        MediaOrigin::Recording
    } else {
        MediaOrigin::Import
    };
    add_media(&state, path, name, info, origin, req.session_id).await
}

/// Add `path` to the media library as `name`, unless it's there already.
async fn add_media(
    state: &AppState,
    path: String,
    name: String,
    info: super::MediaInfo,
    origin: MediaOrigin,
    session_id: Option<String>,
) -> ApiResult<(StatusCode, Json<MediaItemDto>)> {
    let item = MediaItemDto {
        id: uuid::Uuid::new_v4().to_string(),
        path,
        name,
        info,
        origin,
        session_id,
        added_at: chrono::Utc::now().to_rfc3339(),
        missing: false,
    };
    if !db::media_insert(&state.db, &item).await? {
        return Err(ApiError::Conflict("that file is already in the library"));
    }
    state.emit(&WsEvent::MediaUpdated);
    Ok((StatusCode::CREATED, Json(item)))
}

/// A library item's file, with range requests; `?download` saves it (how
/// Replay exports reach the browser).
async fn get_media_file(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<super::node::FileQuery>,
    req: Request,
) -> ApiResult<Response> {
    let item = db::media_get(&state.db, &id)
        .await?
        .ok_or(MEDIA_NOT_FOUND)?;
    let path = Path::new(&item.path);
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Err(ApiError::NotFound("file is no longer on disk"));
    }
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    super::node::serve_file(path, mime, q.download.is_some(), req).await
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
        return Err(ApiError::bad(format!(
            "a replay buffer keeps 1 to {MAX_REPLAY_MINUTES} minutes"
        )));
    }
    if !req.codec.available() {
        return Err(ApiError::bad(format!(
            "this node can't record {:?} for replay (missing {})",
            req.codec,
            req.codec.missing().join(", ")
        )));
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
    let work = state
        .source_manager
        .write()
        .await
        .set_replay(&source_id, Some(req));
    crate::session::run_work(&state, work).await?;
    let utc = state.clock.utc_offset_us();
    state
        .source_manager
        .read()
        .await
        .replay_buffers(utc)
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
    let work = state
        .source_manager
        .write()
        .await
        .set_replay(&source_id, None);
    crate::session::run_work(&state, work).await?;
    // An NDI sender kept only for its buffer goes (see NDI subscriptions).
    if let Err(e) = crate::session::rebuild_sources(&state).await {
        tracing::error!(error = %e, "rebuild sources after removing a replay buffer");
    }
    Ok(StatusCode::NO_CONTENT)
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
        // Without one it plays on into what's recorded meanwhile, holding at
        // the newest frame if it catches up.
        out_point: req.to_us.map(|_| replay.span.to),
        start: None,
        end: ClipEnd::Hold,
        rate,
        replay: Some(replay),
    };
    let result = playout.load(request, req.play).await;
    reply_to(&state, &id, &playout, result).await
}

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
        group_id: row.group_id,
        group_name: row.group_name,
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

/// Save part of a source's replay buffer as a clip's own files, in a new
/// clip folder. Named after the angle and its time of day, in no group.
async fn save_span(
    state: &AppState,
    source_id: &str,
    from_us: u64,
    to_us: u64,
) -> ApiResult<ReplayClipRow> {
    let id = uuid::Uuid::new_v4().to_string();
    let utc_offset_us = state.clock.utc_offset_us();
    let (replay, source_name, dir) = {
        let mgr = state.source_manager.read().await;
        let replay = mgr
            .replay_clip(
                source_id,
                gst::ClockTime::from_useconds(from_us),
                Some(gst::ClockTime::from_useconds(to_us)),
            )
            .map_err(ApiError::bad)?;
        let name = mgr
            .get_source(source_id)
            .map(|s| s.display_name().to_string())
            .unwrap_or_else(|| source_id.to_string());
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
    Ok(ReplayClipRow {
        id,
        source_id: source_id.to_string(),
        name: default_clip_name(&source_name, start_utc_us),
        source_name,
        codec: span.codec,
        files: sqlx::types::Json(files),
        base_us: span.base.useconds() as i64,
        from_us: span.from.useconds() as i64,
        to_us: span.to.useconds() as i64,
        utc_offset_us,
        bytes: bytes as i64,
        created_at: chrono::Utc::now().to_rfc3339(),
        group_id: None,
        group_name: None,
    })
}

/// Delete saved clips' folders, logging what can't be.
async fn remove_clip_dirs(state: &AppState, ids: impl IntoIterator<Item = String>) {
    let dirs: Vec<_> = {
        let mgr = state.source_manager.read().await;
        ids.into_iter().map(|id| mgr.replay_clip_dir(&id)).collect()
    };
    for dir in dirs {
        if dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                tracing::warn!(dir = ?dir, error = %e, "can't delete a replay clip's files");
            }
        }
    }
}

fn check_span(from_us: u64, to_us: u64) -> ApiResult<()> {
    if to_us <= from_us {
        return Err(ApiError::bad("the out point must be after the in point"));
    }
    Ok(())
}

/// A name from a request: trimmed, `None` if blank.
fn given_name(name: Option<String>) -> Option<String> {
    name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
}

/// Save part of a source's replay buffer as a clip of its own.
async fn post_clip(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SaveReplayClipRequest>,
) -> ApiResult<(StatusCode, Json<ReplayClipDto>)> {
    check_span(req.from_us, req.to_us)?;
    let mut row = save_span(&state, &req.source_id, req.from_us, req.to_us).await?;
    if let Some(name) = given_name(req.name) {
        row.name = name;
    }
    if let Err(e) = db::replay_clip_insert(&state.db, &row).await {
        remove_clip_dirs(&state, [row.id]).await;
        return Err(e.into());
    }
    state.emit(&WsEvent::ReplayClipsUpdated);
    Ok((StatusCode::CREATED, Json(clip_dto(row))))
}

/// Save one in → out on several angles, a clip each, as one group: all of
/// them or none.
async fn post_clip_group(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SaveReplayClipGroupRequest>,
) -> ApiResult<(StatusCode, Json<ReplayClipGroupDto>)> {
    check_span(req.from_us, req.to_us)?;
    let mut sources = req.source_ids;
    let mut seen = HashSet::new();
    sources.retain(|s| seen.insert(s.clone()));
    if sources.is_empty() {
        return Err(ApiError::bad("pick at least one angle"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut rows: Vec<ReplayClipRow> = Vec::with_capacity(sources.len());
    for source in &sources {
        match save_span(&state, source, req.from_us, req.to_us).await {
            Ok(mut row) => {
                row.name = row.source_name.clone();
                row.group_id = Some(id.clone());
                rows.push(row);
            }
            Err(e) => {
                remove_clip_dirs(&state, rows.into_iter().map(|r| r.id)).await;
                return Err(e);
            }
        }
    }
    let start_utc_us = rows[0]
        .utc_offset_us
        .map(|o| rows[0].base_us + rows[0].from_us + o);
    let name = given_name(req.name).unwrap_or_else(|| default_clip_name("Replay", start_utc_us));
    let created_at = rows[0].created_at.clone();
    if let Err(e) = db::replay_clip_group_insert(&state.db, &id, &name, &created_at, &rows).await {
        remove_clip_dirs(&state, rows.into_iter().map(|r| r.id)).await;
        return Err(e.into());
    }
    state.emit(&WsEvent::ReplayClipsUpdated);
    let clips = rows
        .into_iter()
        .map(|mut r| {
            r.group_name = Some(name.clone());
            clip_dto(r)
        })
        .collect();
    Ok((
        StatusCode::CREATED,
        Json(ReplayClipGroupDto { id, name, clips }),
    ))
}

async fn patch_clip(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UpdateReplayClipRequest>,
) -> ApiResult<Json<ReplayClipDto>> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad("a clip needs a name"));
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
    remove_clip_dirs(&state, [id]).await;
    state.emit(&WsEvent::ReplayClipsUpdated);
    Ok(StatusCode::NO_CONTENT)
}

const GROUP_NOT_FOUND: ApiError = ApiError::NotFound("replay clip group not found");

async fn patch_clip_group(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UpdateReplayClipRequest>,
) -> ApiResult<StatusCode> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad("a group needs a name"));
    }
    if !db::replay_clip_group_rename(&state.db, &id, name).await? {
        return Err(GROUP_NOT_FOUND);
    }
    state.emit(&WsEvent::ReplayClipsUpdated);
    Ok(StatusCode::NO_CONTENT)
}

/// Delete a group's clips and their files, as `DELETE /replay/clips/{id}`
/// does for each.
async fn delete_clip_group(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    let clips = db::replay_clip_group_clips(&state.db, &id).await?;
    if !db::replay_clip_group_delete(&state.db, &id).await? {
        return Err(GROUP_NOT_FOUND);
    }
    remove_clip_dirs(&state, clips.into_iter().map(|c| c.id)).await;
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

/// What an export of a clip is called: a grouped clip is its group's name
/// and its angle.
fn export_name(row: &ReplayClipRow) -> String {
    match &row.group_name {
        Some(group) => format!("{group} – {}", row.name),
        None => row.name.clone(),
    }
}

/// Remux a saved clip into one .mov in the Replay clips folder, without
/// re-encoding, and add it to the media library.
async fn export_clip(state: &AppState, row: ReplayClipRow) -> ApiResult<MediaItemDto> {
    let (from, to) = clip_points(&row);
    let dir = export_dir();
    let name = export_name(&row);
    let stem = name.clone();
    let base = gst::ClockTime::from_useconds(row.base_us.max(0) as u64);
    let timecode = row.utc_offset_us.map(|offset| (base, offset));
    let files = row.files.0;
    let listed: HashSet<String> = db::media_list(&state.db)
        .await?
        .into_iter()
        .map(|m| m.path)
        .collect();
    let path = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        std::fs::create_dir_all(&dir)?;
        let out = free_path(&dir, &stem, &listed);
        replay::export(&files, from, to, timecode, &out)?;
        Ok(out.to_string_lossy().into_owned())
    })
    .await?
    .map_err(|e| ApiError::Internal(e.context("export failed")))?;
    let probe_path = path.clone();
    let info = tokio::task::spawn_blocking(move || crate::sources::file::probe(&probe_path))
        .await?
        .map_err(|e| ApiError::Internal(e.context("the export can't be read")))?;
    let (_, Json(item)) = add_media(state, path, name, info, MediaOrigin::Replay, None).await?;
    Ok(item)
}

async fn post_clip_export(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<(StatusCode, Json<MediaItemDto>)> {
    let row = clip_row(&state, &id).await?;
    Ok((StatusCode::CREATED, Json(export_clip(&state, row).await?)))
}

/// Export each of a group's clips, as `POST /replay/clips/{id}/export` does.
/// Clips whose files are gone are left out.
async fn post_clip_group_export(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<(StatusCode, Json<Vec<MediaItemDto>>)> {
    let rows = db::replay_clip_group_clips(&state.db, &id).await?;
    if rows.is_empty() {
        return Err(GROUP_NOT_FOUND);
    }
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| r.files.iter().all(|f| Path::new(f).is_file()))
        .collect();
    if rows.is_empty() {
        return Err(ApiError::Conflict("the clips' files are no longer on disk"));
    }
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(export_clip(&state, row).await?);
    }
    Ok((StatusCode::CREATED, Json(items)))
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
    let start = req
        .at_us
        .map(|at| gst::ClockTime::from_useconds(at.saturating_sub(row.base_us.max(0) as u64)));
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
        start,
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
