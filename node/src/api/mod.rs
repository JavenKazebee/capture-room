pub mod error;
pub mod node;
pub mod playout;
pub mod types;
pub mod whep;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use rust_embed::RustEmbed;

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::MediaInfo;
use crate::state::AppState;

/// The node API as a standalone router (paths relative to `/api/v1/node`).
/// Also used in-process to serve `/api/v1/nodes/{self}/…`.
pub fn node_router(state: Arc<AppState>) -> Router {
    node::router().with_state(state)
}

/// No CORS layer: the UI is same-origin, and allowing other origins would let
/// any open page start recordings or add nodes. Only `/whep` allows any origin.
/// The access token guards everything under it (see [`crate::auth`]); the
/// node router served in-process for `/nodes/{self}` is behind it already.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api/v1/node", node::router())
        .merge(crate::controller::api::router())
        .merge(crate::auth::router())
        .merge(crate::auth::open_router())
        .merge(whep::router())
        .fallback(serve_ui)
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            crate::auth::require,
        ))
        // Outermost: plain HTTP is redirected before it's refused.
        .layer(axum::middleware::from_fn(crate::tls::redirect_plain))
        .with_state(state)
}

/// Probe the media file at `path` off the runtime: a 400 if it can't be
/// read.
pub(crate) async fn probe(path: String) -> ApiResult<MediaInfo> {
    tokio::task::spawn_blocking(move || crate::sources::file::probe(&path))
        .await?
        .map_err(ApiError::bad)
}

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

/// Serve the UI for page loads: GET/HEAD outside `/api`, with unknown paths
/// getting `index.html` for client-side routing. Anything else unmatched is a
/// real 404 — an unknown API route must not "succeed" with the UI's HTML.
async fn serve_ui(method: Method, uri: Uri) -> Response {
    if !matches!(method, Method::GET | Method::HEAD) || uri.path().starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    serve_asset(path)
        .or_else(|| serve_asset("index.html"))
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}
