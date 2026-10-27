//! Live sources: what a node is told about (configured sources), what it can
//! see (devices, Decklink cards, NDI senders), and how they're served.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceTypeSupport {
    pub source_type: SourceType,
    /// GStreamer elements this node lacks for the type; empty if it can
    /// run it.
    pub missing: Vec<String>,
    /// Streams: the URL schemes this node can open. Channels: the output
    /// types it can send (`ndi`, `srt`).
    pub protocols: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum SourceType {
    Test,
    Ndi,
    File,
    Stream,
    Device,
    Whip,
    Channel,
    Decklink,
}

/// Whether a live source (stream, device, WHIP) is delivering frames. While
/// it isn't, the feed plays black and silence, and recordings keep going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum LinkState {
    Live,
    /// Connecting out for the first time.
    Connecting,
    /// Waiting for a sender to connect (a listener or WHIP).
    Waiting,
    /// Was live, and is retrying.
    Reconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceCapabilitiesDto {
    pub max_width: u32,
    pub max_height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
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
    /// Live sources only.
    #[serde(default)]
    pub link: Option<LinkState>,
}

// Sources a node is told about, as opposed to discovered (NDI). Stored in
// `configured_sources` as JSON tagged with its `type`.

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
    Stream(StreamSourceConfig),
    Device(DeviceSourceConfig),
    Whip(WhipSourceConfig),
    Channel(ChannelConfig),
    Decklink(DecklinkSourceConfig),
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

/// A network stream pulled (or, for listeners, received) from a URL:
/// `rtsp://`, `rtsps://`, `srt://`, `rtmp://`, `http(s)://` (HLS) or
/// `udp://` (MPEG-TS). SRT is a caller unless the URL says
/// `mode=listener` (or has no host), which is how OBS pushes to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct StreamSourceConfig {
    pub url: String,
    /// Jitter buffer for RTSP and SRT, in milliseconds.
    #[serde(default = "default_stream_latency")]
    pub latency_ms: u32,
    #[serde(default)]
    pub rtsp_transport: RtspTransport,
    #[serde(default)]
    pub audio: AudioPlan,
    /// Hold the feed to this size and rate; by default it takes the
    /// stream's first format.
    #[serde(default)]
    pub format: Option<VideoFormat>,
}

fn default_stream_latency() -> u32 {
    200
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum RtspTransport {
    #[default]
    Auto,
    Tcp,
    Udp,
}

/// Where a live source's audio comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum AudioPlan {
    /// The source's own audio, mixed to `channels` (silence if it has none).
    Source {
        channels: u32,
    },
    /// An audio device on the node (an interface, Dante Virtual Soundcard):
    /// `channels` are its 1-based input channels, in output order.
    Device {
        device_key: String,
        channels: Vec<u32>,
    },
    Silence {
        channels: u32,
    },
}

impl Default for AudioPlan {
    fn default() -> Self {
        Self::Source { channels: 2 }
    }
}

/// A capture device on the node (a webcam, an HDMI/USB capture card, or on
/// macOS and Windows a screen), with audio from another device or silence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DeviceSourceConfig {
    /// [`DeviceDto::key`] of the video device.
    pub video_device: String,
    /// The device's name when the source was saved, shown while it's
    /// unplugged. Filled in by the node; ignored in requests.
    #[serde(default)]
    pub video_device_name: String,
    #[serde(default = "device_audio")]
    pub audio: AudioPlan,
    /// Capture at this size and rate (the device's default otherwise).
    #[serde(default)]
    pub format: Option<VideoFormat>,
}

/// SDI (or HDMI) on a Blackmagic Decklink card: one sub-device's video and
/// its embedded audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkSourceConfig {
    /// [`DecklinkDeviceDto::key`] of the sub-device.
    pub device: String,
    /// The sub-device's name when the source was saved, shown while it's
    /// missing. Filled in by the node; ignored in requests.
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub connection: DecklinkConnection,
    /// A [`DecklinkModeDto::mode`], or none to follow whatever signal
    /// arrives.
    #[serde(default)]
    pub mode: Option<String>,
    /// Embedded audio channels: 2, 8 or 16.
    #[serde(default = "default_decklink_channels")]
    pub channels: u32,
    /// Hold the feed to this size and rate; by default it takes the
    /// signal's first format.
    #[serde(default)]
    pub format: Option<VideoFormat>,
}

fn default_decklink_channels() -> u32 {
    2
}

/// Which input on a Decklink sub-device to capture from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum DecklinkConnection {
    #[default]
    Auto,
    Sdi,
    Hdmi,
    OpticalSdi,
}

/// How a Decklink card splits its connectors into sub-devices (the
/// `profile` of GStreamer's decklink elements). Half-duplex sub-devices
/// capture or play out; full-duplex ones can do both at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum DecklinkProfile {
    /// Leave the card as Desktop Video Setup left it.
    #[default]
    Default,
    OneSubDeviceFull,
    OneSubDeviceHalf,
    TwoSubDevicesFull,
    TwoSubDevicesHalf,
    FourSubDevicesHalf,
}

/// The node's Decklink cards, and whether it can use them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkStatusDto {
    /// Why Decklink can't be used on this node (no plugin, no driver).
    pub unavailable: Option<String>,
    pub cards: Vec<DecklinkCardDto>,
}

/// One Decklink card: the sub-devices sharing a profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkCardDto {
    /// Its serial number where the driver gives one; what the profile is
    /// stored under.
    pub key: String,
    pub name: String,
    pub profile: DecklinkProfile,
    pub devices: Vec<DecklinkDeviceDto>,
}

/// A Decklink sub-device: one SDI connector (or a pair, full duplex).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkDeviceDto {
    /// Stable across restarts and slot order (the driver's persistent id).
    pub key: String,
    pub name: String,
    pub capture: bool,
    pub playout: bool,
    /// The modes it offers, from the driver.
    pub modes: Vec<DecklinkModeDto>,
    /// Sources and channels on this node using it.
    pub used_by: Vec<DecklinkUseDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkUseDto {
    /// The configured source (or channel) id.
    pub source_id: String,
    pub name: String,
    /// Capturing (a source) or playing out (a channel's output).
    pub playout: bool,
}

/// A video mode a Decklink sub-device can run in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkModeDto {
    /// GStreamer's name for it (`1080p5994`).
    pub mode: String,
    /// What people call it (`1080p 59.94`).
    pub label: String,
    #[serde(flatten)]
    pub format: VideoFormat,
    pub interlaced: bool,
}

/// Body for setting a card's profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DecklinkProfileRequest {
    pub profile: DecklinkProfile,
}

/// WebRTC pushed to the node over WHIP (OBS 30+, browsers), at
/// `http://<node>:<port>/whip/endpoint`. One publisher at a time, and only
/// one that sends `Authorization: Bearer <token>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct WhipSourceConfig {
    pub port: u16,
    /// Publishers' bearer token. Blank in a request: the node makes one.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub audio: AudioPlan,
    #[serde(default)]
    pub format: Option<VideoFormat>,
}

/// Which NDI senders a node connects to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum NdiConnect {
    /// Every sender on the network, as it's found.
    #[default]
    All,
    /// Only the chosen ones, and any a recording or replay buffer uses.
    Chosen,
}

/// Which NDI senders a node connects to, as configured. A node never
/// connects to its own outputs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NdiSubscriptionsInput {
    pub connect: NdiConnect,
    /// NDI names (`HOST (name)`), used when `connect` is `chosen`.
    pub chosen: Vec<String>,
}

/// An NDI sender on the network, as a node sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NdiSenderDto {
    pub ndi_name: String,
    /// The source it is, or would be, on this node.
    pub source_id: String,
    /// It's a source on this node.
    pub connected: bool,
    /// A recording or replay buffer uses it, so it stays connected.
    pub in_use: bool,
    /// One of this node's own outputs, never connected.
    pub own: bool,
}

/// A node's NDI subscriptions and the senders it can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NdiSubscriptionsDto {
    pub connect: NdiConnect,
    pub chosen: Vec<String>,
    pub senders: Vec<NdiSenderDto>,
}

fn device_audio() -> AudioPlan {
    AudioPlan::Silence { channels: 2 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum DeviceKind {
    Video,
    Audio,
    Screen,
    /// Somewhere the node plays sound (for display outputs' audio).
    Output,
}

/// A capture device the node can see, for picking one in a device source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DeviceDto {
    /// Stable across restarts and replugging, where the platform allows.
    pub key: String,
    pub name: String,
    pub kind: DeviceKind,
    /// The GStreamer device API (`v4l2`, `pipewire`, `avf`, `mediafoundation`, …).
    pub api: String,
    /// Audio devices: input channels.
    pub channels: Option<u32>,
    /// Video devices: the sizes and rates offered, largest first.
    pub modes: Vec<DeviceMode>,
}

/// A size and rate a video device offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DeviceMode {
    /// The rate is 0/1 when the device doesn't say.
    #[serde(flatten)]
    pub format: VideoFormat,
    /// Pixel formats it comes in (`YUY2`, `NV12`; `jpeg` for MJPEG).
    pub formats: Vec<String>,
}

/// A frame size and rate: a live source's fixed format, a channel's
/// program, a device or SDI mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl VideoFormat {
    pub const fn new(width: u32, height: u32, (fps_num, fps_den): (u32, u32)) -> Self {
        Self {
            width,
            height,
            fps_num,
            fps_den,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Frames per second; 0 when the denominator is (a device that doesn't say).
    pub fn fps(&self) -> f64 {
        f64::from(self.fps_num) / f64::from(self.fps_den.max(1))
    }

    /// Exactly the same rate, however the fraction is written (`60/2` is `30/1`).
    pub fn same_rate(&self, other: &Self) -> bool {
        u64::from(self.fps_num) * u64::from(other.fps_den)
            == u64::from(other.fps_num) * u64::from(self.fps_den)
    }
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
