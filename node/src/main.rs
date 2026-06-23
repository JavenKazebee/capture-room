mod api;
mod audio;
mod controller;
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
use clap::Parser;
use tokio::sync::RwLock;
use tracing::info;

use api::types::WsEvent;
use controller::registry::NodeRegistry;
use recording::RecordingManager;
use sources::registry::SourceRegistry;
use state::{AppState, Role};

#[derive(Parser, Debug)]
#[command(name = "capture-room", version)]
struct Args {
    /// Override the persisted role: "node" or "aggregator". Highest priority.
    #[arg(long)]
    role: Option<String>,

    #[arg(long, default_value_t = 7700)]
    port: u16,

    #[arg(long, default_value = "capture-room.db")]
    db: String,
}

/// Resolve the effective role: CLI flag > env var > persisted DB value > default.
fn resolve_role(cli: Option<&str>, db_value: Option<&str>) -> Role {
    if let Some(r) = cli.and_then(Role::parse) {
        return r;
    }
    if let Ok(v) = std::env::var("CAPTURE_ROOM_ROLE") {
        if let Some(r) = Role::parse(&v) {
            return r;
        }
    }
    db_value.and_then(Role::parse).unwrap_or(Role::Node)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "capture_room=debug,info".into()),
        )
        .init();

    // Every instance is a full capture node.
    gstreamer::init().expect("GStreamer init failed");
    plugins::check_required_plugins()?;

    let args = Args::parse();

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
    let node_name = db::config_get(&pool, "name").await?.unwrap_or_else(|| {
        std::env::var("HOSTNAME").unwrap_or_else(|_| "capture-room-node".to_string())
    });

    // ── Role ──────────────────────────────────────────────────────────────────
    let db_role = db::config_get(&pool, "role").await?;
    let role = resolve_role(args.role.as_deref(), db_role.as_deref());

    info!(id = %node_id, name = %node_name, role = role.as_str(), "identity");

    db::sessions_mark_crashed(&pool).await?;

    // ── Source registry ───────────────────────────────────────────────────────
    let mut registry = SourceRegistry::new();
    registry.scan()?;
    for source in registry.sources() {
        info!(id = source.id(), name = source.display_name(), "source ready");
    }

    let (ws_tx, _) = ws::channel();

    let state = Arc::new(AppState {
        node_id: node_id.clone(),
        node_name: node_name.clone(),
        started_at: Instant::now(),
        role,
        sources: Arc::new(RwLock::new(registry)),
        recordings: Arc::new(RwLock::new(RecordingManager::new())),
        db: pool,
        ws_tx,
        peers: Arc::new(RwLock::new(NodeRegistry::new())),
        http: reqwest::Client::new(),
    });

    // ── Periodic local WS emitter (composite source IDs) ──────────────────────
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            let mut tick: u32 = 0;
            loop {
                interval.tick().await;
                tick = tick.wrapping_add(1);

                let mgr = state.recordings.read().await;
                let active: Vec<_> = mgr.active_sessions().into_iter().cloned().collect();

                for session in &active {
                    if let Some(channels) = mgr.audio_levels(&session.source_id) {
                        ws::send(
                            &state.ws_tx,
                            &WsEvent::AudioLevels {
                                source_id: format!("{}:{}", state.node_id, session.source_id),
                                channels,
                            },
                        );
                    }
                }
                drop(mgr);

                if tick % 10 == 0 {
                    let sources = state.sources.read().await;
                    for source in sources.sources() {
                        let timecode = source.timecode().map(|tc| tc.to_string());
                        ws::send(
                            &state.ws_tx,
                            &WsEvent::FeedStatus {
                                source_id: format!("{}:{}", state.node_id, source.id()),
                                timecode,
                                duration_secs: 0.0,
                            },
                        );
                    }
                    for session in &active {
                        let composite = format!("{}:{}", state.node_id, session.source_id);
                        ws::send(
                            &state.ws_tx,
                            &WsEvent::ThumbnailUpdated {
                                url: format!("/api/v1/thumbnails/{}", composite),
                                source_id: composite,
                            },
                        );
                    }
                }
            }
        });
    }

    // ── Aggregation (control station) ─────────────────────────────────────────
    if role.is_aggregator() {
        controller::start_mdns_browser(Arc::clone(&state));
        controller::start_health_poller(Arc::clone(&state));
        info!("aggregator: discovering peers via mDNS");
    }

    // Advertise ourselves so aggregators can find us. Kept alive for process life.
    let _mdns = controller::register_mdns_service(&node_id, &node_name, args.port);

    // ── HTTP server ───────────────────────────────────────────────────────────
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let router = api::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(addr = %addr, role = role.as_str(), "listening");
    axum::serve(listener, router).await?;

    Ok(())
}
