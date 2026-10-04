use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::{broadcast, RwLock};

use crate::controller::Controller;
use crate::sources::manager::SourceManager;

pub struct AppState {
    pub node_id: String,
    pub node_name: std::sync::RwLock<String>,
    pub started_at: Instant,

    // ── Node: local capture (present on every instance) ────────────────────
    pub source_manager: Arc<RwLock<SourceManager>>,
    pub db: SqlitePool,
    /// This node's own events only. Served on `/api/v1/node/ws`.
    pub node_tx: broadcast::Sender<String>,
    /// This node's events plus everything relayed from peers while acting as
    /// a controller. Served on `/ws`.
    pub ws_tx: broadcast::Sender<String>,
    /// The benchmark running on this node, if any.
    pub benchmark: std::sync::Mutex<Option<crate::benchmark::Running>>,

    // ── Controller: present only while promoted ────────────────────────────
    pub controller: RwLock<Option<Controller>>,
    pub http: reqwest::Client,
    /// The node API router, used to serve `/api/v1/nodes/{self}/…` in-process.
    /// Set once at startup (it holds an `Arc` back to this state).
    pub node_router: std::sync::OnceLock<axum::Router>,
}

impl AppState {
    pub fn node_name(&self) -> String {
        self.node_name.read().unwrap().clone()
    }

    pub async fn is_controller(&self) -> bool {
        self.controller.read().await.is_some()
    }

    /// Emit a local event on both the node-only and the merged channel.
    pub fn emit(&self, event: &crate::api::types::WsEvent) {
        if let Some(json) = crate::ws::encode(&self.node_id, event) {
            let _ = self.node_tx.send(json.clone());
            let _ = self.ws_tx.send(json);
        }
    }

    /// Emit a controller event (about its peers) on the merged channel only:
    /// it isn't this node's own event, so other controllers mustn't relay it.
    pub fn emit_controller(&self, event: &crate::api::types::WsEvent) {
        if let Some(json) = crate::ws::encode(&self.node_id, event) {
            let _ = self.ws_tx.send(json);
        }
    }
}
