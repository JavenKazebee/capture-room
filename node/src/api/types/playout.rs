//! Playout channels: their config, outputs, transport and playlists.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

/// A playout channel: clips played out at a fixed format, black and
/// silence when nothing plays. Its program is a source like any other, so it
/// can be monitored and recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ChannelConfig {
    pub format: VideoFormat,
    #[serde(default = "default_channel_audio")]
    pub audio_channels: u32,
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
}

fn default_channel_audio() -> u32 {
    2
}

/// Where a channel's program is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum OutputConfig {
    /// An NDI sender, named `<node> (<ndi_name or channel name>)` on the
    /// network.
    Ndi {
        #[serde(default)]
        ndi_name: Option<String>,
    },
    /// An SRT stream (H.264 and AAC in MPEG-TS). A URL with no host, like
    /// `srt://:9000`, listens for receivers to connect; one with a host
    /// calls it, and keeps retrying until it answers.
    Srt {
        url: String,
        #[serde(default = "default_srt_latency")]
        latency_ms: u32,
        #[serde(default = "default_srt_bitrate")]
        bitrate_kbps: u32,
        /// Encrypts the stream; receivers need the same one. 10–79
        /// characters.
        #[serde(default)]
        passphrase: Option<String>,
    },
    /// Served by the node's RTSP server at `rtsp://<node>:8554/<path>`
    /// (H.264 and AAC), encoded only while someone is watching.
    Rtsp {
        /// The mount path; the channel's name when unset. Made URL-safe.
        #[serde(default)]
        path: Option<String>,
        #[serde(default = "default_srt_bitrate")]
        bitrate_kbps: u32,
    },
    /// WebRTC to browsers and other WHEP players, at
    /// `http://<node>:<API port>/whep/<path>`, encoded for each viewer (H.264
    /// or VP8, and Opus stereo).
    Whep {
        /// The endpoint path; the channel's name when unset. Made URL-safe.
        #[serde(default)]
        path: Option<String>,
        /// The most each viewer is sent; less when their link can't take it.
        #[serde(default = "default_srt_bitrate")]
        bitrate_kbps: u32,
    },
    /// SDI out of a Decklink sub-device, with the program's audio embedded
    /// (2, 8 or 16 channels).
    Decklink {
        /// A [`DecklinkDeviceDto::key`].
        device: String,
        /// A [`DecklinkModeDto::mode`]; by default the one matching the
        /// channel's format.
        #[serde(default)]
        mode: Option<String>,
    },
    /// Fullscreen on a screen attached to the node, set to the channel's
    /// format when the screen offers it (for HDMI→SDI converters).
    Display {
        /// A [`DisplayDto::id`].
        display: String,
        /// The audio output to play to (a [`DeviceDto::key`]), usually the
        /// screen's own HDMI audio so it's embedded; none for no audio.
        #[serde(default)]
        audio_device: Option<String>,
        /// What the screen shows while the channel has nothing loaded.
        #[serde(default)]
        idle: DisplayIdle,
    },
}

/// What a display output shows while its channel is idle (no clip loaded).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum DisplayIdle {
    /// The channel's black: the signal never drops (what an HDMI→SDI
    /// converter and whatever is downstream of it need).
    #[default]
    Black,
    /// The output window hides, so the screen shows the desktop.
    Desktop,
}

fn default_srt_latency() -> u32 {
    200
}

/// Default bitrate for encoded outputs (SRT, RTSP).
fn default_srt_bitrate() -> u32 {
    8000
}

/// What a channel does when a clip reaches its out point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ClipEnd {
    /// Hold the last frame (with silence).
    #[default]
    Hold,
    /// Go to black.
    Black,
    /// Play again from the in point.
    Loop,
    /// Play the playlist's next item, without a gap (holds after the last
    /// one unless the playlist loops).
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum TransportState {
    /// No clip: black and silence.
    Idle,
    /// A clip is loaded at its in point, showing its first frame.
    Cued,
    Playing,
    Paused,
    /// The clip reached its out point (holding its last frame, or black).
    Ended,
}

/// A channel's transport, as served and in `channel.state` events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ChannelStatusDto {
    pub state: TransportState,
    pub clip: Option<LoadedClipDto>,
    /// Position in the file.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub position_ms: Option<u64>,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub duration_ms: Option<u64>,
    /// Why the last clip stopped, if it failed.
    pub error: Option<String>,
    /// The channel's outputs, in config order.
    pub outputs: Vec<OutputStatusDto>,
    /// The playlist item on air (none for a clip that isn't in it).
    pub item_id: Option<String>,
    /// The item that plays next, cued in the background once it's ready.
    pub next_id: Option<String>,
    pub next_ready: bool,
    /// Bumped whenever the playlist changes, to know when to fetch it.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub playlist_rev: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct OutputStatusDto {
    pub label: String,
    /// Why the output isn't sending, if it isn't.
    pub error: Option<String>,
    /// How many receivers are connected, for outputs that know (an SRT
    /// listener, an RTSP mount).
    pub receivers: Option<u32>,
    /// More about how it's sending: a display output's screen mode.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct LoadedClipDto {
    pub media_id: String,
    pub name: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub in_ms: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub out_ms: Option<u64>,
    pub end: ClipEnd,
    /// Its speed (1 for normal).
    #[serde(default = "normal_rate")]
    pub rate: f64,
    /// A replay's source, and the node clock time (µs) of position 0, so a
    /// position maps back to the moment it shows.
    #[serde(default)]
    pub replay_source_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub replay_start_us: Option<u64>,
    /// The saved replay clip it plays, if it's one.
    #[serde(default)]
    pub replay_clip_id: Option<String>,
}

fn normal_rate() -> f64 {
    1.0
}

/// A channel's playlist, as served.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PlaylistDto {
    pub items: Vec<PlaylistItemDto>,
    /// After the last item, start again from the first.
    pub loop_playlist: bool,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub rev: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PlaylistItemDto {
    pub id: String,
    pub media_id: String,
    pub name: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub in_ms: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub out_ms: Option<u64>,
    pub end: ClipEnd,
    /// The file's length.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub duration_ms: Option<u64>,
    /// The file is no longer on disk.
    pub missing: bool,
}

/// Body replacing a channel's playlist: the whole list, in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PlaylistInput {
    pub items: Vec<PlaylistItemInput>,
    #[serde(default)]
    pub loop_playlist: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PlaylistItemInput {
    /// An existing item's id; new items leave it out.
    #[serde(default)]
    pub id: Option<String>,
    pub media_id: String,
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub in_ms: u64,
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub out_ms: Option<u64>,
    #[serde(default = "default_item_end")]
    pub end: ClipEnd,
}

fn default_item_end() -> ClipEnd {
    ClipEnd::Next
}

/// Body for cueing a playlist item: loaded, paused at its in point.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CueRequest {
    pub item_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum TransportAction {
    Play,
    Pause,
    /// Unload the clip: black and silence.
    Stop,
    /// Go to `position_ms` in the file.
    Seek,
    /// Take the next item: it plays now if the channel is playing or has
    /// ended, else it's cued.
    Next,
    /// Play at `rate` (1 is normal speed, 0.5 half, negative backwards).
    /// Only a replay plays at other speeds.
    Speed,
    /// Paused or cued: show the frame `frames` away (1 next, -1 previous).
    Step,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct TransportRequest {
    pub action: TransportAction,
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub position_ms: Option<u64>,
    #[serde(default)]
    pub rate: Option<f64>,
    #[serde(default)]
    pub frames: Option<i32>,
}

/// A screen attached to the node, for display outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DisplayDto {
    /// What a display output names it by.
    pub id: String,
    pub name: String,
    pub primary: bool,
    /// Its current mode.
    pub width: u32,
    pub height: u32,
    /// Refresh rate in millihertz (59.94 Hz is 59940); 0 when unknown.
    pub refresh_mhz: u32,
    /// The modes it offers, largest and fastest first.
    pub modes: Vec<DisplayModeDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct DisplayModeDto {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}
