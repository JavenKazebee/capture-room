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
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use tokio::sync::RwLock;
use tracing::info;

use api::types::WsEvent;
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

    // ── Mark orphaned sessions from a previous run ────────────────────────────
    db::sessions_mark_crashed(&pool).await?;

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

    // ── Periodic WS event emitter ─────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            // 100 ms tick → audio levels at ~10 fps; thumbnail + feed.status at 1 fps
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            let mut tick: u32 = 0;
            loop {
                interval.tick().await;
                tick = tick.wrapping_add(1);

                let mgr = state.recordings.read().await;
                let active: Vec<_> = mgr.active_sessions().into_iter().cloned().collect();

                // Audio levels at every tick
                for session in &active {
                    if let Some(channels) = mgr.audio_levels(&session.source_id) {
                        ws::send(&state.ws_tx, &WsEvent::AudioLevels {
                            source_id: session.source_id.clone(),
                            channels,
                        });
                    }
                }
                drop(mgr);

                // Every 10 ticks (~1 fps): timecode for all sources + thumbnail for recording ones
                if tick % 10 == 0 {
                    let sources = state.sources.read().await;

                    // Timecode update for every source, regardless of recording state
                    for source in sources.sources() {
                        let timecode = source.timecode().map(|tc| tc.to_string());
                        ws::send(&state.ws_tx, &WsEvent::FeedStatus {
                            source_id: source.id().to_string(),
                            timecode,
                            duration_secs: 0.0,
                        });
                    }

                    // Thumbnail update only for sources with an active pipeline
                    for session in &active {
                        ws::send(&state.ws_tx, &WsEvent::ThumbnailUpdated {
                            source_id: session.source_id.clone(),
                            url: format!("/api/v1/thumbnails/{}", session.source_id),
                        });
                    }
                }
            }
        });
    }

    // ── HTTP server ───────────────────────────────────────────────────────────
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let router = api::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(addr = %addr, mode = ?args.mode, "listening");
    axum::serve(listener, router).await?;

    Ok(())
}
