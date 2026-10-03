mod api;
mod audio;
mod controller;
mod db;
mod pipeline;
mod plugins;
mod recording;
mod sources;
mod state;
mod storage;
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
use controller::Controller;
use pipeline::monitor::MonitorConfig;
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
    let controller_enabled = db::config_get(&pool, controller::CONFIG_KEY).await?.as_deref() == Some("true");
    info!(id = %node_id, name = %node_name, controller = controller_enabled, "identity");

    db::sessions_mark_crashed(&pool).await?;

    // ── Source manager ────────────────────────────────────────────────────────
    let test_configs: Vec<sources::test::TestSourceConfig> = db::test_sources_list(&pool)
        .await?
        .into_iter()
        .map(|row| {
            use sources::test::{AudioTestSignal, TestSourceConfig, VideoTestPattern};
            TestSourceConfig {
                id: row.id,
                name: row.name,
                pattern: VideoTestPattern::from_db(&row.pattern),
                width: row.width as u32,
                height: row.height as u32,
                fps_num: row.fps_num as u32,
                fps_den: row.fps_den as u32,
                audio_signal: AudioTestSignal::from_db(&row.audio_signal),
                frequency: row.frequency,
                channels: row.channels as u32,
            }
        })
        .collect();

    let monitor_config = load_monitor_config(&pool).await;
    let ndi_monitor = tokio::task::spawn_blocking(sources::ndi::NdiMonitor::start)
        .await
        .expect("NDI monitor thread panicked");
    let mut source_manager = SourceManager::new(monitor_config, ndi_monitor);
    source_manager.scan(&test_configs);

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
        controller: RwLock::new(None),
        http: reqwest::Client::new(),
        node_router: std::sync::OnceLock::new(),
    });
    let _ = state.node_router.set(api::node_router(Arc::clone(&state)));

    // ── Periodic WS emitter ───────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            let mut tick: u32 = 0;
            loop {
                interval.tick().await;
                tick = tick.wrapping_add(1);

                let mgr = state.source_manager.read().await;

                // Audio levels for every monitored source (~10 fps).
                for (source_id, channels) in mgr.all_audio_levels() {
                    state.emit(&WsEvent::AudioLevels { source_id, channels });
                }

                // Timecode (feed.status) at 1 Hz — every 10 ticks.
                if tick % 10 == 0 {
                    for source in mgr.sources() {
                        state.emit(&WsEvent::FeedStatus {
                            source_id: source.id().to_string(),
                            timecode: source.timecode().map(|tc| tc.to_string()),
                            duration_secs: 0.0,
                        });
                    }
                }

                // Thumbnail updates at the configured fps.
                // Tick interval = 100 ms, so 10 ticks = 1 s.
                // fps=1 → every 10 ticks, fps=2 → every 5, fps=10 → every 1.
                let fps = mgr.monitor_config().thumb_fps_num.max(1) as u32;
                let thumb_div = (10 / fps).max(1);
                if tick % thumb_div == 0 {
                    for source in mgr.sources() {
                        if mgr.is_monitored(source.id()) {
                            state.emit(&WsEvent::ThumbnailUpdated {
                                source_id: source.id().to_string(),
                                url: format!("/api/v1/node/thumbnails/{}", source.id()),
                            });
                        }
                    }
                }
            }
        });
    }

    // ── Controller (optional, toggleable at runtime) ──────────────────────────
    if controller_enabled {
        Controller::enable(&state).await?;
    }

    let _mdns = controller::discovery::register_mdns_service(&node_id, &node_name, args.port);

    // ── HTTP server ───────────────────────────────────────────────────────────
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let router = api::build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "listening");
    axum::serve(listener, router).await?;

    Ok(())
}

async fn load_monitor_config(pool: &sqlx::SqlitePool) -> MonitorConfig {
    let def = MonitorConfig::default();
    let thumb_fps = db::config_get(pool, "monitor_thumb_fps")
        .await.ok().flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(def.thumb_fps_num);
    let thumb_width = db::config_get(pool, "monitor_thumb_width")
        .await.ok().flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(def.thumb_width);
    let thumb_height = db::config_get(pool, "monitor_thumb_height")
        .await.ok().flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(def.thumb_height);
    let level_ms = db::config_get(pool, "monitor_level_ms")
        .await.ok().flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(def.level_interval_ns / 1_000_000);
    MonitorConfig {
        thumb_fps_num: thumb_fps,
        thumb_fps_den: 1,
        thumb_width,
        thumb_height,
        level_interval_ns: level_ms * 1_000_000,
    }
}
