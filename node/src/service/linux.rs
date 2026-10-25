//! The service on Linux: a systemd user unit.
//!
//! The app is a folder in `$XDG_DATA_HOME/capture-room/app` (the binary and
//! its licenses), beside the database; the log goes to
//! `$XDG_STATE_HOME/capture-room`. The unit is written to the user's systemd
//! folder and start-at-login is `systemctl --user enable`, so toggling it
//! never touches the running node. `Restart=on-failure` restarts on errors,
//! kills and exit 75 but not a clean quit. Lingering is turned on so a
//! headless node starts at boot, not at its user's first login.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::{Installed, RELEASES_ENV, SERVICE_ENV};

const UNIT: &str = "capture-room.service";
const DEFAULT_PORT: u16 = 7700;
/// The first start after an install scans every plugin.
const START_TIMEOUT: Duration = Duration::from_secs(90);
/// Set in the service's environment: its log file, which may not be where
/// this process's XDG variables say (the user manager has its own).
const LOG_ENV: &str = "CAPTURE_ROOM_LOG";
/// In the data folder when the install turned lingering on, so uninstalling
/// turns it off again (and leaves it alone when it was on already).
const LINGER_MARKER: &str = ".enabled-linger";

fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// An XDG base folder: the variable if it's set to an absolute path (as
/// the spec says), else its default under the home folder.
fn xdg(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(default))
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("capture-room")
}

fn log_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("capture-room")
}

pub fn log_file() -> Option<PathBuf> {
    Some(
        std::env::var_os(LOG_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| log_dir().join("capture-room.log")),
    )
}

fn unit_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
        .join("systemd/user")
        .join(UNIT)
}

/// The installed app folder (`…/capture-room/app`), when this binary is the
/// one in it.
pub fn app_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let app = exe.parent()?;
    (app.file_name()? == "app" && app.parent()?.file_name()? == "capture-room")
        .then(|| app.to_path_buf())
}

pub fn install(controller: bool, port: Option<u16>) -> Result<Installed> {
    let app = app_path().with_context(|| {
        format!(
            "run this from the installed app ({}/app/capture-room)",
            data_dir().display()
        )
    })?;
    let exe = app.join("capture-room");
    let port = port.or_else(installed_port).unwrap_or(DEFAULT_PORT);
    // A fresh install starts at login; a reinstall keeps the choice.
    let at_login = !unit_path().exists() || start_at_login();

    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(log_dir())?;
    std::fs::create_dir_all(unit_path().parent().unwrap())?;
    let log = log_dir().join("capture-room.log");

    let mut args = vec![
        exe.display().to_string(),
        "--db".into(),
        data_dir().join("capture-room.db").display().to_string(),
        "--port".into(),
        port.to_string(),
    ];
    if controller {
        args.push("--controller".into());
    }
    let mut env = vec![
        (SERVICE_ENV.to_string(), "1".to_string()),
        (LOG_ENV.to_string(), log.display().to_string()),
    ];
    if let Ok(releases) = std::env::var(RELEASES_ENV) {
        env.push((RELEASES_ENV.to_string(), releases));
    }

    stop()?;
    std::fs::write(unit_path(), unit(&args, &env, &log))
        .with_context(|| format!("write {}", unit_path().display()))?;
    systemctl(&["daemon-reload"])?;
    set_start_at_login(at_login)?;
    enable_linger();
    link_on_path(&exe);
    start()?;
    wait_for_port(port)?;
    Ok(Installed { port, log })
}

/// The unit file for running `args` with `env`, logging to `log`.
fn unit(args: &[String], env: &[(String, String)], log: &Path) -> String {
    let exec = args
        .iter()
        .map(|a| quote(&a.replace('%', "%%").replace('$', "$$")))
        .collect::<Vec<_>>()
        .join(" ");
    let env = env
        .iter()
        .map(|(k, v)| {
            format!(
                "Environment={}\n",
                quote(&format!("{k}={v}").replace('%', "%%"))
            )
        })
        .collect::<String>();
    let log = log.display().to_string().replace('%', "%%");
    format!(
        "# Written by `capture-room service install`, which rewrites it.\n\
         [Unit]\n\
         Description=Capture Room node\n\
         Documentation=https://github.com/JavenKazebee/capture-room\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         {env}\
         # Again after a crash, a kill or exit 75 (an update), not a clean quit.\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         StandardOutput=append:{log}\n\
         StandardError=append:{log}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Double-quoted for a unit file, with `\` and `"` escaped.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The words of a unit file's `ExecStart=` value, as [`unit`] writes it.
fn unquote_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(first) = chars.next() else {
            return words;
        };
        let mut word = String::new();
        if first == '"' {
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => word.extend(chars.next()),
                    c => word.push(c),
                }
            }
        } else {
            word.push(first);
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        words.push(word.replace("%%", "%").replace("$$", "$"));
    }
}

pub fn uninstall(purge: bool) -> Result<()> {
    stop()?;
    if unit_path().exists() {
        systemctl(&["disable", UNIT])?;
        remove(&unit_path())?;
        systemctl(&["daemon-reload"])?;
        let _ = systemctl(&["reset-failed", UNIT]);
    }
    let marker = data_dir().join(LINGER_MARKER);
    if marker.exists() {
        // Only off again if this install turned it on.
        if run("loginctl", &["disable-linger"]).is_ok() {
            remove(&marker)?;
        }
    }
    for app in [app_path(), Some(data_dir().join("app"))]
        .into_iter()
        .flatten()
    {
        remove(&app)?;
    }
    remove(&data_dir().join(".capture-room-update"))?;
    if std::fs::read_link(path_link()).is_ok_and(|to| to.starts_with(data_dir())) {
        remove(&path_link())?;
    }
    if purge {
        remove(&data_dir())?;
        remove(&log_dir())?;
    }
    Ok(())
}

/// The port the installed service runs on, if it's installed.
pub fn installed_port() -> Option<u16> {
    installed_arg("--port")?.parse().ok()
}

/// The database the installed service uses, if it's installed.
pub fn installed_db() -> Option<PathBuf> {
    installed_arg("--db").map(PathBuf::from)
}

/// The value after `flag` in the installed service's arguments.
fn installed_arg(flag: &str) -> Option<String> {
    let unit = std::fs::read_to_string(unit_path()).ok()?;
    arg_in_unit(&unit, flag)
}

fn arg_in_unit(unit: &str, flag: &str) -> Option<String> {
    let exec = unit.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
    let args = unquote_words(exec);
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).cloned()
}

pub fn is_running() -> bool {
    systemctl(&["is-active", "--quiet", UNIT]).is_ok()
}

pub fn start_at_login() -> bool {
    systemctl(&["is-enabled", "--quiet", UNIT]).is_ok()
}

pub fn set_start_at_login(on: bool) -> Result<()> {
    systemctl(&[if on { "enable" } else { "disable" }, UNIT])
}

pub fn start() -> Result<()> {
    systemctl(&["start", UNIT])
}

/// Restart the running service (stopping anything it's doing).
pub fn restart() -> Result<()> {
    systemctl(&["restart", UNIT])?;
    wait_for_port(installed_port().unwrap_or(DEFAULT_PORT))
}

/// Waits for the node to go: systemctl returns once the job is done.
fn stop() -> Result<()> {
    if !unit_path().exists() {
        return Ok(());
    }
    systemctl(&["stop", UNIT])
}

/// `~/.local/bin/capture-room`, on the `PATH` of most distros' shells.
fn path_link() -> PathBuf {
    home().join(".local/bin/capture-room")
}

/// Put `capture-room` on the `PATH`, unless something else has that name
/// there already.
fn link_on_path(exe: &Path) {
    let link = path_link();
    if link.symlink_metadata().is_ok() && !std::fs::read_link(&link).is_ok_and(|to| to == exe) {
        return;
    }
    let _ = std::fs::remove_file(&link);
    let made = std::fs::create_dir_all(link.parent().unwrap())
        .and_then(|()| std::os::unix::fs::symlink(exe, &link));
    if let Err(e) = made {
        eprintln!("Couldn't link {} to the app: {e}", link.display());
    }
}

/// Let the user's services run without a login session, so the node starts
/// at boot. Usually allowed without sudo; if not, the node still starts at
/// login, so say how and carry on.
fn enable_linger() {
    let uid = unsafe { libc::getuid() }.to_string();
    let lingering = Command::new("loginctl")
        .args(["show-user", &uid, "--property=Linger", "--value"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "yes");
    if lingering {
        return;
    }
    match run("loginctl", &["enable-linger"]) {
        Ok(()) => {
            let _ = std::fs::write(data_dir().join(LINGER_MARKER), "");
        }
        Err(e) => eprintln!(
            "Couldn't turn on lingering ({e:#}), so the node starts when you log in, not at \
             boot. To start it at boot:\n  sudo loginctl enable-linger {}",
            std::env::var("USER").unwrap_or(uid)
        ),
    }
}

fn systemctl(args: &[&str]) -> Result<()> {
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    run("systemctl", &all).map_err(|e| {
        if format!("{e}").contains("Failed to connect to") {
            e.context(
                "can't reach your systemd user session: run this as yourself in a login \
                 session (a desktop terminal or ssh), not through sudo or su",
            )
        } else {
            e
        }
    })
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// The unit's state: `active`, `activating`, `failed`, `inactive`, …
fn active_state() -> String {
    Command::new("systemctl")
        .args(["--user", "show", "--property=ActiveState", "--value", UNIT])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn wait_for_port(port: u16) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if matches!(active_state().as_str(), "failed" | "inactive") {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!(
        "the node didn't answer on port {port}; see {} (or journalctl --user -u {UNIT})",
        log_dir().join("capture-room.log").display()
    )
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_arguments_round_trip() {
        let args: Vec<String> = [
            "/home/a b/.local/share/capture-room/app/capture-room",
            "--db",
            "/home/a b/100% \"odd\" $HOME \\ name/capture-room.db",
            "--port",
            "7701",
            "--controller",
        ]
        .map(String::from)
        .to_vec();
        let env = [("CAPTURE_ROOM_SERVICE".to_string(), "1".to_string())];
        let unit = unit(&args, &env, Path::new("/tmp/log"));
        let exec = unit
            .lines()
            .find_map(|l| l.strip_prefix("ExecStart="))
            .unwrap();
        assert_eq!(unquote_words(exec), args);
        assert_eq!(arg_in_unit(&unit, "--port").as_deref(), Some("7701"));
        assert_eq!(
            arg_in_unit(&unit, "--db").as_deref(),
            Some(args[2].as_str())
        );
        assert!(unit.contains("Environment=\"CAPTURE_ROOM_SERVICE=1\"\n"));
        assert!(unit.contains("StandardOutput=append:/tmp/log\n"));
    }
}
