//! Updating a node to the newest GitHub release.
//!
//! Each release has `latest.json` and, per platform, an app archive with a
//! minisign signature checked against the compiled-in key (so only our
//! releases install, whoever asks). The archive is unpacked beside the app,
//! version-checked, swapped in, and the node exits with
//! [`RESTART_EXIT_CODE`](crate::service::RESTART_EXIT_CODE) for the service
//! manager to start it. Only the installed service updates, never to an older
//! version or while recording or playing (a controller, nor while its nodes
//! are). The old app is kept until the new one starts.
//!
//! Windows won't replace a running exe or loaded DLL, nor rename a folder with
//! one open, but renames the files themselves: the update moves the app's
//! files aside one by one and the new ones in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use futures_util::StreamExt;
use tracing::{info, warn};

use crate::api::types::{UpdatePhase, UpdateStatusDto};
use crate::service;
use crate::state::AppState;

const REPO: &str = "JavenKazebee/capture-room";
const PUBLIC_KEY: &str = include_str!("../../scripts/release-key.pub");
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
#[cfg(target_os = "macos")]
const APP_NAME: &str = "Capture Room.app";
/// The folder in the Linux archive: the binary and its licenses.
#[cfg(target_os = "linux")]
const APP_NAME: &str = "capture-room";
/// The archive's folder, which becomes the app folder (`…\Capture Room\app`).
#[cfg(target_os = "windows")]
const APP_NAME: &str = "Capture Room";

pub struct Updater {
    status: Mutex<UpdateStatusDto>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Mutex::new(UpdateStatusDto {
                current: env!("CARGO_PKG_VERSION").to_string(),
                unsupported: unsupported(),
                ..Default::default()
            }),
        }
    }
}

impl Updater {
    pub fn status(&self) -> UpdateStatusDto {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, change: impl FnOnce(&mut UpdateStatusDto)) {
        change(&mut self.status.lock().unwrap());
    }

    /// Show why an update couldn't start, on the menu bar icon (the API
    /// answers with the error instead).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn fail(&self, error: String) {
        self.set(|s| s.error = Some(error));
    }

    fn busy(&self) -> bool {
        self.status.lock().unwrap().phase != UpdatePhase::Idle
    }
}

/// Why this process can't update itself, if it can't.
fn unsupported() -> Option<String> {
    if !cfg!(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )) {
        return Some("Updates aren't available on this OS yet.".into());
    }
    if service::app_path().is_none() || !service::running_as_service() {
        return Some("Only the installed app updates itself.".into());
    }
    None
}

fn asset() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    format!("capture-room-{os}-{}.tar.gz", std::env::consts::ARCH)
}

/// A release file's URL: from GitHub, or from the folder set in
/// `CAPTURE_ROOM_RELEASES` (flat, for testing).
fn url(version: Option<&str>, file: &str) -> String {
    if let Ok(base) = std::env::var(service::RELEASES_ENV) {
        return format!("{}/{file}", base.trim_end_matches('/'));
    }
    match version {
        Some(v) => format!("https://github.com/{REPO}/releases/download/v{v}/{file}"),
        None => format!("https://github.com/{REPO}/releases/latest/download/{file}"),
    }
}

/// What this node is doing that an update would interrupt, if anything
/// ("recording and a channel is playing").
pub async fn own_work(state: &AppState) -> Option<String> {
    let activity = crate::session::activity(state).await;
    let mut doing = Vec::new();
    if activity.recordings > 0 {
        doing.push("recording");
    }
    if activity.playing > 0 {
        doing.push("a channel is playing");
    }
    if crate::benchmark::running_id(state).is_some() {
        doing.push("a benchmark is running");
    }
    (!doing.is_empty()).then(|| doing.join(" and "))
}

/// Why the node can't update right now, if it can't: its own work, or on a
/// controller, any of its nodes' (as of their last health check), since the
/// controller on a new version can't reach them until they're updated too.
pub async fn busy_reason(state: &AppState) -> Option<String> {
    if let Some(doing) = own_work(state).await {
        return Some(format!("Not while {doing}."));
    }
    let controller = state.controller.read().await;
    let busy = controller.as_ref()?.registry.read().await.busy();
    (!busy.is_empty()).then(|| format!("Not while nodes are busy ({}).", busy.join("; ")))
}

/// The status with its `busy` reason filled in.
pub async fn status(state: &AppState) -> UpdateStatusDto {
    let mut status = state.updater.status();
    status.busy = busy_reason(state).await;
    status
}

/// Check now, then every few hours. Also clears what the last update left.
pub fn spawn_checker(state: Arc<AppState>) {
    tokio::spawn(async move {
        if let Some(app) = service::app_path() {
            let staging = service::update_staging(&app);
            let _ = tokio::fs::remove_dir_all(staging).await;
        }
        loop {
            check(&state).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[derive(serde::Deserialize)]
struct Latest {
    version: String,
}

pub async fn check(state: &AppState) {
    if state.updater.busy() {
        return;
    }
    state.updater.set(|s| s.phase = UpdatePhase::Checking);
    let result = async {
        let latest: Latest = state
            .http
            .get(url(None, "latest.json"))
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let newer = semver::Version::parse(&latest.version)?
            > semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
        anyhow::Ok((latest.version, newer))
    }
    .await;
    state.updater.set(|s| {
        s.phase = UpdatePhase::Idle;
        s.checked_at = Some(chrono::Utc::now().to_rfc3339());
        match result {
            Ok((latest, newer)) => {
                s.latest = Some(latest);
                s.available = newer;
                s.error = None;
            }
            Err(e) => {
                warn!(error = %e, "update check failed");
                s.error = Some(format!("Couldn't check for updates: {e}"));
            }
        }
    });
}

/// Start updating to `version`, or the newest release, in the background.
/// Fails at once if this node can't update now.
pub async fn start(state: &Arc<AppState>, version: Option<String>) -> Result<()> {
    let status = status(state).await;
    if let Some(why) = status.unsupported.or(status.busy) {
        bail!(why);
    }
    ensure!(
        status.phase == UpdatePhase::Idle,
        "An update or check is already running."
    );
    let version = match version {
        Some(v) => {
            ensure!(
                semver::Version::parse(&v)? > semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
                "This node runs {}, not older than {v}.",
                env!("CARGO_PKG_VERSION")
            );
            v
        }
        None => match status.latest.filter(|_| status.available) {
            Some(v) => v,
            None => bail!("This is the newest version."),
        },
    };
    state.updater.set(|s| {
        s.phase = UpdatePhase::Downloading;
        s.percent = Some(0);
        s.error = None;
    });
    let state = Arc::clone(state);
    tokio::spawn(async move {
        match install(&state, &version).await {
            Ok(()) => {
                info!(version, "update installed; restarting");
                state.updater.set(|s| s.phase = UpdatePhase::Restarting);
                // Let the UI see it before the node goes.
                tokio::time::sleep(Duration::from_millis(500)).await;
                crate::session::quit(&state, service::RESTART_EXIT_CODE).await;
            }
            Err(e) => {
                warn!(error = format!("{e:#}"), version, "update failed");
                // The download, unless the app didn't make it back into place.
                if let Some(app) = service::app_path().filter(|app| app.exists()) {
                    let _ = tokio::fs::remove_dir_all(service::update_staging(&app)).await;
                }
                state.updater.set(|s| {
                    s.phase = UpdatePhase::Idle;
                    s.percent = None;
                    s.error = Some(format!("Update to {version} failed: {e:#}"));
                });
            }
        }
    });
    Ok(())
}

async fn install(state: &AppState, version: &str) -> Result<()> {
    let app = service::app_path().context("not running from the installed app")?;
    let staging = service::update_staging(&app);
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await?;

    let asset = asset();
    let signature = fetch_text(state, &url(Some(version), &format!("{asset}.minisig"))).await?;
    let archive = staging.join(&asset);
    download(state, &url(Some(version), &asset), &archive).await?;

    state.updater.set(|s| {
        s.phase = UpdatePhase::Installing;
        s.percent = None;
    });
    let staging_ = staging.clone();
    let version_ = version.to_string();
    tokio::task::spawn_blocking(move || {
        verify(&archive, &signature)?;
        unpack_and_swap(&archive, &staging_, &app, &version_)
    })
    .await?
}

async fn fetch_text(state: &AppState, url: &str) -> Result<String> {
    Ok(state
        .http
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("download {url}"))?
        .text()
        .await?)
}

async fn download(state: &AppState, url: &str, to: &Path) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let response = state
        .http
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("download {url}"))?;
    let total = response.content_length();
    let mut file = tokio::fs::File::create(to).await?;
    let mut done = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            let percent = (done * 100 / total).min(100) as u8;
            state.updater.set(|s| s.percent = Some(percent));
        }
    }
    file.flush().await?;
    Ok(())
}

fn verify(file: &Path, signature: &str) -> Result<()> {
    let key = PUBLIC_KEY
        .lines()
        .find(|l| !l.starts_with("untrusted comment:") && !l.trim().is_empty())
        .context("no release key compiled in")?;
    let key = minisign_verify::PublicKey::from_base64(key.trim())?;
    let signature = minisign_verify::Signature::decode(signature)?;
    let data = std::fs::read(file)?;
    key.verify(&data, &signature, false)
        .context("the download isn't signed with the release key")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn untar(archive: &Path, into: &Path) -> Result<()> {
    let out = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()?;
    ensure!(
        out.status.success(),
        "unpack: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Move `app` to `old` and `new` into its place, putting it back if that
/// fails.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn swap(app: &Path, new: &Path, old: &Path) -> Result<()> {
    std::fs::rename(app, old).context("move the current app aside")?;
    if let Err(e) = std::fs::rename(new, app) {
        let _ = std::fs::rename(old, app);
        return Err(e).context("move the new app into place");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn unpack_and_swap(archive: &Path, staging: &Path, app: &Path, version: &str) -> Result<()> {
    untar(archive, staging)?;
    let new = staging.join(APP_NAME);
    let info = plist::Value::from_file(new.join("Contents/Info.plist"))?;
    let unpacked = info
        .as_dictionary()
        .and_then(|d| d.get("CFBundleShortVersionString"))
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    ensure!(
        unpacked == version,
        "the archive holds version {unpacked}, not {version}"
    );
    swap(app, &new, &staging.join("previous.app"))
}

/// The new binary is run for its version, which also shows it runs on this
/// system (the distro's GStreamer, say) before it replaces the old one.
#[cfg(target_os = "linux")]
fn unpack_and_swap(archive: &Path, staging: &Path, app: &Path, version: &str) -> Result<()> {
    untar(archive, staging)?;
    let new = staging.join(APP_NAME);
    let out = std::process::Command::new(new.join("capture-room"))
        .arg("--version")
        .output()
        .context("run the new version")?;
    ensure!(
        out.status.success(),
        "the new version doesn't run here: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    // clap prints "capture-room 0.2.0".
    let stdout = String::from_utf8_lossy(&out.stdout);
    let unpacked = stdout.split_whitespace().nth(1).unwrap_or_default();
    ensure!(
        unpacked == version,
        "the archive holds version {unpacked}, not {version}"
    );
    swap(app, &new, &staging.join("previous"))
}

#[cfg(target_os = "windows")]
fn unpack_and_swap(archive: &Path, staging: &Path, app: &Path, version: &str) -> Result<()> {
    // Windows' own tar (bsdtar): a Git for Windows tar earlier on PATH would
    // read `C:` as a remote host.
    let tar = std::env::var_os("SystemRoot")
        .map(|root| Path::new(&root).join(r"System32\tar.exe"))
        .unwrap_or_else(|| "tar".into());
    let out = std::process::Command::new(tar)
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(staging)
        .output()?;
    ensure!(
        out.status.success(),
        "unpack: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let new = staging.join(APP_NAME);
    let unpacked = std::fs::read_to_string(new.join("version.txt")).unwrap_or_default();
    let unpacked = unpacked.trim();
    ensure!(
        unpacked == version,
        "the archive holds version {unpacked}, not {version}"
    );
    let old = staging.join("previous");
    move_files(app, &old).context("move the current app aside")?;
    if let Err(e) = move_files(&new, app) {
        let _ = move_files(&old, app);
        return Err(e).context("move the new app into place");
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn unpack_and_swap(_: &Path, _: &Path, _: &Path, _: &str) -> Result<()> {
    bail!("updates aren't available on this OS yet")
}

/// Move every file under `from` to the same place under `to`, one rename
/// each (folders are made as needed, and left behind empty). All or
/// nothing: on an error the files moved so far go back.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn move_files(from: &Path, to: &Path) -> Result<()> {
    let mut files = Vec::new();
    files_in(from, &mut files).with_context(|| format!("list {}", from.display()))?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for file in files {
        let dest = to.join(file.strip_prefix(from)?);
        let result = dest
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(&file, &dest));
        if let Err(e) = result {
            for (back, moved_to) in moved.iter().rev() {
                let _ = std::fs::rename(moved_to, back);
            }
            return Err(e).with_context(|| format!("move {}", file.display()));
        }
        moved.push((file, dest));
    }
    Ok(())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn files_in(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files_in(&entry.path(), files)?;
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_files_moves_all_or_nothing() {
        let dir = std::env::temp_dir().join(format!("move-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (from, to) = (dir.join("from"), dir.join("to"));
        std::fs::create_dir_all(from.join("lib/plugins")).unwrap();
        std::fs::write(from.join("app.exe"), "app").unwrap();
        std::fs::write(from.join("lib/plugins/a.dll"), "a").unwrap();

        move_files(&from, &to).unwrap();
        let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();
        assert_eq!(read(to.join("app.exe")), "app");
        assert_eq!(read(to.join("lib/plugins/a.dll")), "a");
        assert!(!from.join("app.exe").exists());

        // `lib` is a file where the move needs a folder: nothing moves.
        let blocked = dir.join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("lib"), "").unwrap();
        assert!(move_files(&to, &blocked).is_err());
        assert!(to.join("app.exe").exists());
        assert!(to.join("lib/plugins/a.dll").exists());
        assert!(!blocked.join("app.exe").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
