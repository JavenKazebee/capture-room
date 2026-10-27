//! Recording sessions and the output legs a start carries.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum RecordingStatus {
    Active,
    Stopped,
    Error,
}

/// A recording session, as stored in `recording_sessions` and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct RecordingSessionDto {
    pub id: String,
    pub source_id: String,
    pub preset_id: String,
    /// The source's name when recording started. `None` for older sessions.
    pub source_name: Option<String>,
    /// The preset's name, as sent with the start. `None` without a preset
    /// (or for older sessions).
    pub preset_name: Option<String>,
    pub started_at: String,
    pub stopped_at: Option<String>,
    /// What each output leg recorded, ordered like `output_paths`. Empty for
    /// older sessions.
    #[sqlx(json)]
    pub outputs: Vec<SessionOutputDto>,
    /// The planned path of each output leg.
    #[sqlx(json)]
    pub output_paths: Vec<String>,
    /// Video frames each output leg dropped because it couldn't keep up,
    /// ordered like `output_paths`. Live while recording, final once stopped.
    #[sqlx(json)]
    #[cfg_attr(feature = "export-types", ts(type = "Array<number>"))]
    pub dropped_frames: Vec<u64>,
    /// Every file each leg has written, ordered like `output_paths` (several
    /// if it splits). Saved as each file opens, so it survives a crash.
    #[sqlx(json)]
    pub files: Vec<Vec<String>>,
    pub status: RecordingStatus,
    pub error_message: Option<String>,
    /// The clock the recording ran on, for lining it up with others. `None`
    /// for older sessions, or a source still on a previous clock.
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub clock: Option<SessionClockDto>,
}

/// Where a recording sits on its clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SessionClockDto {
    /// See [`ClockStatusDto::domain`]: recordings with the same domain can be
    /// lined up by `first_frame_us`.
    pub domain: String,
    /// The synchronized start it waited for, if any.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub start_at_us: Option<u64>,
    /// When its first video frame was due on the clock. `None` until one
    /// arrives.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub first_frame_us: Option<u64>,
    /// UTC minus the clock's time when it started, in µs (see
    /// [`ClockStatusDto::utc_offset_us`]). MOV files' timecode is the time
    /// of day this gives each frame.
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub utc_offset_us: Option<i64>,
    /// When the first video frame was due, in UTC (RFC 3339).
    #[serde(default)]
    pub first_frame_utc: Option<String>,
}

/// One output leg of a session: its name and format, as recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SessionOutputDto {
    pub name: String,
    pub codec: VideoCodec,
    pub container: Container,
    pub audio: SessionAudio,
    /// Whether browsers can play its files: H.264 4:2:0 with AAC audio in
    /// MP4 or MOV.
    pub playable: bool,
    /// The preset marked this output as the one to preview with.
    pub preview: bool,
}

/// The audio codec a leg wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum SessionAudio {
    Pcm,
    Aac,
    Opus,
}

/// `GET /recordings` paging.
#[derive(Debug, Default, Deserialize)]
pub struct RecordingsQuery {
    /// Only sessions that started before this (RFC 3339): the `started_at`
    /// of the last session on the previous page. Active sessions are listed
    /// only on the first page (without it).
    pub before: Option<String>,
    /// Page size; default 100, at most 500.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct StartRecordingRequest {
    pub source_id: String,
    /// Informational: the controller-side preset these outputs came from.
    pub preset_id: Option<String>,
    /// The preset's name, for `{preset}` in path templates.
    pub preset_name: Option<String>,
    /// The output legs to record. Sent inline so nodes keep no preset store.
    pub outputs: Vec<PresetOutputInput>,
    /// Start at this time on the shared clock so recordings on several nodes
    /// line up: each leg begins at the first frame due at or after it. A node
    /// on another clock starts now (its `clock.start_at_us` is then `null`).
    #[serde(default)]
    pub start_at: Option<ClockTimeDto>,
}

/// Video codec of an output leg. Stored as text in `preset_outputs`; the serde
/// and sqlx names must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    #[serde(rename = "prores_4444")]
    #[sqlx(rename = "prores_4444")]
    ProRes4444,
    #[serde(rename = "prores_422hq")]
    #[sqlx(rename = "prores_422hq")]
    ProRes422Hq,
    #[serde(rename = "prores_422")]
    #[sqlx(rename = "prores_422")]
    ProRes422,
    #[serde(rename = "prores_422lt")]
    #[sqlx(rename = "prores_422lt")]
    ProRes422Lt,
    #[serde(rename = "prores_422proxy")]
    #[sqlx(rename = "prores_422proxy")]
    ProRes422Proxy,
    Uncompressed,
}

/// Container format of an output leg. Stored as text in `preset_outputs`.
/// Not every codec fits every container: see `RecordingProfile::from_output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum Container {
    Mov,
    Mp4,
    Mkv,
}

/// Chroma subsampling for H.264/H.265 outputs. Other codecs ignore it
/// (ProRes picks 422 vs 4444 through the codec itself).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ChromaSubsampling {
    /// Plays everywhere.
    #[default]
    #[serde(rename = "420")]
    #[sqlx(rename = "420")]
    Yuv420,
    #[serde(rename = "422")]
    #[sqlx(rename = "422")]
    Yuv422,
    #[serde(rename = "444")]
    #[sqlx(rename = "444")]
    Yuv444,
}

/// One output leg: what to encode and where to write it.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PresetOutputInput {
    pub name: String,
    pub codec: VideoCodec,
    pub container: Container,
    pub resolution: Option<String>,
    pub framerate: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub chroma: ChromaSubsampling,
    pub path_template: String,
    /// Preview recordings with this output when several are
    /// browser-playable. Left out when false, so it doesn't change the
    /// capacity key (`capacity::outputs_key`) of stored benchmark results.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "export-types", ts(as = "Option<bool>", optional))]
    pub preview: bool,
    /// Fine-tuning; every field defaults to the recording defaults.
    #[serde(default)]
    #[sqlx(json)]
    pub advanced: OutputAdvanced,
}

/// An output's Advanced settings. Each defaults to what recording does
/// without it (`None` / the first variant), so `{}` is a plain output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct OutputAdvanced {
    pub encoder: EncoderChoice,
    pub rate_control: RateControl,
    /// Constant-quality level, 1–100, higher is better. `None` = 70.
    pub quality: Option<u8>,
    /// x264/x265 speed preset. `None` = veryfast.
    pub speed_preset: Option<SpeedPreset>,
    /// Seconds between keyframes. `None` = 2.
    pub keyframe_secs: Option<f64>,
    pub deinterlace: Deinterlace,
    pub audio_codec: AudioCodecChoice,
    /// AAC/Opus bitrate. `None` = 256 kbps for AAC, 160 for Opus.
    pub audio_bitrate_kbps: Option<u32>,
    pub audio_channels: AudioChannels,
    /// 1-based source channels for [`AudioChannels::Pick`], in output order.
    pub channel_pick: Vec<u32>,
    /// Start a new file every this many minutes. With `split_gb`, whichever
    /// comes first. Both `None` = one file.
    pub split_minutes: Option<u32>,
    /// Start a new file at about this many GB.
    pub split_gb: Option<f64>,
}

/// Which kind of encoder an output may use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum EncoderChoice {
    /// Hardware where the node has it, else software.
    #[default]
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum RateControl {
    /// Average bitrate: more bits for complex scenes, fewer for simple ones.
    #[default]
    Average,
    /// Constant bitrate.
    Constant,
    /// Constant quality: the size follows the content.
    Quality,
}

/// x264/x265 speed presets, fastest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum SpeedPreset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum Deinterlace {
    /// Deinterlace interlaced sources (not ProRes); progressive passes through.
    #[default]
    Auto,
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum AudioCodecChoice {
    /// PCM beside ProRes/uncompressed, else AAC (.mov/.mp4) or Opus (.mkv).
    #[default]
    Auto,
    Pcm,
    Aac,
    Opus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum AudioChannels {
    #[default]
    All,
    /// Mixed down to two channels.
    Stereo,
    /// The channels in `channel_pick`.
    Pick,
}

/// A preset, as stored in `presets` (plus its outputs) and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PresetDto {
    pub id: String,
    pub name: String,
    #[sqlx(skip)]
    pub outputs: Vec<PresetOutputInput>,
    pub created_at: String,
    pub updated_at: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PresetCreateRequest {
    pub name: String,
    pub outputs: Vec<PresetOutputInput>,
}
