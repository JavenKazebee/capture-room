use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

// ── Node status ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct NodeStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub uptime_secs: u64,
    pub mode: String,
}

// ── Sources ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct TimecodeDto {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
    pub framerate: [u32; 2],
    pub display: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct SourceCapabilitiesDto {
    pub video_formats: Vec<String>,
    pub max_width: u32,
    pub max_height: u32,
    pub max_framerate: [u32; 2],
    pub audio_channels: u32,
    pub audio_sample_rates: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct SourceDto {
    pub id: String,
    pub display_name: String,
    pub source_type: String,
    pub is_available: bool,
    pub timecode: Option<TimecodeDto>,
    pub capabilities: SourceCapabilitiesDto,
}

// ── Recordings ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct RecordingSessionDto {
    pub id: String,
    pub source_id: String,
    pub preset_id: String,
    pub started_at: String,
    pub stopped_at: Option<String>,
    pub primary_path: String,
    pub secondary_path: Option<String>,
    pub redundant_path: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct StartRecordingRequest {
    pub source_id: String,
    pub preset_id: String,
    /// Optional explicit output path. If omitted, the preset's template is used.
    pub primary_path: Option<String>,
    pub secondary_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct PatchRecordingRequest {
    /// Only valid value currently: "stop"
    pub action: String,
}

// ── Presets ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct PresetCacheDto {
    pub id: String,
    pub name: String,
    pub data: serde_json::Value,
    pub version: i64,
    pub synced_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct PresetSyncRequest {
    pub presets: Vec<PresetCacheDto>,
}

// ── WebSocket events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub struct ChannelLevelDto {
    pub peak_db: f64,
    pub rms_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export, export_to = "../ui/src/types/generated/"))]
pub enum WsEvent {
    #[serde(rename = "source.available")]
    SourceAvailable {
        source_id: String,
        name: String,
    },
    #[serde(rename = "source.lost")]
    SourceLost {
        source_id: String,
        name: String,
    },
    #[serde(rename = "recording.started")]
    RecordingStarted {
        session_id: String,
        source_id: String,
    },
    #[serde(rename = "recording.stopped")]
    RecordingStopped {
        session_id: String,
        source_id: String,
    },
    #[serde(rename = "recording.error")]
    RecordingError {
        session_id: String,
        source_id: String,
        error: String,
    },
    #[serde(rename = "feed.status")]
    FeedStatus {
        source_id: String,
        timecode: Option<String>,
        duration_secs: f64,
    },
    #[serde(rename = "audio.levels")]
    AudioLevels {
        source_id: String,
        channels: Vec<ChannelLevelDto>,
    },
    #[serde(rename = "thumbnail.updated")]
    ThumbnailUpdated {
        source_id: String,
        url: String,
    },
    #[serde(rename = "log")]
    Log {
        level: String,
        message: String,
        timestamp: String,
    },
}
