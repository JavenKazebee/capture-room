use clap::{Parser, ValueEnum};
use tracing::info;

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
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "capture_room=debug,info".into()),
        )
        .init();

    let args = Args::parse();

    match args.mode {
        Mode::Node => {
            info!(port = args.port, "starting in node mode");
        }
        Mode::Controller => {
            info!(port = args.port, "starting in controller mode");
        }
    }

    // TODO: initialise subsystems per mode
    tokio::signal::ctrl_c().await.unwrap();
    info!("shutting down");
}
