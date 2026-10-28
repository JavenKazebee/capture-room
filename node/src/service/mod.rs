//! Running the node as a service in the user's login session: starts at login
//! (unless turned off), restarts on a crash, stays stopped after a clean quit.
//! The installer scripts and updater go through this too. macOS, Linux and
//! Windows so far.

use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use anyhow::bail;
use anyhow::{Context, Result};

use crate::api::types::UpdateChannel;

#[cfg(any(unix, windows))]
pub mod log_rotation;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod windows_flags;
#[cfg(target_os = "windows")]
pub mod windows_quit;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
use unsupported as platform;

/// Set in the service's environment, so the node knows it runs as one.
const SERVICE_ENV: &str = "CAPTURE_ROOM_SERVICE";
/// Passed on to the service when set at install: where the updater looks
/// for releases instead of GitHub (a folder served over HTTP, for testing).
pub const RELEASES_ENV: &str = "CAPTURE_ROOM_RELEASES";
/// The exit code that asks the service manager to start the node again
/// (anything but 0 does; this one says it was on purpose).
pub const RESTART_EXIT_CODE: i32 = 75;
pub const DEFAULT_PORT: u16 = 7700;
/// Where an update is unpacked, beside the app (one volume, so the swap is
/// two renames). The updater clears it; uninstalling removes what's left.
const UPDATE_STAGING: &str = ".capture-room-update";

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
        /// Offer pre-releases (betas, release candidates) as updates too
        /// (remembered; Settings › Updates changes it).
        #[arg(long)]
        prerelease: bool,
    },
    /// Stop the service and remove it and the app. Recordings stay.
    Uninstall {
        /// Also remove the node's database and logs.
        #[arg(long)]
        purge: bool,
    },
    /// Say whether the service is installed and running.
    Status,
    /// Run the node under a supervisor that restarts it: what the service
    /// starts (Windows has no service manager that would).
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    Run,
}

/// Run a `capture-room service …` command.
pub fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Install {
            controller,
            port,
            prerelease,
        } => {
            if prerelease {
                set_update_channel(UpdateChannel::Prerelease)?;
            }
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
                "Removed the service and the app. Recordings in {} are untouched.",
                crate::storage::default_folder().display()
            );
            if !purge {
                println!(
                    "Kept the database in {}; --purge removes it and the logs.",
                    data_dir().display()
                );
            }
        }
        Command::Status => match installed_port() {
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
        #[cfg(target_os = "windows")]
        Command::Run => platform::run_supervisor()?,
    }
    Ok(())
}

/// Set the channel in the service's database before it starts, not as a
/// service argument: that would set it again on every start, undoing a change
/// made in Settings.
fn set_update_channel(channel: UpdateChannel) -> Result<()> {
    let db = installed_db().unwrap_or_else(|| data_dir().join("capture-room.db"));
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let pool = crate::db::init(&db).await?;
            crate::db::update_channel_set(&pool, channel).await?;
            pool.close().await;
            anyhow::Ok(())
        })
}

/// The port the installed service runs on, if it's installed.
pub fn installed_port() -> Option<u16> {
    platform::installed_arg("--port")?.parse().ok()
}

/// The installed service's database, if it's installed and running from it.
pub fn installed_db() -> Option<PathBuf> {
    platform::installed_arg("--db").map(PathBuf::from)
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
/// bar icon.
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
    let port = match installed_port() {
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

/// Put `text` on the clipboard, for the menu bar icon.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    #[cfg(target_os = "macos")]
    let copiers: &[&[&str]] = &[&["pbcopy"]];
    #[cfg(target_os = "windows")]
    let copiers: &[&[&str]] = &[&["clip"]];
    // Not in the distros' base installs; each stays behind to serve the
    // clipboard, as it has to on both window systems.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let copiers: &[&[&str]] = &[
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
        &["xsel", "--clipboard", "--input"],
    ];
    for copier in copiers {
        let mut child = match std::process::Command::new(copier[0])
            .args(&copier[1..])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        child.stdin.take().unwrap().write_all(text.as_bytes())?;
        anyhow::ensure!(child.wait()?.success(), "{} failed", copier[0]);
        return Ok(());
    }
    anyhow::bail!("no clipboard tool: install wl-clipboard or xclip")
}

pub struct Installed {
    pub port: u16,
    pub log: PathBuf,
}

/// The folder an update to `app` is unpacked in.
pub fn update_staging(app: &Path) -> PathBuf {
    app.with_file_name(UPDATE_STAGING)
}

/// The node's arguments after its executable: the database in the data
/// folder, the port, and whether it's a controller.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn node_args(controller: bool, port: u16) -> Vec<String> {
    let mut args = vec![
        "--db".to_string(),
        data_dir().join("capture-room.db").display().to_string(),
        "--port".into(),
        port.to_string(),
    ];
    if controller {
        args.push("--controller".into());
    }
    args
}

/// The service's environment: [`SERVICE_ENV`], and [`RELEASES_ENV`] when
/// it's set for the install.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn node_env() -> Vec<(String, String)> {
    let mut env = vec![(SERVICE_ENV.to_string(), "1".to_string())];
    if let Ok(releases) = std::env::var(RELEASES_ENV) {
        env.push((RELEASES_ENV.to_string(), releases));
    }
    env
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn answers(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// Wait for the node to answer on `port`, giving up early once the service
/// has stopped trying.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn wait_for_port(port: u16) -> Result<()> {
    let deadline = Instant::now() + platform::START_TIMEOUT;
    while Instant::now() < deadline {
        if answers(port) {
            return Ok(());
        }
        if !platform::starting() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!(
        "the node didn't answer on port {port}; see {}{}",
        platform::log_file().unwrap_or_default().display(),
        platform::MORE_LOGS
    )
}

/// Remove a file or folder, if it's there.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn remove(path: &Path) -> Result<()> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("remove {}", path.display()))
        }
        _ => Ok(()),
    }
}
