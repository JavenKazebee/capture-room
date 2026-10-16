//! Running the node as a service in the user's login session: it starts at
//! login (unless turned off), restarts if it crashes, and stays stopped
//! after a clean quit. `capture-room service install` sets it up, and the
//! installer scripts and the updater go through it, so there's one way the
//! node is set up on each OS. macOS only so far (a LaunchAgent); see
//! ARCHITECTURE.md (Deployment).

use std::path::PathBuf;

use anyhow::Result;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as platform;

/// Set in the service's environment, so the node knows it runs as one.
const SERVICE_ENV: &str = "CAPTURE_ROOM_SERVICE";
/// Passed on to the service when set at install: where the updater looks
/// for releases instead of GitHub (a folder served over HTTP, for testing).
pub const RELEASES_ENV: &str = "CAPTURE_ROOM_RELEASES";
/// The exit code that asks the service manager to start the node again
/// (anything but 0 does; this one says it was on purpose).
pub const RESTART_EXIT_CODE: i32 = 75;

#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    /// Install the node as a service that starts at login, or update the
    /// service to this copy of the app, and start it.
    Install {
        /// Make the node a controller (remembered).
        #[arg(long)]
        controller: bool,
        /// API and UI port [default: the installed service's, else 7700].
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stop the service and remove it and the app. Recordings stay.
    Uninstall {
        /// Also remove the node's database and logs.
        #[arg(long)]
        purge: bool,
    },
    /// Say whether the service is installed and running.
    Status,
}

/// Run a `capture-room service …` command.
pub fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Install { controller, port } => {
            let installed = platform::install(controller, port)?;
            println!(
                "Capture Room is running: http://localhost:{}",
                installed.port
            );
            println!("Logs: {}", installed.log.display());
        }
        Command::Uninstall { purge } => {
            platform::uninstall(purge)?;
            println!(
                "Removed the service and the app. Recordings in ~/capture-room are untouched."
            );
            if !purge {
                println!(
                    "Kept the database in {}; --purge removes it and the logs.",
                    data_dir().display()
                );
            }
        }
        Command::Status => match platform::installed_port() {
            None => println!("Not installed."),
            Some(port) => println!(
                "Installed on port {port}, {}; starts at login: {}.",
                if platform::is_running() {
                    "running"
                } else {
                    "stopped"
                },
                if platform::start_at_login() {
                    "yes"
                } else {
                    "no"
                },
            ),
        },
    }
    Ok(())
}

/// Whether this process was started by the service manager.
pub fn running_as_service() -> bool {
    std::env::var_os(SERVICE_ENV).is_some()
}

/// Whether the service starts the node when the user logs in.
pub fn start_at_login() -> bool {
    platform::start_at_login()
}

/// Turn starting at login on or off; the running node carries on.
pub fn set_start_at_login(on: bool) -> Result<()> {
    platform::set_start_at_login(on)
}

/// The installed app this process runs from (`Capture Room.app` on macOS),
/// if it runs from one.
pub fn app_path() -> Option<PathBuf> {
    platform::app_path()
}

/// The service's database and other state.
pub fn data_dir() -> PathBuf {
    platform::data_dir()
}

/// Started from the Finder (the app opened with no arguments): make sure the
/// service is installed and running, open the UI in the browser, and return
/// `true` so the process exits instead of running a second node.
pub fn launched_from_desktop() -> Result<bool> {
    if running_as_service() || std::env::args_os().len() > 1 || app_path().is_none() {
        return Ok(false);
    }
    let port = match platform::installed_port() {
        Some(port) => {
            platform::start()?;
            port
        }
        None => platform::install(false, None)?.port,
    };
    open_url(&format!("http://localhost:{port}"));
    Ok(true)
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opener = "xdg-open";
    if let Err(e) = std::process::Command::new(opener).arg(url).spawn() {
        tracing::warn!(error = %e, url, "couldn't open the browser");
    }
}

/// What `install` set up.
pub struct Installed {
    pub port: u16,
    pub log: PathBuf,
}
