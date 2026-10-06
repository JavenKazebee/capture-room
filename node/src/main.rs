mod api;
mod benchmark;
mod capacity;
mod controller;
mod db;
mod pipeline;
mod plugins;
mod session;
mod sources;
mod state;
mod storage;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use tokio::sync::RwLock;
use tracing::{info, warn};

use controller::Controller;
use sources::manager::SourceManager;
use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "capture-room", version)]
struct Args {
    /// Act as a controller (also persisted, so later runs don't need the flag).
    #[arg(long)]
    controller: bool,

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

    let args = Args::parse();

    gstreamer::init().expect("GStreamer init failed");
    gstndi::plugin_register_static().expect("NDI plugin registration failed");
    plugins::check_required_plugins()?;
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
        controller::discovery::local_hostname().unwrap_or_else(|| "capture-room-node".to_string())
    });

    if args.controller {
        db::config_set(&pool, controller::CONFIG_KEY, "true").await?;
    }
    let controller_enabled = db::config_get(&pool, controller::CONFIG_KEY)
        .await?
        .as_deref()
        == Some("true");
    info!(id = %node_id, name = %node_name, controller = controller_enabled, "identity");

    db::sessions_mark_crashed(&pool).await?;
    benchmark::recover(&pool).await?;

    // ── Source manager ────────────────────────────────────────────────────────
    let configured = db::configured_sources_list(&pool).await?;
    let monitor_config = db::monitor_settings_get(&pool).await?;
    let ndi_monitor = tokio::task::spawn_blocking(sources::ndi::NdiMonitor::start)
        .await
        .expect("NDI monitor thread panicked");
    let (leg_failure_tx, leg_failure_rx) = tokio::sync::mpsc::unbounded_channel();
    let (leg_file_tx, leg_file_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut source_manager =
        SourceManager::new(monitor_config, ndi_monitor, leg_failure_tx, leg_file_tx);
    source_manager.scan(&configured);

    for source in source_manager.sources() {
        info!(
            id = source.id(),
            name = source.display_name(),
            monitored = source_manager.is_monitored(source.id()),
            "source ready"
        );
    }

    let (node_tx, _) = ws::channel();
    let (ws_tx, _) = ws::channel();

    let state = Arc::new(AppState {
        node_id: node_id.clone(),
        node_name: std::sync::RwLock::new(node_name.clone()),
        started_at: Instant::now(),
        source_manager: Arc::new(RwLock::new(source_manager)),
        db: pool,
        node_tx,
        ws_tx,
        benchmark: std::sync::Mutex::new(None),
        controller: RwLock::new(None),
        http: reqwest::Client::new(),
        node_router: std::sync::OnceLock::new(),
    });
    let _ = state.node_router.set(api::node_router(Arc::clone(&state)));

    ws::spawn_emitter(Arc::clone(&state));
    session::spawn_monitor_recovery(Arc::clone(&state));
    session::spawn_leg_failure_reporter(Arc::clone(&state), leg_failure_rx);
    session::spawn_leg_file_recorder(Arc::clone(&state), leg_file_rx);

    // ── Controller (optional, toggleable at runtime) ──────────────────────────
    if controller_enabled {
        Controller::enable(&state).await?;
    }

    let _mdns = controller::discovery::register_mdns_service(&node_id, &node_name, args.port)
        .inspect_err(
            |e| warn!(error = %e, "mDNS registration failed; add this node to a controller by URL"),
        )
        .ok();

    // ── HTTP server ───────────────────────────────────────────────────────────
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let router = api::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "listening");
    axum::serve(listener, router).await?;

    Ok(())
}
