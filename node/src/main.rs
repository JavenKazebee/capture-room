//! Capture Room's node: one binary, and every running instance is a node. A
//! node serves its own machine (sources, storage, recordings, channels,
//! replay buffers) and knows nothing about other nodes. Any node can also be
//! a controller ([`controller`]), which finds other nodes, forwards commands
//! to them and merges their events. Every instance serves the web UI,
//! embedded from `ui/dist` in release builds.

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
mod power;
#[cfg(target_os = "windows")]
mod registry;
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

use anyhow::{Context, Result};
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

    #[arg(long, default_value_t = service::DEFAULT_PORT)]
    port: u16,

    /// UDP port to serve the clock on while a controller [default: port + 100]
    #[arg(long)]
    clock_port: Option<u16>,

    #[arg(long, default_value = "capture-room.db")]
    db: std::path::PathBuf,

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
    /// Show a node's access token and pairing code, or replace them.
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

/// Raise the open-file limit as far as the system allows. A recording feed
/// holds about a dozen (its files, and a socket pair for each pipeline's
/// bus), and macOS starts an app with 256: fewer than 20 feeds.
#[cfg(unix)]
fn raise_open_file_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is an rlimit, which is what this call writes.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        warn!(error = %std::io::Error::last_os_error(), "couldn't read the open-file limit");
        return;
    }
    // macOS reports no hard limit but refuses more than this.
    #[cfg(target_os = "macos")]
    let max = limit.rlim_max.min(macos_max_files_per_process());
    #[cfg(not(target_os = "macos"))]
    let max = limit.rlim_max;
    if max <= limit.rlim_cur {
        return;
    }
    let raised = libc::rlimit {
        rlim_cur: max,
        rlim_max: limit.rlim_max,
    };
    // SAFETY: a valid rlimit, read by the call.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
        info!(
            from = limit.rlim_cur,
            to = max,
            "raised the open-file limit"
        );
    } else {
        warn!(
            limit = limit.rlim_cur,
            error = %std::io::Error::last_os_error(),
            "couldn't raise the open-file limit; recording many feeds may fail"
        );
    }
}

/// `kern.maxfilesperproc`, or the classic 10240 if it can't be read.
#[cfg(target_os = "macos")]
fn macos_max_files_per_process() -> libc::rlim_t {
    let mut max: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is a C string and `max` is an int `len` bytes long,
    // which is what this sysctl writes.
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&mut max as *mut libc::c_int).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if ok && max > 0 {
        max as libc::rlim_t
    } else {
        10240
    }
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
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let bundled_gst = plugins::use_bundled_gstreamer();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let bundled_gst: Option<std::path::PathBuf> = None;
    let ndi_dir = plugins::locate_ndi_runtime();
    let (tray, tray_rx) = if args.tray || service::running_as_service() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Some(tray::Config { actions: tx }), Some(rx))
    } else {
        (None, None)
    };
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
        use tracing_subscriber::{EnvFilter, Layer};
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    // No colour codes in a service's log file.
                    .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
                    // The terminal and the service's log file: RUST_LOG for more.
                    .with_filter(
                        EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
                    ),
            )
            // Kept for the Logs view, which can show debug lines too.
            .with(logs::layer().with_filter(EnvFilter::new("capture_room=debug,info")))
            .init();
    }
    #[cfg(unix)]
    service::log_rotation::spawn();
    #[cfg(unix)]
    raise_open_file_limit();

    if let Some(dir) = &bundled_gst {
        info!(dir = %dir.display(), "using the GStreamer bundled in");
    }
    if let Some(dir) = ndi_dir {
        info!(dir, "using the NDI library found in");
    }

    gstreamer::init().context("couldn't start GStreamer")?;
    gstndi::plugin_register_static().context("couldn't register the NDI plugin")?;
    gstfallbackswitch::plugin_register_static()
        .context("couldn't register the fallbackswitch plugin")?;
    // Before the WebRTC elements list the encoders they can use.
    pipeline::whep::hide_unsupported_encoders();
    gstrswebrtc::plugin_register_static().context("couldn't register the WebRTC plugin")?;
    #[cfg(target_os = "linux")]
    sources::x11::register().context("couldn't register the X11 screen provider")?;
    plugins::check_required_plugins(bundled_gst.is_some())?;
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
    devices
        .screencasts
        .replace(db::screencasts_get(&pool).await?);
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
    let (_, work) = source_manager.scan(&configured);
    source_manager.run_now(work);
    for (source_id, config) in db::replay_buffers_list(&pool).await? {
        let work = source_manager.set_replay(&source_id, Some(config));
        source_manager.run_now(work);
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
    session::spawn_power_keeper(Arc::clone(&state));
    session::spawn_screencast_saver(Arc::clone(&state));
    state
        .updater
        .set_channel(db::update_channel_get(&state.db).await?);
    update::spawn_checker(Arc::clone(&state));
    // Without a menu bar, nothing else signs a browser in: print a link. A
    // headless Linux service has no window loop to show the menu bar on.
    let tray_actions = tray_actions.filter(|_| displays.available());
    let print_sign_in_link = tray_actions.is_none();
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
    let router = api::build_router(Arc::clone(&state));
    let listener = tls::TlsListener::new(tokio::net::TcpListener::bind(addr).await?, tls_config)?;
    info!(addr = %addr, fingerprint = %identity.fingerprint, "listening");
    if print_sign_in_link {
        auth::print_sign_in_link(&state).await;
    }
    // Connect info: whether a request came over TLS, and the address a
    // controller claims a node from (its clock is served from there).
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<tls::Conn>(),
    )
    .await?;

    Ok(())
}
