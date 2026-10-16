//! Updating a node to the newest GitHub release.
//!
//! Each release carries `latest.json` (`{"version": "0.2.0"}`) and, per
//! platform, an archive of the app with a minisign signature. A node checks
//! `latest.json` every few hours; updating downloads its platform's archive,
//! checks the signature against the release key compiled in (so only our
//! releases install, whoever asks), unpacks it beside the installed app,
//! checks it's the version expected, swaps it in and exits with
//! [`RESTART_EXIT_CODE`](crate::service::RESTART_EXIT_CODE); the service
//! manager starts the new version. Only the installed app running as a
//! service updates, never to an older version, and never while it records
//! or a channel plays. The previous app is kept until the new one starts.
//! See ARCHITECTURE.md (Deployment).

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
/// The minisign public key releases are signed with.
const PUBLIC_KEY: &str = include_str!("../../scripts/release-key.pub");
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Where an update is unpacked, beside the app (one volume, so the swap is
/// two renames).
const STAGING: &str = ".capture-room-update";
#[cfg(target_os = "macos")]
const APP_NAME: &str = "Capture Room.app";

/// The update state a node serves and its menu bar icon shows.
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

    /// Show why an update couldn't start.
    pub fn fail(&self, error: String) {
        self.set(|s| s.error = Some(error));
    }

    fn busy(&self) -> bool {
        self.status.lock().unwrap().phase != UpdatePhase::Idle
    }
}

/// Why this process can't update itself, if it can't.
fn unsupported() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return Some("Updates aren't available on this OS yet.".into());
    }
    if service::app_path().is_none() || !service::running_as_service() {
        return Some("Only the installed app updates itself (install.sh).".into());
    }
    None
}

/// The archive for this platform.
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

/// Why the node can't update right now, if it can't.
pub async fn busy_reason(state: &AppState) -> Option<String> {
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
    (!doing.is_empty()).then(|| format!("Not while {}.", doing.join(" and ")))
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
            let staging = app.with_file_name(STAGING);
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

/// Look for a newer release.
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

/// Start updating to the newest release, in the background. Fails at once
/// if this node can't update now.
pub async fn start(state: &Arc<AppState>) -> Result<()> {
    let status = status(state).await;
    if let Some(why) = status.unsupported.or(status.busy) {
        bail!(why);
    }
    ensure!(
        status.phase == UpdatePhase::Idle,
        "An update or check is already running."
    );
    let Some(version) = status.latest.filter(|_| status.available) else {
        bail!("This is the newest version.");
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
                    let _ = tokio::fs::remove_dir_all(app.with_file_name(STAGING)).await;
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
    let staging = app.with_file_name(STAGING);
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

/// Check `file` was signed with the release key.
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

#[cfg(target_os = "macos")]
fn unpack_and_swap(archive: &Path, staging: &Path, app: &Path, version: &str) -> Result<()> {
    let out = std::process::Command::new("tar")
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
    let old: PathBuf = staging.join("previous.app");
    std::fs::rename(app, &old).context("move the current app aside")?;
    if let Err(e) = std::fs::rename(&new, app) {
        let _ = std::fs::rename(&old, app);
        return Err(e).context("move the new app into place");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn unpack_and_swap(_: &Path, _: &Path, _: &Path, _: &str) -> Result<()> {
    bail!("updates aren't available on this OS yet")
}
