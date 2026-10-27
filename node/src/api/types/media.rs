//! The media library, and browsing the node's directories for a file.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum MediaOrigin {
    /// Added from the node's file browser.
    Import,
    /// Added from a recording session's files.
    Recording,
    /// Exported from a saved replay clip.
    Replay,
}

/// A file in the node's media library.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct MediaItemDto {
    pub id: String,
    pub path: String,
    pub name: String,
    #[sqlx(json)]
    pub info: MediaInfo,
    pub origin: MediaOrigin,
    pub session_id: Option<String>,
    pub added_at: String,
    /// The file is no longer on disk.
    #[sqlx(skip)]
    #[serde(default)]
    pub missing: bool,
}

/// Body for adding a file to the media library.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AddMediaRequest {
    pub path: String,
    /// Defaults to the file name.
    #[serde(default)]
    pub name: Option<String>,
    /// The recording session the file is from.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// A directory on the node, for picking a media file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DirListingDto {
    pub path: String,
    /// `None` at the filesystem root.
    pub parent: Option<String>,
    /// The node user's home directory.
    pub home: Option<String>,
    /// Subdirectories, then media files, each sorted by name. Hidden entries
    /// and other files are left out.
    pub entries: Vec<DirEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// File size; `None` for directories.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub size: Option<u64>,
}
