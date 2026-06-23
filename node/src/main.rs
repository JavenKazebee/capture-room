mod api;
mod audio;
mod db;
mod pipeline;
mod plugins;
mod recording;
mod sources;
mod state;
mod thumbnail;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use tokio::sync::RwLock;
use tracing::info;

use sources::registry::SourceRegistry;
use state::AppState;
use recording::RecordingManager;

#[derive(Debug, Clone, ValueEnum)]
enum Mode {
    Node,
    Controller,
}

#[derive(Parser, Debug)]
#[command(name = "capture-room", version)]
struct Args {
    #[arg(long, default_value = "node")]
    mode: Mode,

    #[arg(long, default_value_t = 7700)]
    port: u16,

    #[arg(long, default_value = "capture-room.db")]
    db: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "capture_room=debug,info".into()),
        )
        .init();

    gstreamer::init().expect("GStreamer init failed");
    plugins::check_required_plugins()?;

    let args = Args::parse();

    // ── Database ──────────────────────────────────────────────────────────────
    let pool = db::init(&args.db).await?;

    // ── Node identity ─────────────────────────────────────────────────────────
    let node_id = match db::config_get(&pool, "uuid").await? {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            db::config_set(&pool, "uuid", &id).await?;
            id
        }
    };
    let node_name = db::config_get(&pool, "name")
        .await?
        .unwrap_or_else(|| {
            std::env::var("HOSTNAME").unwrap_or_else(|_| "capture-room-node".to_string())
        });

    info!(id = %node_id, name = %node_name, "node identity");

    // ── Source registry ───────────────────────────────────────────────────────
    let mut registry = SourceRegistry::new();
    registry.scan()?;
    for source in registry.sources() {
        info!(id = source.id(), name = source.display_name(), "source ready");
    }

    // ── WebSocket broadcast channel ───────────────────────────────────────────
    let (ws_tx, _) = ws::channel();

    // ── App state ─────────────────────────────────────────────────────────────
    let state = Arc::new(AppState {
        node_id,
        node_name,
        started_at: Instant::now(),
        sources: Arc::new(RwLock::new(registry)),
        recordings: Arc::new(RwLock::new(RecordingManager::new())),
        db: pool,
        ws_tx,
    });

    // ── HTTP server ───────────────────────────────────────────────────────────
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let router = api::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(addr = %addr, mode = ?args.mode, "listening");
    axum::serve(listener, router).await?;

    Ok(())
}
