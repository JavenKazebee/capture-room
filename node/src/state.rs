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

    pub source_manager: Arc<RwLock<SourceManager>>,
    pub db: SqlitePool,
    /// This node's own events only. Served on `/api/v1/node/ws`.
    pub node_tx: broadcast::Sender<String>,
    /// This node's events plus everything relayed from peers while acting as
    /// a controller. Served on `/ws`.
    pub ws_tx: broadcast::Sender<String>,
    pub benchmark: std::sync::Mutex<Option<crate::benchmark::Running>>,
    /// What this node's pipelines run on.
    pub clock: Arc<crate::clock::NodeClock>,
    pub port: u16,
    /// The UDP port this instance serves its clock on while a controller.
    pub clock_port: u16,

    pub controller: RwLock<Option<Controller>>,
    /// For the outside world (updates): not other nodes.
    pub http: reqwest::Client,
    /// For other nodes, over TLS, trusting only approved ones.
    pub peers: crate::tls::Peers,
    /// SHA-256 of this node's TLS key: what controllers pin it by.
    pub fingerprint: String,
    /// The node API router, used to serve `/api/v1/nodes/{self}/…` in-process.
    /// Set once at startup (it holds an `Arc` back to this state).
    pub node_router: std::sync::OnceLock<axum::Router>,
    pub updater: crate::update::Updater,
    pub access_token: crate::auth::AccessToken,
}

impl AppState {
    pub fn node_name(&self) -> String {
        self.node_name.read().unwrap().clone()
    }

    /// A request to an approved peer node, carrying this node's access token.
    pub fn peer(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let req = self.peers.http.request(method, url);
        match self.access_token.bearer() {
            Some(v) => req.header(reqwest::header::AUTHORIZATION, v),
            None => req,
        }
    }

    pub async fn is_controller(&self) -> bool {
        self.controller.read().await.is_some()
    }

    pub async fn clock_status(&self) -> crate::api::types::ClockStatusDto {
        let stale = self.source_manager.read().await.stale_sources();
        self.clock.status(stale)
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
