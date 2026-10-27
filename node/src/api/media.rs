//! The media library: files a node can play out, added by path or by
//! exporting a replay clip. Part of the node API (`/api/v1/node/…`).

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, Query, Request, State},
    http::StatusCode,
    response::Response,
    routing::get,
    Json, Router,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::playout::{playout, reload_playlist};
use crate::api::types::{AddMediaRequest, MediaItemDto, MediaOrigin, WsEvent};
use crate::db;
use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/media", get(get_media).post(post_media))
        .route("/media/{id}", axum::routing::delete(delete_media))
        .route("/media/{id}/file", get(get_media_file))
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
pub(super) async fn add_media(
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
