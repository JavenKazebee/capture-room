use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::{broadcast, RwLock};

use crate::controller::registry::NodeRegistry;
use crate::sources::manager::SourceManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Node,
    Aggregator,
}

impl Role {
    pub fn is_aggregator(self) -> bool {
        matches!(self, Role::Aggregator)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Node => "node",
            Role::Aggregator => "aggregator",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_lowercase().as_str() {
            "node" => Some(Role::Node),
            "aggregator" | "controller" => Some(Role::Aggregator),
            _ => None,
        }
    }
}

pub struct AppState {
    pub node_id: String,
    pub node_name: String,
    pub started_at: Instant,
    pub role: Role,

    // ── Local capture (present on every instance) ──────────────────────────
    pub source_manager: Arc<RwLock<SourceManager>>,
    pub db: SqlitePool,
    pub ws_tx: broadcast::Sender<String>,

    // ── Aggregation (populated only when role == Aggregator) ───────────────
    pub peers: Arc<RwLock<NodeRegistry>>,
    pub http: reqwest::Client,
}
