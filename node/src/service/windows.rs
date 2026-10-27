//! The service on Windows: a supervisor process in the user's session, and a
//! Task Scheduler logon task that starts it.
//!
//! Everything is in `%LOCALAPPDATA%\Capture Room`: the app in `app` (the exe
//! beside GStreamer's DLLs in GStreamer's own layout, which
//! `plugins::use_bundled_gstreamer` points GStreamer at), the database and
//! plugin registry in `data`, the log in `logs`.
//!
//! `capture-room service run` is the supervisor. It starts the node with the
//! arguments in `service.json`, writes the node's output to the log file
//! (rotating it), starts it again when it stops with an error, and stays
//! stopped after a clean quit. After exit 75 (an update) it starts a fresh
//! supervisor from the app's files and exits, so nothing keeps the old files
//! open. A lock file the supervisor holds says the service is running.
//!
//! Start at login is the logon task, which runs the supervisor through
//! `capture-room-launcher.exe` so no console window opens. Starting the
//! service never goes through the task, so turning it off never touches the
//! running node. A task, not a Windows service: services can't show a tray
//! icon or reach the user's cameras.
//!
//! Stopping asks the node to quit through `windows_quit` (a process without
//! a console window has no Ctrl+C to send it), so it finishes its recordings
//! and exits cleanly, which ends the supervisor too. Only a node that hasn't
//! gone after [`STOP_TIMEOUT`] is killed, and its recordings end unfinalised.
//! Restarting is a stop and a start. The node finishes its recordings itself
//! when the user signs out or Windows shuts down.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};

use super::log_rotation;
use super::windows_flags::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW};
use super::{
    answers, installed_port, node_args, node_env, remove, update_staging, wait_for_port, Installed,
    DEFAULT_PORT, RESTART_EXIT_CODE,
};

/// The logon task's name, in the Task Scheduler Library's root.
const TASK: &str = "Capture Room";
/// The first start after an install scans every plugin.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// The supervisor writes everything, its own notes too, to the log file.
pub const MORE_LOGS: &str = "";
/// A node that fails sooner than this after starting waits [`RETRY_DELAY`]
/// before the next try, so a broken install doesn't spin.
const MIN_UPTIME: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(10);
/// How long a stop waits for the node to finish its recordings and quit
/// before killing it: well past `session::EOS_TIMEOUT`.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

const ERROR_SHARING_VIOLATION: i32 = 32;

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(r"AppData\Local")))
        .unwrap_or_else(std::env::temp_dir)
}

/// `%LOCALAPPDATA%\Capture Room`: the app in `app`, the database in `data`,
/// the log in `logs`.
fn root() -> PathBuf {
    local_app_data().join("Capture Room")
}

pub fn data_dir() -> PathBuf {
    root().join("data")
}

fn log_dir() -> PathBuf {
    root().join("logs")
}

pub fn log_file() -> Option<PathBuf> {
    Some(log_dir().join("capture-room.log"))
}

fn config_path() -> PathBuf {
    data_dir().join("service.json")
}

fn lock_path() -> PathBuf {
    data_dir().join("service.lock")
}

/// The supervisor's process id.
fn pid_path() -> PathBuf {
    data_dir().join("service.pid")
}

/// The node's process id, while the supervisor runs it.
fn node_pid_path() -> PathBuf {
    data_dir().join("node.pid")
}

/// The Start menu shortcut `install.ps1` makes.
fn shortcut_path() -> Option<PathBuf> {
    let programs = r"Microsoft\Windows\Start Menu\Programs\Capture Room.lnk";
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join(programs))
}

/// The app folder (`bin`, `lib`, `libexec`, `version.txt`), when this
/// binary is the one in its `bin`.
pub fn app_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin = exe.parent()?;
    let app = bin.parent()?;
    (bin.file_name()? == "bin" && app.join("version.txt").is_file()).then(|| app.to_path_buf())
}

/// What the supervisor runs.
#[derive(serde::Serialize, serde::Deserialize)]
struct Config {
    /// The node, `app\bin\capture-room.exe`.
    exe: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

fn read_config() -> Option<Config> {
    serde_json::from_slice(&std::fs::read(config_path()).ok()?).ok()
}

pub fn install(controller: bool, port: Option<u16>) -> Result<Installed> {
    let app = app_path().context("run this from the installed Capture Room (install.ps1)")?;
    let port = port.or_else(installed_port).unwrap_or(DEFAULT_PORT);
    // A fresh install starts at login; a reinstall keeps the choice.
    let at_login = !config_path().exists() || start_at_login();

    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(log_dir())?;
    let config = Config {
        exe: app.join(r"bin\capture-room.exe"),
        args: node_args(controller, port),
        env: node_env(),
    };

    stop()?;
    std::fs::write(config_path(), serde_json::to_vec_pretty(&config)?)?;
    set_start_at_login(at_login)?;
    start()?;
    wait_for_port(port)?;
    Ok(Installed {
        port,
        log: log_dir().join("capture-room.log"),
    })
}

pub fn uninstall(purge: bool) -> Result<()> {
    stop()?;
    set_start_at_login(false)?;
    for file in [config_path(), lock_path(), pid_path(), node_pid_path()] {
        remove(&file)?;
    }
    if let Some(shortcut) = shortcut_path() {
        remove(&shortcut)?;
    }
    let app = app_path().unwrap_or_else(|| root().join("app"));
    remove(&update_staging(&app))?;
    if purge {
        remove(&data_dir())?;
        remove(&log_dir())?;
    }
    let running_from_app = std::env::current_exe().is_ok_and(|exe| exe.starts_with(&app));
    if running_from_app {
        delete_after_exit(&app, purge.then(root))?;
    } else {
        remove(&app)?;
        if purge {
            // Only if empty.
            let _ = std::fs::remove_dir(root());
        }
    }
    Ok(())
}

/// This process runs from `app`, and Windows won't delete a running exe:
/// cmd deletes the folder once this process has exited (ping is cmd's way
/// to wait a moment), and then `root` if it's empty.
fn delete_after_exit(app: &Path, root: Option<PathBuf>) -> Result<()> {
    let mut script = format!(
        "/C ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
        app.display()
    );
    if let Some(root) = root {
        script.push_str(&format!(" & rmdir \"{}\"", root.display()));
    }
    Command::new("cmd")
        .raw_arg(script)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("schedule removing the app")?;
    Ok(())
}

/// The value after `flag` in the installed service's arguments.
pub fn installed_arg(flag: &str) -> Option<String> {
    let config = read_config()?;
    let at = config.args.iter().position(|a| a == flag)?;
    config.args.get(at + 1).cloned()
}

/// The supervisor keeps trying until the node quits cleanly.
pub fn starting() -> bool {
    is_running()
}

/// Whether a supervisor holds the lock file.
pub fn is_running() -> bool {
    match OpenOptions::new()
        .write(true)
        .share_mode(0)
        .open(lock_path())
    {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(ERROR_SHARING_VIOLATION),
    }
}

pub fn start_at_login() -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", TASK])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|out| out.status.success())
}

pub fn set_start_at_login(on: bool) -> Result<()> {
    if !on {
        if start_at_login() {
            run("schtasks", &["/Delete", "/TN", TASK, "/F"])?;
        }
        return Ok(());
    }
    let config = read_config().context("the service isn't installed")?;
    let launcher = config.exe.with_file_name("capture-room-launcher.exe");
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) => format!("{domain}\\{name}"),
        _ => bail!("USERDOMAIN and USERNAME aren't set"),
    };
    // schtasks reads UTF-16 task files.
    let xml = task_xml(&launcher, &user);
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let file = data_dir().join("logon-task.xml");
    std::fs::write(&file, bytes)?;
    let file_arg = file.display().to_string();
    let result = run(
        "schtasks",
        &["/Create", "/TN", TASK, "/XML", &file_arg, "/F"],
    );
    let _ = std::fs::remove_file(&file);
    result
}

/// A task that runs `launcher service run` when `user` signs in, as them,
/// unelevated, with no time limit, on battery too, at normal priority
/// (a task's default is below normal, which its processes would inherit).
fn task_xml(launcher: &Path, user: &str) -> String {
    let user = xml_escape(user);
    let launcher = xml_escape(&launcher.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts Capture Room when you sign in.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{launcher}</Command>
      <Arguments>service run</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn start() -> Result<()> {
    if is_running() {
        return Ok(());
    }
    let config = read_config().context("the service isn't installed")?;
    spawn_supervisor(&config.exe)?;
    // Until it holds the lock, so callers see it running.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !is_running() {
        ensure!(Instant::now() < deadline, "the service didn't start");
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// Start `exe service run` with no console window, on its own.
fn spawn_supervisor(exe: &Path) -> Result<()> {
    let mut command = Command::new(exe);
    command
        .args(["service", "run"])
        .current_dir(data_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Out of the caller's job where the job allows it: a terminal or SSH
    // session may end its job's processes when it closes.
    if command
        .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    command
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("start the service")?;
    Ok(())
}

/// Restart the running node (finishing anything it's doing): stop the
/// service and start it again.
pub fn restart() -> Result<()> {
    ensure!(is_running(), "the service isn't running");
    let port = installed_port().unwrap_or(DEFAULT_PORT);
    stop()?;
    // Until the old node has let go of the port.
    let deadline = Instant::now() + Duration::from_secs(10);
    while answers(port) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    start()?;
    wait_for_port(port)
}

/// Ask the node to quit, which ends the supervisor too; kill them both if
/// they're still there after [`STOP_TIMEOUT`].
fn stop() -> Result<()> {
    if !is_running() {
        return Ok(());
    }
    let pid = std::fs::read_to_string(pid_path()).context("the service's pid file is missing")?;
    // No node, or one not listening yet: it's between starts or still
    // starting, with nothing to finish.
    let asked = std::fs::read_to_string(node_pid_path())
        .ok()
        .and_then(|node| node.trim().parse().ok())
        .is_some_and(|node| super::windows_quit::request(node).is_ok());
    if asked {
        let deadline = Instant::now() + STOP_TIMEOUT;
        while is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        if !is_running() {
            return Ok(());
        }
    }
    // The supervisor and the node under it. It may have gone just now.
    if let Err(e) = run("taskkill", &["/F", "/T", "/PID", pid.trim()]) {
        if is_running() {
            return Err(e);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while is_running() {
        ensure!(Instant::now() < deadline, "the service didn't stop");
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
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

/// `capture-room service run`: the supervisor (see the module docs).
pub fn run_supervisor() -> Result<()> {
    std::fs::create_dir_all(data_dir())?;
    std::fs::create_dir_all(log_dir())?;
    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(lock_path())
    {
        Ok(lock) => lock,
        // Another supervisor runs the node already.
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => return Ok(()),
        Err(e) => return Err(e).context("take the service's lock"),
    };
    std::fs::write(pid_path(), std::process::id().to_string())?;
    let log = Arc::new(Mutex::new(Log::open(log_dir().join("capture-room.log"))?));
    let note = |message: &str| {
        let time = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let line = format!("{time}  INFO capture_room::service: {message}\n");
        if let Ok(mut log) = log.lock() {
            log.write(line.as_bytes());
        }
    };

    loop {
        let config = read_config().context("the service isn't installed")?;
        let started = Instant::now();
        let spawned = Command::new(&config.exe)
            .args(&config.args)
            .envs(config.env.iter().map(|(k, v)| (k, v)))
            .current_dir(data_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut node = match spawned {
            Ok(node) => node,
            Err(e) => {
                note(&format!("couldn't start {}: {e}", config.exe.display()));
                std::thread::sleep(RETRY_DELAY);
                continue;
            }
        };
        let _ = std::fs::write(node_pid_path(), node.id().to_string());
        if let Some(out) = node.stdout.take() {
            copy_to_log(out, Arc::clone(&log));
        }
        if let Some(err) = node.stderr.take() {
            copy_to_log(err, Arc::clone(&log));
        }
        let status = node.wait()?;
        let _ = std::fs::remove_file(node_pid_path());
        match status.code() {
            Some(0) => {
                note("the node quit");
                return Ok(());
            }
            Some(RESTART_EXIT_CODE) => {
                // From the app's files, which an update has just replaced:
                // this process runs from the old ones.
                note("restarting the node");
                drop(lock);
                return spawn_supervisor(&config.exe);
            }
            _ => {
                note(&format!("the node stopped ({status}); starting it again"));
                if started.elapsed() < MIN_UPTIME {
                    std::thread::sleep(RETRY_DELAY);
                }
            }
        }
    }
}

/// Copy the node's output into the log, a line at a time.
fn copy_to_log(from: impl Read + Send + 'static, log: Arc<Mutex<Log>>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            if let Ok(mut log) = log.lock() {
                log.write(&line);
            }
            line.clear();
        }
    });
}

/// The log file, rotated past [`log_rotation::MAX_BYTES`].
struct Log {
    path: PathBuf,
    file: File,
    size: u64,
}

impl Log {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self { path, file, size })
    }

    fn write(&mut self, line: &[u8]) {
        if self.size >= log_rotation::MAX_BYTES {
            // On failure (the file held open elsewhere), keep writing to it.
            if let Ok(file) = log_rotation::rotate(&self.path, log_rotation::KEEP) {
                self.file = file;
                self.size = 0;
            }
        }
        if self.file.write_all(line).is_ok() {
            self.size += line.len() as u64;
        }
    }
}
