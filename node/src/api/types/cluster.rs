//! A controller's view of its nodes: approved, found and unreachable.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeDto {
    /// Empty for this instance.
    pub url: String,
    pub healthy: bool,
    pub is_self: bool,
    /// SHA-256 of its TLS key, as pinned. `None` for a node approved before
    /// TLS, until its key is pinned.
    pub fingerprint: Option<String>,
    /// Why its last health check failed; `None` while healthy, and until
    /// its first check.
    pub unreachable: Option<Unreachable>,
    /// What the node last reported; see [`NodeStatus::unanswered`] for one
    /// that hasn't answered yet.
    #[serde(flatten)]
    pub status: NodeStatus,
}

/// Why an approved node failed its health check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum Unreachable {
    /// Nothing answers at its address.
    Offline,
    /// It refuses this controller's access token: it has a different one.
    AccessDenied,
    /// It holds a key other than the one pinned: reinstalled, or its data
    /// reset. `fingerprint` is the key it holds now.
    KeyChanged { fingerprint: String },
    /// Another node answers at its address (its IP went to another machine).
    OtherNode { name: String },
    /// It answers with its pinned key, but the check failed.
    Failing { error: String },
    /// It runs another version than the controller. Only its update is
    /// forwarded until the two match.
    OtherVersion { version: String },
}

/// Which release a node updates to (`/update`). `None` is the newest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct UpdateRequest {
    #[serde(default)]
    pub version: Option<String>,
}

/// A node that announced itself over mDNS, waiting to be approved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct FoundNodeDto {
    pub id: String,
    pub name: String,
    pub url: String,
    pub version: String,
    /// SHA-256 of the TLS key it presented: compare with the one its own
    /// Setup shows before approving it.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AddNodeRequest {
    pub url: String,
    /// The pairing code the node shows; not needed if it already uses this
    /// controller's token.
    #[serde(default)]
    pub pairing_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ControllerToggleRequest {
    pub enabled: bool,
}
