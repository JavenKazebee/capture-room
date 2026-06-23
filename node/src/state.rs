use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::{broadcast, RwLock};

use crate::controller::registry::NodeRegistry;
use crate::recording::RecordingManager;
use crate::sources::registry::SourceRegistry;

/// Every instance is a full capture node. An `Aggregator` is a node that
/// additionally discovers peers, polls their health, relays their events,
/// and fans API requests out across them — the "control station".
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
            // "controller" accepted as a legacy alias for "aggregator"
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
    pub sources: Arc<RwLock<SourceRegistry>>,
    pub recordings: Arc<RwLock<RecordingManager>>,
    pub db: SqlitePool,
    pub ws_tx: broadcast::Sender<String>,

    // ── Aggregation (populated only when role == Aggregator) ───────────────
    pub peers: Arc<RwLock<NodeRegistry>>,
    pub http: reqwest::Client,
}
