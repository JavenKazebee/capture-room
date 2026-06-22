mod sources;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use tracing::info;

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

    tokio::signal::ctrl_c().await?;
    info!("shutting down");
    Ok(())
}
