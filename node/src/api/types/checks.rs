//! What a node needs from its machine (permissions, plugins, ports), as read
//! from the OS.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

/// What a node needs from its machine for each feature, and whether it has
/// it. Read live from the OS on every request; nothing is stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeChecksDto {
    /// `macos`, `linux`, `windows`, …
    pub os: String,
    /// macOS: the app privacy permissions belong to, the one that launched
    /// the node (Terminal when started from a shell).
    pub permission_holder: Option<String>,
    pub checks: Vec<CheckDto>,
    /// Ports others connect to on this node, for a controller to probe
    /// (a node can't see its own firewall).
    pub ports: Vec<NodePortDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CheckDto {
    /// Stable: `camera`, `ndi_runtime`, `port:tcp:8554`, …
    pub id: String,
    pub group: CheckGroup,
    pub label: String,
    pub status: CheckStatus,
    /// The state found, in a sentence.
    pub summary: String,
    /// What the check is for.
    pub needed_for: String,
    /// Sources on the node that need it.
    pub used_by: Vec<String>,
    /// Something on the node needs it now: missing, it's a problem rather
    /// than something to set up before using the feature.
    pub required: bool,
    pub fix: Option<CheckFix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum CheckGroup {
    Permissions,
    Software,
    Storage,
    Clock,
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum CheckStatus {
    Ok,
    Missing,
    /// macOS hasn't asked yet: it will the first time it's needed.
    NotAsked,
    /// Can't be told from here.
    Unknown,
}

/// How to put a check right. Shortcuts help; nothing grants for the user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CheckFix {
    /// What to do, in words.
    pub steps: String,
    /// A System Settings pane to open on that machine (macOS).
    pub settings_url: Option<String>,
    /// A command to run on that machine.
    pub command: Option<String>,
    /// A page to go to (e.g. a download).
    pub link: Option<CheckLink>,
    /// A page of this app to go to, where the fix is made.
    pub route: Option<CheckRoute>,
    /// The node can ask the OS to show its permission prompt
    /// (`POST /checks/request`).
    pub can_request: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CheckLink {
    pub url: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CheckRoute {
    /// An app path, with a `#hash` to scroll to (`/setup/settings#access-token`).
    pub path: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodePortDto {
    pub port: u16,
    pub protocol: PortProtocol,
    /// What listens on it: `API`, `RTSP outputs`, a source's name.
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum PortProtocol {
    Tcp,
    Udp,
}

/// Body for `POST /checks/request`: ask the OS to show a permission prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CheckRequest {
    /// A permission check's id: `camera`, `microphone` or `screen_recording`.
    pub id: String,
}
