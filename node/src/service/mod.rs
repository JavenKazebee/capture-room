//! Running the node as a service in the user's login session: starts at login
//! (unless turned off), restarts on a crash, stays stopped after a clean quit.
//! The installer scripts and updater go through this too. macOS and Linux so
//! far.

use std::path::PathBuf;

use anyhow::Result;

#[cfg(unix)]
pub mod log_rotation;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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

/// The installed service's database, if it's installed and running from it.
pub fn installed_db() -> Option<PathBuf> {
    platform::installed_db()
}

pub fn is_running() -> bool {
    platform::is_running()
}

/// Restart the installed service, which stops its recordings.
pub fn restart() -> Result<()> {
    platform::restart()
}

pub fn running_as_service() -> bool {
    std::env::var_os(SERVICE_ENV).is_some()
}

/// Where the service writes this node's output, if it runs as the service.
pub fn log_file() -> Option<PathBuf> {
    running_as_service().then(platform::log_file).flatten()
}

/// Whether the service starts the node when the user logs in. For the menu
/// bar icon, which Linux doesn't have yet.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn start_at_login() -> bool {
    platform::start_at_login()
}

/// Turn starting at login on or off; the running node carries on.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn set_start_at_login(on: bool) -> Result<()> {
    platform::set_start_at_login(on)
}

/// The installed app this process runs from (`Capture Room.app` on macOS),
/// if it runs from one.
pub fn app_path() -> Option<PathBuf> {
    platform::app_path()
}

pub fn data_dir() -> PathBuf {
    platform::data_dir()
}

/// Started from the Finder (the app opened with no arguments; on Linux, the
/// installed binary run with none): make sure the service is installed and
/// running, open the UI in the browser, and return `true` so the process
/// exits instead of running a second node.
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
    let url = crate::auth::installed_sign_in_url(port);
    // Run from a shell, perhaps with no browser to open.
    #[cfg(target_os = "linux")]
    println!("Capture Room is running: {url}");
    open_url(&url);
    Ok(true)
}

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

/// Put `text` on the clipboard.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    #[cfg(target_os = "macos")]
    let copier = "pbcopy";
    #[cfg(target_os = "windows")]
    let copier = "clip";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let copier = "wl-copy";
    let mut child = std::process::Command::new(copier)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(text.as_bytes())?;
    anyhow::ensure!(child.wait()?.success(), "{copier} failed");
    Ok(())
}

pub struct Installed {
    pub port: u16,
    pub log: PathBuf,
}
