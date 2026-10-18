mod api;
mod auth;
mod benchmark;
mod capacity;
mod checks;
mod clock;
mod controller;
mod db;
mod display;
mod logs;
mod pipeline;
mod plugins;
mod service;
mod session;
mod sources;
mod state;
mod storage;
mod thumbnails;
mod tls;
mod tray;
mod update;
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

    /// UDP port to serve the clock on while a controller [default: port + 100]
    #[arg(long)]
    clock_port: Option<u16>,

    #[arg(long, default_value = "capture-room.db")]
    db: String,

    /// Show the menu bar icon (the installed service always does).
    #[arg(long)]
    tray: bool,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Install, remove or inspect the login service.
    Service {
        #[command(subcommand)]
        command: service::Command,
    },
    /// Show or clear a node's access token, when it's been lost.
    AccessToken {
        #[command(subcommand)]
        command: auth::Command,
    },
}

/// Replay buffers (`.replay`) and clips (`.replay-clips`), per node, beside
/// the default recording folder: one volume, so clips can hard-link buffers.
fn replay_root(folder: &str, node_id: &str) -> std::path::PathBuf {
    storage::default_folder().join(folder).join(node_id)
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    match args.command.take() {
        Some(Cmd::Service { command }) => return service::run_command(command),
        Some(Cmd::AccessToken { command }) => return auth::run_command(command),
        None => {}
    }
    if service::launched_from_desktop()? {
        return Ok(());
    }
    // Before the runtime's threads start: these set environment variables.
    #[cfg(target_os = "macos")]
    let bundled_gst = plugins::use_bundled_gstreamer();
    #[cfg(not(target_os = "macos"))]
    let bundled_gst: Option<std::path::PathBuf> = None;
    let ndi_dir = plugins::locate_ndi_runtime();
    let (tray_tx, tray_rx) = tokio::sync::mpsc::unbounded_channel();
    let tray = (args.tray || service::running_as_service()).then(|| tray::Config {
        port: args.port,
        actions: tray_tx,
    });
    let tray_rx = tray.is_some().then_some(tray_rx);
    // The main thread runs the window loop for display outputs and the menu
    // bar icon, where there's a window system; the node runs beside it.
    display::run(tray, move |displays| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(args, bundled_gst, ndi_dir, displays, tray_rx))
    })
}

async fn run(
    args: Args,
    bundled_gst: Option<std::path::PathBuf>,
    ndi_dir: Option<&'static str>,
    displays: display::Displays,
    tray_actions: Option<tokio::sync::mpsc::UnboundedReceiver<tray::Action>>,
) -> Result<()> {
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "capture_room=debug,info".into()),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    // No colour codes in a service's log file.
                    .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout())),
            )
            // Kept for the Logs view.
            .with(logs::layer())
            .init();
    }
    #[cfg(unix)]
    service::log_rotation::spawn();

    if let Some(dir) = bundled_gst {
        info!(dir = %dir.display(), "using the GStreamer bundled in");
    }
    if let Some(dir) = ndi_dir {
        info!(dir, "using the NDI library found in");
    }

    gstreamer::init().expect("GStreamer init failed");
    gstndi::plugin_register_static().expect("NDI plugin registration failed");
    gstfallbackswitch::plugin_register_static().expect("fallbackswitch plugin registration failed");
    // Before the WebRTC elements list the encoders they can use.
    pipeline::whep::hide_unsupported_encoders();
    gstrswebrtc::plugin_register_static().expect("webrtc plugin registration failed");
    plugins::check_required_plugins()?;
    let pool = db::init(&args.db).await?;

    let node_id = match db::config_get(&pool, "uuid").await? {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            db::config_set(&pool, "uuid", &id).await?;
            id
        }
    };
    let hostname = controller::discovery::local_hostname();
    let node_name = db::config_get(&pool, "name").await?.unwrap_or_else(|| {
        hostname
            .clone()
            .unwrap_or_else(|| "capture-room-node".to_string())
    });
    let identity = tls::load_identity(&pool, hostname.as_deref()).await?;
    let tls_config = tls::server_config(&identity)?;

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

    let configured = db::configured_sources_list(&pool).await?;
    let monitor_config = db::monitor_settings_get(&pool).await?;
    let ndi_monitor = tokio::task::spawn_blocking(sources::ndi::NdiMonitor::start)
        .await
        .expect("NDI monitor thread panicked");
    let devices = tokio::task::spawn_blocking(sources::device::LocalDevices::start)
        .await
        .expect("device monitor thread panicked");
    devices
        .decklink_profiles
        .replace(db::decklink_profiles_get(&pool).await?);
    let node_clock = Arc::new(clock::NodeClock::new(&node_id));
    let (leg_failure_tx, leg_failure_rx) = tokio::sync::mpsc::unbounded_channel();
    let (leg_file_tx, leg_file_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut source_manager = SourceManager::new(
        monitor_config,
        node_clock.current(),
        ndi_monitor,
        std::sync::Arc::new(devices),
        displays.clone(),
        leg_failure_tx,
        leg_file_tx,
    );
    source_manager.set_node_name(&node_name);
    source_manager.set_ndi_subscriptions(db::ndi_subscriptions_get(&pool).await?);
    source_manager.set_replay_root(
        replay_root(".replay", &node_id),
        replay_root(".replay-clips", &node_id),
    );
    source_manager.scan(&configured);
    for (source_id, config) in db::replay_buffers_list(&pool).await? {
        source_manager.set_replay(&source_id, Some(config));
    }

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
        clock: node_clock,
        port: args.port,
        clock_port: args.clock_port.unwrap_or(args.port.saturating_add(100)),
        controller: RwLock::new(None),
        http: reqwest::Client::new(),
        peers: tls::Peers::new()?,
        fingerprint: identity.fingerprint.clone(),
        node_router: std::sync::OnceLock::new(),
        updater: update::Updater::default(),
        access_token: auth::AccessToken::default(),
    });
    auth::load(&state).await?;
    let _ = state.node_router.set(api::node_router(Arc::clone(&state)));

    ws::spawn_emitter(Arc::clone(&state));
    session::spawn_monitor_recovery(Arc::clone(&state));
    session::spawn_clock_follower(Arc::clone(&state));
    session::spawn_leg_failure_reporter(Arc::clone(&state), leg_failure_rx);
    session::spawn_leg_file_recorder(Arc::clone(&state), leg_file_rx);
    session::spawn_quit_on_signal(Arc::clone(&state));
    update::spawn_checker(Arc::clone(&state));
    if let Some(actions) = tray_actions {
        tray::spawn(Arc::clone(&state), displays, actions);
    }

    if controller_enabled {
        Controller::enable(&state).await?;
    }

    let _mdns = controller::discovery::register_mdns_service(&node_id, &node_name, args.port)
        .inspect_err(
            |e| warn!(error = %e, "mDNS registration failed; add this node to a controller by URL"),
        )
        .ok();

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let router = api::build_router(state);
    let listener = tls::TlsListener::new(tokio::net::TcpListener::bind(addr).await?, tls_config)?;
    info!(addr = %addr, fingerprint = %identity.fingerprint, "listening");
    // Connect info: whether a request came over TLS, and the address a
    // controller claims a node from (its clock is served from there).
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<tls::Conn>(),
    )
    .await?;

    Ok(())
}
