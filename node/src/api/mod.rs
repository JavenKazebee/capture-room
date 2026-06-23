pub mod node;
pub mod types;

use std::sync::Arc;

use axum::Router;
use tower_http::cors::CorsLayer;

use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .merge(node::router())
        .layer(CorsLayer::permissive())
        .with_state(state)
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
        RecordingSessionDto::export_all().unwrap();
        StartRecordingRequest::export_all().unwrap();
        PatchRecordingRequest::export_all().unwrap();
        PresetCacheDto::export_all().unwrap();
        PresetSyncRequest::export_all().unwrap();
        WsEvent::export_all().unwrap();
        ChannelLevelDto::export_all().unwrap();
    }
}
