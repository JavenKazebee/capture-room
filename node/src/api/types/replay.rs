//! Replay buffers and the clips saved from them.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

/// What a replay buffer records with. All intra-frame, so it can be cut,
/// played backwards and stepped on any frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ReplayCodec {
    /// ProRes 422 Proxy (VideoToolbox: macOS only). About 40 GB an hour at
    /// 1080p59.94.
    ProresProxy,
    /// H.264 with every frame a keyframe: ProRes Proxy's picture at about
    /// 60% of the size, 20–30 GB an hour at 1080p59.94.
    H264Intra,
    /// Motion JPEG: any platform, in software. About 40–55 GB an hour at
    /// 1080p59.94.
    Mjpeg,
}

/// A source's replay buffer, as configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ReplayConfigInput {
    pub codec: ReplayCodec,
    /// How much it keeps, in minutes.
    pub minutes: u32,
}

/// A source's replay buffer and what it holds. Times are on the node clock
/// (µs), with `utc_offset_us` mapping them to UTC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ReplayBufferDto {
    pub source_id: String,
    pub source_name: Option<String>,
    pub codec: ReplayCodec,
    pub minutes: u32,
    /// The encoder element recording it, while it records.
    pub encoder: Option<String>,
    /// Why it isn't recording, if it should be.
    pub error: Option<String>,
    /// The span that can be replayed now: from the oldest frame kept to the
    /// newest in a finished segment. `None` while empty.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub start_us: Option<u64>,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub end_us: Option<u64>,
    /// Where the span has gaps (the source restarted), as the start of each
    /// stretch after the first.
    #[cfg_attr(feature = "export-types", ts(type = "number[]"))]
    pub breaks_us: Vec<u64>,
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub utc_offset_us: Option<i64>,
    /// The node clock's time now, to measure the span against.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub now_us: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub bytes: u64,
}

/// Play part of a replay buffer on a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ReplayClipRequest {
    pub source_id: String,
    /// From when, on the node clock (µs). Moved into the buffer's span if
    /// it's outside it.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub from_us: u64,
    /// Until when. If `None`, on into what's recorded meanwhile: no out
    /// point, and at the newest frame it holds until more is recorded.
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub to_us: Option<u64>,
    /// 1 if `None`. Negative plays backwards, from `to_us`.
    #[serde(default)]
    pub rate: Option<f64>,
    /// Play straight away; otherwise it's cued on its first frame.
    #[serde(default)]
    pub play: bool,
}

/// A saved replay clip: part of a replay buffer kept as its own files, so it
/// stays playable after the buffer has rolled past it (and across restarts).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ReplayClipDto {
    pub id: String,
    pub source_id: String,
    /// The angle's name when the clip was saved.
    pub source_name: String,
    pub name: String,
    pub codec: ReplayCodec,
    /// Its in and out on the node clock (µs) when it was saved. They only
    /// line up with a buffer until the node restarts.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub from_us: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub to_us: u64,
    /// When its first frame was captured (UTC, µs since the epoch), if the
    /// node knew its clock's UTC offset.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub start_utc_us: Option<i64>,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub bytes: u64,
    pub created_at: String,
    /// Some of its files are no longer on disk.
    #[serde(default)]
    pub missing: bool,
    /// The group it was saved in with other angles (Shift+S), and its name.
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub group_name: Option<String>,
}

/// Save part of a source's replay buffer as a clip.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SaveReplayClipRequest {
    pub source_id: String,
    /// In and out on the node clock (µs), moved into the buffer's span.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub from_us: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub to_us: u64,
    /// The angle and the time of day it starts, if `None`.
    #[serde(default)]
    pub name: Option<String>,
}

/// Save one in → out on several angles: a clip each, grouped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SaveReplayClipGroupRequest {
    /// The angles, at least one. Every one's buffer must hold the span.
    pub source_ids: Vec<String>,
    /// In and out on the node clock (µs), moved into each buffer's span.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub from_us: u64,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub to_us: u64,
    /// "Replay" and the time of day it starts, if `None`.
    #[serde(default)]
    pub name: Option<String>,
}

/// Replay clips saved together on several angles; each clip is named after
/// its angle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ReplayClipGroupDto {
    pub id: String,
    pub name: String,
    pub clips: Vec<ReplayClipDto>,
}

/// Rename a saved replay clip, or a group of them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct UpdateReplayClipRequest {
    pub name: String,
}

/// Play (or cue) a saved replay clip on a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PlayReplayClipRequest {
    pub clip_id: String,
    /// 1 if `None`. Negative plays backwards, from its out point.
    #[serde(default)]
    pub rate: Option<f64>,
    /// Play straight away; otherwise it's cued on its first frame.
    #[serde(default)]
    pub play: bool,
    /// Cue it here instead, on the node clock (µs) as its `from_us` and
    /// `to_us` are: another angle of its group at the same moment.
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub at_us: Option<u64>,
}
