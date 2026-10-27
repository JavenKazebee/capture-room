//! A node's own status, software update and settings.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub uptime_secs: u64,
    pub is_controller: bool,
    /// Video encoder elements this node has (of those recording can use), so
    /// the UI can say what an output's Auto encoder resolves to here.
    #[serde(default)]
    pub encoders: Vec<String>,
    /// Source types that depend on optional plugins, and what this node
    /// is missing for each.
    #[serde(default)]
    pub source_types: Vec<SourceTypeSupport>,
    /// What the node's pipelines run on. `None` from nodes that predate
    /// clock sync.
    #[serde(default)]
    pub clock: Option<ClockStatusDto>,
    /// The codecs it can keep replay buffers in, the default first.
    #[serde(default)]
    pub replay_codecs: Vec<ReplayCodec>,
    /// What it's doing that an update would interrupt, if anything: its
    /// controller doesn't update itself meanwhile.
    #[serde(default)]
    pub busy: Option<String>,
}

impl NodeStatus {
    /// A node that hasn't answered yet: only what the controller stored.
    pub fn unanswered(id: String, name: String) -> Self {
        Self {
            id,
            name,
            version: String::new(),
            uptime_secs: 0,
            is_controller: false,
            encoders: Vec::new(),
            source_types: Vec::new(),
            clock: None,
            replay_codecs: Vec::new(),
            busy: None,
        }
    }
}

/// Where a node's software update stands (`/update`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct UpdateStatusDto {
    pub current: String,
    /// The newest release, once checked.
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// Why this node can never update itself (not the installed app).
    pub unsupported: Option<String>,
    /// Why it can't update right now (recording, a channel playing), if so.
    pub busy: Option<String>,
    pub phase: UpdatePhase,
    /// How much of the download is done, while downloading.
    pub percent: Option<u8>,
    /// Why the last check or update failed.
    pub error: Option<String>,
    pub checked_at: Option<String>,
    /// Which releases `latest` is from.
    #[serde(default)]
    pub channel: UpdateChannel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Installing,
    /// Installed; the node is restarting into the new version.
    Restarting,
}

/// How each source's monitor pipeline samples thumbnails and audio levels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct MonitorSettingsDto {
    pub thumb_fps: i32,
    pub thumb_width: i32,
    pub thumb_height: i32,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub level_interval_ms: u64,
}

impl Default for MonitorSettingsDto {
    fn default() -> Self {
        Self {
            thumb_fps: 1,
            thumb_width: 320,
            thumb_height: 180,
            level_interval_ms: 100,
        }
    }
}

impl MonitorSettingsDto {
    /// Limit every field to a range the pipelines handle well.
    pub fn clamped(self) -> Self {
        Self {
            thumb_fps: self.thumb_fps.clamp(1, crate::thumbnails::MAX_FPS as i32),
            thumb_width: self.thumb_width.clamp(160, 1920),
            thumb_height: self.thumb_height.clamp(90, 1080),
            level_interval_ms: self.level_interval_ms.clamp(50, 1000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeSettingsDto {
    pub node_id: String,
    pub node_name: String,
    pub is_controller: bool,
    pub monitor: MonitorSettingsDto,
    pub update_channel: UpdateChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct UpdateNodeSettingsRequest {
    pub name: Option<String>,
    pub monitor: Option<MonitorSettingsDto>,
    #[serde(default)]
    pub update_channel: Option<UpdateChannel>,
}

/// Which releases the updater offers. Pre-releases (`0.2.0-beta.1`) are
/// GitHub pre-releases, which only this node's choice brings in; a node on
/// them still moves on to the stable release that follows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum UpdateChannel {
    #[default]
    Stable,
    Prerelease,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct StorageVolumeDto {
    pub name: String,
    pub mount_point: String,
    /// More mount points of the same filesystem (e.g. btrfs subvolumes, bind
    /// mounts), which share its space.
    pub other_mounts: Vec<String>,
    pub file_system: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub total_bytes: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub available_bytes: u64,
    pub removable: bool,
    /// What this node's active recordings write to the volume, measured from
    /// their files. 0 when nothing records here.
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub write_bytes_per_sec: u64,
    /// Recording time left at that rate; `None` when nothing records here.
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub seconds_left: Option<u64>,
}

/// What a folder in [`FolderListingDto`] is, for its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum FolderKind {
    Folder,
    Home,
    /// The node's own folder for recordings, replay buffers and clips.
    Storage,
    Volume,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct FolderDto {
    pub name: String,
    pub path: String,
    pub kind: FolderKind,
}

/// A folder's subfolders on the node, for picking where recordings go.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct FolderListingDto {
    /// `None` for the roots: home, the storage folder and the volumes.
    pub path: Option<String>,
    /// `None` at a filesystem root and for the roots.
    pub parent: Option<String>,
    /// Sorted by name; hidden folders are left out.
    pub folders: Vec<FolderDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CreateFolderRequest {
    /// The folder to create it in.
    pub parent: String,
    pub name: String,
}
