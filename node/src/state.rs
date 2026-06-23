use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;
use tokio::sync::{broadcast, RwLock};

use crate::recording::RecordingManager;
use crate::sources::registry::SourceRegistry;

pub struct AppState {
    pub node_id: String,
    pub node_name: String,
    pub started_at: Instant,
    pub sources: Arc<RwLock<SourceRegistry>>,
    pub recordings: Arc<RwLock<RecordingManager>>,
    pub db: SqlitePool,
    pub ws_tx: broadcast::Sender<String>,
}
