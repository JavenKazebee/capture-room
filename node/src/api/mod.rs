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

// ── ts-rs export test (runs under --features export-types) ───────────────────

#[cfg(all(test, feature = "export-types"))]
mod export_tests {
    use super::types::*;
    use ts_rs::TS;

    #[test]
    fn export_all_types() {
        NodeStatus::export_all().unwrap();
        SourceDto::export_all().unwrap();
        TimecodeDto::export_all().unwrap();
        SourceCapabilitiesDto::export_all().unwrap();
        TestSourceConfigDto::export_all().unwrap();
        CreateTestSourceRequest::export_all().unwrap();
        RecordingSessionDto::export_all().unwrap();
        StartRecordingRequest::export_all().unwrap();
        PresetDto::export_all().unwrap();
        PresetCreateRequest::export_all().unwrap();
        WsEvent::export_all().unwrap();
        ChannelLevelDto::export_all().unwrap();
        NodeSettingsDto::export_all().unwrap();
        UpdateNodeSettingsRequest::export_all().unwrap();
        StorageVolumeDto::export_all().unwrap();
        NodeDto::export_all().unwrap();
        AddNodeRequest::export_all().unwrap();
        ControllerToggleRequest::export_all().unwrap();
    }
}
