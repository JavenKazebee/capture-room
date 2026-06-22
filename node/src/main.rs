mod pipeline;
mod plugins;
mod sources;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use tracing::info;

use pipeline::{profile::RecordingProfile, Pipeline};
use sources::registry::SourceRegistry;

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

    let mut registry = SourceRegistry::new();
    registry.scan()?;

    for source in registry.sources() {
        info!(
            id = source.id(),
            name = source.display_name(),
            available = source.is_available(),
            "source"
        );
    }

    match args.mode {
        Mode::Node => info!(port = args.port, "starting in node mode"),
        Mode::Controller => info!(port = args.port, "starting in controller mode"),
    }

    // ── Dev smoke-test: record 3 seconds from test-1 ─────────────────────────
    if let Some(source) = registry.get("test-1") {
        let profile = RecordingProfile::h264_mov("dev-test");
        let output = std::path::Path::new("/tmp/capture-room-test.mov");

        info!(path = %output.display(), "starting test recording");
        let p = Pipeline::new(source, output, &profile)?;
        p.start()?;

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        info!("stopping test recording");
        p.stop(10).await?;
        info!(path = %output.display(), "recording complete");
    }

    tokio::signal::ctrl_c().await?;
    info!("shutting down");
    Ok(())
}
