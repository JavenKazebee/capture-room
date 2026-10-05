use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

// ── Node status ───────────────────────────────────────────────────────────────

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
}

// ── Sources ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum SourceType {
    Test,
    Ndi,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceCapabilitiesDto {
    pub max_width: u32,
    pub max_height: u32,
    pub max_framerate: [u32; 2],
    pub audio_channels: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceDto {
    pub id: String,
    pub display_name: String,
    pub source_type: SourceType,
    pub connected: bool,
    /// Set when the monitor pipeline has failed (e.g. an NDI sender went away).
    pub error: Option<String>,
    /// `HH:MM:SS:FF`, or `None` if the source has no timecode.
    pub timecode: Option<String>,
    /// `None` when the source can't know its format up front (NDI negotiates
    /// it at runtime).
    pub capabilities: Option<SourceCapabilitiesDto>,
}

// ── Configured sources ────────────────────────────────────────────────────────
//
// Sources a node is told about, as opposed to ones it discovers (NDI). Stored
// in `configured_sources`, the config as JSON tagged with its `type`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum VideoTestPattern {
    Smpte,
    Snow,
    Black,
    White,
    Ball,
    #[serde(rename = "smpte75")]
    Smpte75,
    #[serde(rename = "checkers-1")]
    Checkers1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum AudioTestSignal {
    Tone,
    Silence,
    PinkNoise,
}

/// What a configured source is, and its type's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum SourceConfig {
    Test(TestSourceConfig),
    File(FileSourceConfig),
}

/// A synthetic feed: a video pattern plus a test audio signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct TestSourceConfig {
    pub pattern: VideoTestPattern,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub audio_signal: AudioTestSignal,
    pub frequency: f64,
    pub channels: u32,
}

/// A media file on the node, played in a loop as a live feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct FileSourceConfig {
    /// Absolute path on the node.
    pub path: String,
    /// What the node found in the file when the source was saved. Filled in
    /// by the node; ignored in requests.
    #[serde(default)]
    pub media: Option<MediaInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// 0 when the file has no audio (the source then plays silence).
    pub audio_channels: u32,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub duration_ms: Option<u64>,
}

/// A configured source, as stored and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ConfiguredSourceDto {
    pub id: String,
    pub name: String,
    #[sqlx(json)]
    pub config: SourceConfig,
    pub created_at: String,
}

/// Body for creating or replacing a configured source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ConfiguredSourceRequest {
    pub name: String,
    pub config: SourceConfig,
}

// ── File browsing ─────────────────────────────────────────────────────────────

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

// ── Recordings ────────────────────────────────────────────────────────────────

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
    /// The source's name when recording started. `None` for sessions
    /// recorded before names were kept.
    pub source_name: Option<String>,
    /// The preset's name, as sent with the start. `None` without a preset
    /// (or for older sessions).
    pub preset_name: Option<String>,
    pub started_at: String,
    pub stopped_at: Option<String>,
    /// What each output leg recorded, ordered like `output_paths`. Empty for
    /// sessions recorded before this was kept.
    #[sqlx(json)]
    pub outputs: Vec<SessionOutputDto>,
    /// Ordered list of output file paths, one per preset output leg.
    /// Stored as a JSON array.
    #[sqlx(json)]
    pub output_paths: Vec<String>,
    /// Video frames each output leg dropped because it couldn't keep up,
    /// ordered like `output_paths`. Live while recording, final once stopped.
    #[sqlx(json)]
    #[cfg_attr(feature = "export-types", ts(type = "Array<number>"))]
    pub dropped_frames: Vec<u64>,
    /// Every file each output leg has written, in order, ordered like
    /// `output_paths`: one per leg unless it splits. Saved as each file
    /// opens, so it survives a crash.
    #[sqlx(json)]
    pub files: Vec<Vec<String>>,
    pub status: RecordingStatus,
    pub error_message: Option<String>,
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
}

// ── Presets ───────────────────────────────────────────────────────────────────

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
    /// Stored as JSON.
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

/// A stored output leg, as kept in `preset_outputs` and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PresetOutputDto {
    pub id: String,
    pub preset_id: String,
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub output: PresetOutputInput,
    pub sort_order: u32,
}

/// A preset, as stored in `presets` (plus its outputs) and as served.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PresetDto {
    pub id: String,
    pub name: String,
    #[sqlx(skip)]
    pub outputs: Vec<PresetOutputDto>,
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

// ── WebSocket events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ChannelLevelDto {
    pub peak_db: f64,
    pub rms_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum WsEvent {
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
    /// A leg failed while the session keeps recording on its other legs.
    /// `error` is the session's accumulated error message.
    #[serde(rename = "recording.leg_failed")]
    RecordingLegFailed {
        session_id: String,
        source_id: String,
        error: String,
    },
    /// Live dropped-frame counts and files of an active session, once a second.
    #[serde(rename = "recording.stats")]
    RecordingStats {
        session_id: String,
        source_id: String,
        #[cfg_attr(feature = "export-types", ts(type = "Array<number>"))]
        dropped_frames: Vec<u64>,
        files: Vec<Vec<String>>,
    },
    /// A finished session was removed from history, with or without its files.
    #[serde(rename = "recording.removed")]
    RecordingRemoved { session_id: String },
    /// A finished session changed after it stopped: some of its files were deleted.
    #[serde(rename = "recording.updated")]
    RecordingUpdated { session: Box<RecordingSessionDto> },
    #[serde(rename = "feed.status")]
    FeedStatus {
        source_id: String,
        timecode: Option<String>,
        error: Option<String>,
    },
    #[serde(rename = "audio.levels")]
    AudioLevels {
        source_id: String,
        channels: Vec<ChannelLevelDto>,
    },
    #[serde(rename = "thumbnail.updated")]
    ThumbnailUpdated { source_id: String },
    /// A benchmark started, began a step, finished a step, or ended.
    #[serde(rename = "benchmark.updated")]
    BenchmarkUpdated { run: Box<BenchmarkRunDto> },
    /// This node's name or monitor settings changed.
    #[serde(rename = "node.updated")]
    NodeUpdated,
    #[serde(rename = "node.online")]
    NodeOnline { peer_id: String },
    #[serde(rename = "node.offline")]
    NodeOffline { peer_id: String },
}

// ── Settings ──────────────────────────────────────────────────────────────────

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
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeSettingsDto {
    pub node_id: String,
    pub node_name: String,
    pub is_controller: bool,
    pub monitor: MonitorSettingsDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct UpdateNodeSettingsRequest {
    pub name: Option<String>,
    pub monitor: Option<MonitorSettingsDto>,
}

// ── Storage ───────────────────────────────────────────────────────────────────

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

// ── Benchmarks ────────────────────────────────────────────────────────────────

/// Body for starting a benchmark: the outputs to record (sent inline, like a
/// recording start) and the media file every feed plays.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct BenchmarkRequest {
    /// Informational: the preset the outputs came from.
    pub preset_id: Option<String>,
    pub preset_name: Option<String>,
    pub outputs: Vec<PresetOutputInput>,
    /// Absolute path on the node of the footage every feed plays in a loop.
    pub media_path: String,
    /// Search no higher than this many feeds. Default 64.
    #[serde(default)]
    pub max_feeds: Option<u32>,
    /// Seconds each step is measured for, after a warm-up. Default 20.
    #[serde(default)]
    pub step_secs: Option<u32>,
    /// A step fails once more than this percentage of frames is dropped.
    /// Default 0.5.
    #[serde(default)]
    pub drop_threshold_pct: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum BenchmarkStatus {
    Running,
    /// Ran until a step failed or it reached the feed limit.
    Completed,
    Cancelled,
    Error,
}

/// One step of a benchmark: `feeds` feeds recording at once, measured over
/// `secs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct BenchmarkStepDto {
    pub feeds: u32,
    pub secs: f64,
    /// Frames the feeds should have produced, from the footage's frame rate.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub expected_frames: u64,
    /// Frames the feeds didn't produce: decoding or the monitors fell behind.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub source_shortfall: u64,
    /// Frames the outputs dropped because an encoder couldn't keep up.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub output_dropped: u64,
    /// The larger of the two, as a percentage of the frames involved.
    pub drop_pct: f64,
    /// Whole-machine CPU use, averaged over the step.
    pub cpu_pct: f64,
    /// Whole-machine memory use at the end of the step.
    pub memory_pct: f64,
    /// Everything the step wrote, per second.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub write_bytes_per_sec: u64,
    /// Per output, ordered like the run's `outputs`: bytes per second one
    /// feed wrote, averaged over the feeds.
    #[cfg_attr(feature = "export-types", ts(type = "Array<number>"))]
    pub output_bytes_per_sec: Vec<u64>,
    pub passed: bool,
    /// A quick check while the feeds are doubled: it passes only well clear of
    /// the limit, and is otherwise followed by a full-length step.
    #[serde(default)]
    pub quick: bool,
}

/// A benchmark run, as stored and as served.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct BenchmarkRunDto {
    pub id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: BenchmarkStatus,
    pub preset_id: Option<String>,
    pub preset_name: Option<String>,
    pub outputs: Vec<PresetOutputInput>,
    pub media_path: String,
    pub media: MediaInfo,
    pub max_feeds: u32,
    pub step_secs: u32,
    pub drop_threshold_pct: f64,
    /// Feeds currently running (0 once finished). Goes down as well as up:
    /// the search narrows in on the answer.
    pub feeds_running: u32,
    pub steps: Vec<BenchmarkStepDto>,
    /// The most feeds that kept up (while running, the most so far). 0 if
    /// even one feed couldn't.
    pub sustainable_feeds: u32,
    /// The encoder element each output used, ordered like `outputs`.
    pub encoders: Vec<String>,
    /// Why the run ended: the failed step, the feed limit, a cancel, or an
    /// error.
    pub message: Option<String>,
    /// Where the run writes its files (deleted when it ends).
    pub scratch_dirs: Vec<String>,
}

// ── Capacity ──────────────────────────────────────────────────────────────────

/// What one benchmarked recording setup sustains on this node: the latest
/// finished run for those outputs and that footage format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ProfileCapacityDto {
    pub run_id: String,
    pub finished_at: String,
    pub preset_name: Option<String>,
    pub outputs: Vec<PresetOutputInput>,
    pub media: MediaInfo,
    pub sustainable_feeds: u32,
    /// Every step passed, so the node may sustain more than
    /// `sustainable_feeds`.
    pub at_limit: bool,
    pub encoders: Vec<String>,
    /// Bytes per second one feed writes, all outputs together.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub bytes_per_sec_per_feed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeCapacityDto {
    /// Latest result per benchmarked setup, newest first.
    pub profiles: Vec<ProfileCapacityDto>,
    pub active_feeds: u32,
    /// Share of the benchmarked capacity the active recordings use (1 = full),
    /// counting only those with a matching benchmark.
    pub load: f64,
    /// Active recordings with no benchmark to judge them by.
    pub unknown_feeds: u32,
    /// The benchmark running now, if any.
    pub running: Option<BenchmarkRunDto>,
}

/// Would recording `source_ids` with `outputs` fit, on top of what's
/// recording already?
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CapacityCheckRequest {
    pub outputs: Vec<PresetOutputInput>,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum CapacityVerdict {
    /// Within 80% of the benchmarked capacity.
    Fits,
    /// Between 80% and 100%.
    Tight,
    Over,
    /// Some of it has no benchmark to judge by (and what does fits).
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CapacityCheckDto {
    pub verdict: CapacityVerdict,
    /// Load before and after starting (see [`NodeCapacityDto::load`]).
    pub load_before: f64,
    pub load_after: f64,
    /// Feeds (active or new) with no benchmark to judge them by.
    pub unknown_feeds: u32,
    /// A benchmark for these outputs was scaled from footage of another
    /// format, so the numbers are approximate.
    pub estimated: bool,
    /// Volumes the new recordings would write to.
    pub volumes: Vec<VolumeCheckDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct VolumeCheckDto {
    pub mount_point: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub available_bytes: u64,
    /// Active recordings plus the new ones (estimated from a benchmark, or
    /// the outputs' bitrates).
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub write_bytes_per_sec: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub seconds_left: Option<u64>,
}

// ── Controller ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
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
    /// See [`NodeStatus::encoders`]. Empty until the node has answered.
    pub encoders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AddNodeRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ControllerToggleRequest {
    pub enabled: bool,
}
