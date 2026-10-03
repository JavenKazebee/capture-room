pub mod node;
pub mod types;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use rust_embed::RustEmbed;
use tower_http::cors::CorsLayer;

use crate::state::AppState;

/// The node API as a standalone router (paths relative to `/api/v1/node`).
/// Also used in-process to serve `/api/v1/nodes/{self}/…`.
pub fn node_router(state: Arc<AppState>) -> Router {
    node::router().with_state(state)
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api/v1/node", node::router())
        .merge(crate::controller::api::router())
        .fallback(serve_ui)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

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
