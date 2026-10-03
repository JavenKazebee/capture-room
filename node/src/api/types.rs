use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

// ── Node status ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct NodeStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub uptime_secs: u64,
    pub is_controller: bool,
}

// ── Sources ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
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
#[cfg_attr(feature = "export-types", ts(export))]
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
#[cfg_attr(feature = "export-types", ts(export))]
pub struct SourceDto {
    pub id: String,
    pub display_name: String,
    pub source_type: String,
    pub is_available: bool,
    pub connected: bool,
    pub timecode: Option<TimecodeDto>,
    pub capabilities: SourceCapabilitiesDto,
}

// ── Test source config ────────────────────────────────────────────────────────
//
// Stored as text in `test_sources`; the serde and sqlx names must match.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "kebab-case")]
#[sqlx(rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub enum VideoTestPattern {
    Smpte,
    Snow,
    Black,
    White,
    Ball,
    #[serde(rename = "smpte75")]
    #[sqlx(rename = "smpte75")]
    Smpte75,
    #[serde(rename = "checkers-1")]
    #[sqlx(rename = "checkers-1")]
    Checkers1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "kebab-case")]
#[sqlx(rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub enum AudioTestSignal {
    Tone,
    Silence,
    PinkNoise,
}

/// A configured test source, as stored and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct TestSourceConfigDto {
    pub id: String,
    pub name: String,
    pub pattern: VideoTestPattern,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub audio_signal: AudioTestSignal,
    pub frequency: f64,
    pub channels: u32,
    pub created_at: String,
}

/// Body for creating or replacing a test source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct TestSourceRequest {
    pub name: String,
    pub pattern: VideoTestPattern,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub audio_signal: AudioTestSignal,
    pub frequency: f64,
    pub channels: u32,
}

impl TestSourceRequest {
    pub fn into_config(self, id: String, created_at: String) -> TestSourceConfigDto {
        TestSourceConfigDto {
            id,
            name: self.name,
            pattern: self.pattern,
            width: self.width,
            height: self.height,
            fps_num: self.fps_num,
            fps_den: self.fps_den,
            audio_signal: self.audio_signal,
            frequency: self.frequency,
            channels: self.channels,
            created_at,
        }
    }
}

// ── Recordings ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub enum RecordingStatus {
    Active,
    Stopped,
    Error,
}

/// A recording session, as stored in `recording_sessions` and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct RecordingSessionDto {
    pub id: String,
    pub source_id: String,
    pub preset_id: String,
    pub started_at: String,
    pub stopped_at: Option<String>,
    /// Ordered list of output file paths, one per preset output leg.
    /// Stored as a JSON array.
    #[sqlx(json)]
    pub output_paths: Vec<String>,
    pub status: RecordingStatus,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct StartRecordingRequest {
    pub source_id: String,
    /// Informational: the controller-side preset these outputs came from.
    pub preset_id: Option<String>,
    /// The output legs to record. Sent inline so nodes keep no preset store.
    pub outputs: Vec<PresetOutputInput>,
}

// ── Presets ───────────────────────────────────────────────────────────────────

/// Chroma subsampling for H.264/H.265 outputs. Other codecs ignore it
/// (ProRes picks 422 vs 4444 through the codec itself).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub enum ChromaSubsampling {
    /// Plays everywhere.
    #[default]
    #[serde(rename = "420")]
    Yuv420,
    #[serde(rename = "422")]
    Yuv422,
    #[serde(rename = "444")]
    Yuv444,
}

impl ChromaSubsampling {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yuv420 => "420",
            Self::Yuv422 => "422",
            Self::Yuv444 => "444",
        }
    }

    /// Unknown values fall back to the default.
    pub fn from_db(s: &str) -> Self {
        match s {
            "422" => Self::Yuv422,
            "444" => Self::Yuv444,
            _ => Self::Yuv420,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct PresetOutputDto {
    pub id: String,
    pub preset_id: String,
    pub name: String,
    pub codec: String,
    pub container: String,
    pub resolution: Option<String>,
    pub framerate: Option<String>,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub bitrate_kbps: Option<i64>,
    pub chroma: ChromaSubsampling,
    pub path_template: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct PresetOutputInput {
    pub name: String,
    pub codec: String,
    pub container: String,
    pub resolution: Option<String>,
    pub framerate: Option<String>,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub bitrate_kbps: Option<i64>,
    pub chroma: ChromaSubsampling,
    pub path_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct PresetDto {
    pub id: String,
    pub name: String,
    pub outputs: Vec<PresetOutputDto>,
    pub created_at: String,
    pub updated_at: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct PresetCreateRequest {
    pub name: String,
    pub outputs: Vec<PresetOutputInput>,
}

// ── WebSocket events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct ChannelLevelDto {
    pub peak_db: f64,
    pub rms_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
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
    #[serde(rename = "node.online")]
    NodeOnline { peer_id: String },
    #[serde(rename = "node.offline")]
    NodeOffline { peer_id: String },
}

// ── Settings ──────────────────────────────────────────────────────────────────

/// How each source's monitor pipeline samples thumbnails and audio levels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct MonitorSettingsDto {
    pub thumb_fps: i32,
    pub thumb_width: i32,
    pub thumb_height: i32,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub level_interval_ms: u64,
}

impl Default for MonitorSettingsDto {
    fn default() -> Self {
        Self { thumb_fps: 1, thumb_width: 320, thumb_height: 180, level_interval_ms: 100 }
    }
}

impl MonitorSettingsDto {
    /// Limit every field to a range the pipelines handle well. Thumbnail
    /// notifications go out on the emitter's 100 ms tick, so 10 fps is the
    /// most a client can actually see.
    pub fn clamped(self) -> Self {
        Self {
            thumb_fps: self.thumb_fps.clamp(1, 10),
            thumb_width: self.thumb_width.clamp(160, 1920),
            thumb_height: self.thumb_height.clamp(90, 1080),
            level_interval_ms: self.level_interval_ms.clamp(50, 1000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct NodeSettingsDto {
    pub node_id: String,
    pub node_name: String,
    pub is_controller: bool,
    pub monitor: MonitorSettingsDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct UpdateNodeSettingsRequest {
    pub name: Option<String>,
    pub monitor: Option<MonitorSettingsDto>,
}

// ── Storage ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct StorageVolumeDto {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub total_bytes: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub available_bytes: u64,
    pub removable: bool,
}

// ── Controller ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct NodeDto {
    pub id: String,
    pub name: String,
    /// Empty for this instance.
    pub url: String,
    pub version: String,
    pub healthy: bool,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub uptime_secs: u64,
    pub is_self: bool,
    /// Whether this node was added by URL (persisted) rather than via mDNS.
    pub manual: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct AddNodeRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS))]
#[cfg_attr(feature = "export-types", ts(export))]
pub struct ControllerToggleRequest {
    pub enabled: bool,
}
