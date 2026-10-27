//! The service on macOS: a LaunchAgent in the user's login session. Not a
//! LaunchDaemon: a daemon can't be granted camera, microphone or screen
//! recording permission.
//!
//! The plist lives in Application Support and the service always loads from
//! there; start-at-login is a copy in `~/Library/LaunchAgents`, so toggling it
//! never touches the running node. `KeepAlive` with `SuccessfulExit = false`
//! restarts on errors and kills but not a clean quit; a `Crashed` condition
//! would miss panics and kills.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use plist::{Dictionary, Value};

use super::{
    home, installed_port, node_args, node_env, remove, update_staging, wait_for_port, Installed,
    DEFAULT_PORT,
};

const LABEL: &str = "com.capture-room.node";
/// The first start after an install scans every plugin, which takes about
/// 30 s on an Intel Mac.
pub const START_TIMEOUT: Duration = Duration::from_secs(90);
/// Nothing to add to the log file when the node doesn't start.
pub const MORE_LOGS: &str = "";

pub fn data_dir() -> PathBuf {
    home().join("Library/Application Support/Capture Room")
}

fn log_dir() -> PathBuf {
    home().join("Library/Logs/Capture Room")
}

pub fn log_file() -> Option<PathBuf> {
    Some(log_dir().join("capture-room.log"))
}

fn plist_path() -> PathBuf {
    data_dir().join(format!("{LABEL}.plist"))
}

fn login_plist_path() -> PathBuf {
    home().join(format!("Library/LaunchAgents/{LABEL}.plist"))
}

fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn target() -> String {
    format!("{}/{LABEL}", domain())
}

/// `Capture Room.app`, when this binary is the one in its `Contents/MacOS`.
pub fn app_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

pub fn install(controller: bool, port: Option<u16>) -> Result<Installed> {
    let app = app_path().context("run this from the installed Capture Room.app")?;
    let exe = app.join("Contents/MacOS/capture-room");
    let port = port.or_else(installed_port).unwrap_or(DEFAULT_PORT);
    // A fresh install starts at login; a reinstall keeps the choice.
    let at_login = !plist_path().exists() || start_at_login();

    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(log_dir())?;
    let log = log_dir().join("capture-room.log");

    let mut args = vec![exe.display().to_string()];
    args.extend(node_args(controller, port));
    let env: Dictionary = node_env()
        .into_iter()
        .map(|(k, v)| (k, Value::from(v)))
        .collect();
    let mut keep_alive = Dictionary::new();
    keep_alive.insert("SuccessfulExit".into(), false.into());

    let mut job = Dictionary::new();
    job.insert("Label".into(), LABEL.into());
    job.insert(
        "ProgramArguments".into(),
        Value::Array(args.into_iter().map(Value::from).collect()),
    );
    job.insert("EnvironmentVariables".into(), env.into());
    job.insert("KeepAlive".into(), keep_alive.into());
    job.insert("LimitLoadToSessionType".into(), "Aqua".into());
    // Not throttled like a background job: it handles live video.
    job.insert("ProcessType".into(), "Interactive".into());
    job.insert("StandardOutPath".into(), log.display().to_string().into());
    job.insert("StandardErrorPath".into(), log.display().to_string().into());

    stop()?;
    Value::Dictionary(job).to_file_xml(plist_path())?;
    set_start_at_login(at_login)?;
    start()?;
    wait_for_port(port)?;
    Ok(Installed { port, log })
}

pub fn uninstall(purge: bool) -> Result<()> {
    stop()?;
    remove(&login_plist_path())?;
    remove(&plist_path())?;
    let apps = [
        app_path(),
        Some(PathBuf::from("/Applications/Capture Room.app")),
        Some(home().join("Applications/Capture Room.app")),
    ];
    for app in apps.into_iter().flatten() {
        remove(&update_staging(&app))?;
        remove(&app)?;
    }
    remove(&home().join("Library/Caches/capture-room"))?;
    if purge {
        remove(&data_dir())?;
        remove(&log_dir())?;
    }
    Ok(())
}

/// The value after `flag` in the installed service's arguments.
pub fn installed_arg(flag: &str) -> Option<String> {
    let job = Value::from_file(plist_path()).ok()?;
    let args = job.as_dictionary()?.get("ProgramArguments")?.as_array()?;
    let at = args.iter().position(|a| a.as_string() == Some(flag))?;
    Some(args.get(at + 1)?.as_string()?.to_string())
}

pub fn is_running() -> bool {
    Command::new("launchctl")
        .args(["print", &target()])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("state = running"))
}

pub fn start_at_login() -> bool {
    login_plist_path().exists()
}

pub fn set_start_at_login(on: bool) -> Result<()> {
    let login = login_plist_path();
    if on {
        std::fs::create_dir_all(login.parent().unwrap())?;
        std::fs::copy(plist_path(), &login).context("copy the service into LaunchAgents")?;
    } else {
        remove(&login)?;
    }
    Ok(())
}

pub fn start() -> Result<()> {
    if !loaded() {
        launchctl(&["bootstrap", &domain(), &plist_path().display().to_string()])?;
    }
    launchctl(&["kickstart", &target()])
}

/// Restart the running service (stopping anything it's doing).
pub fn restart() -> Result<()> {
    launchctl(&["kickstart", "-k", &target()])?;
    wait_for_port(installed_port().unwrap_or(DEFAULT_PORT))
}

fn stop() -> Result<()> {
    if !loaded() {
        return Ok(());
    }
    launchctl(&["bootout", &target()])?;
    // bootout returns before the process has gone.
    let deadline = Instant::now() + Duration::from_secs(30);
    while loaded() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Whether launchd still has the service, which it drops when it gives up.
pub fn starting() -> bool {
    loaded()
}

fn loaded() -> bool {
    Command::new("launchctl")
        .args(["print", &target()])
        .output()
        .is_ok_and(|out| out.status.success())
}

fn launchctl(args: &[&str]) -> Result<()> {
    let out = Command::new("launchctl").args(args).output()?;
    if !out.status.success() {
        bail!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
