//! What the sockets carry: `/ws` events, thumbnail and log subscriptions, log
//! lines.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

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
    /// A channel's transport: on every change, and ~10 Hz while playing.
    #[serde(rename = "channel.state")]
    ChannelState {
        source_id: String,
        status: Box<ChannelStatusDto>,
    },
    /// The media library changed.
    #[serde(rename = "media.updated")]
    MediaUpdated,
    /// A replay clip was saved, renamed or deleted.
    #[serde(rename = "replay.clips")]
    ReplayClipsUpdated,
    /// Every replay buffer on the node, when one starts, stops, fails, is
    /// reconfigured or fills up: not per segment. Meanwhile a recording
    /// buffer's `end_us` runs on with `now_us`, and once full its start
    /// follows `minutes` behind.
    #[serde(rename = "replay.buffers")]
    ReplayBuffers { buffers: Vec<ReplayBufferDto> },
    #[serde(rename = "feed.status")]
    FeedStatus {
        source_id: String,
        timecode: Option<String>,
        error: Option<String>,
        link: Option<LinkState>,
        capabilities: Option<SourceCapabilitiesDto>,
    },
    #[serde(rename = "audio.levels")]
    AudioLevels {
        source_id: String,
        channels: Vec<ChannelLevelDto>,
    },
    /// A benchmark started, began a step, finished a step, or ended.
    #[serde(rename = "benchmark.updated")]
    BenchmarkUpdated { run: Box<BenchmarkRunDto> },
    /// A configured source was added, changed or removed: the node's
    /// source list is different.
    #[serde(rename = "sources.changed")]
    SourcesChanged,
    /// This node's name or monitor settings changed; from a controller, also
    /// its approved or found nodes.
    #[serde(rename = "node.updated")]
    NodeUpdated,
    #[serde(rename = "node.online")]
    NodeOnline { peer_id: String },
    #[serde(rename = "node.offline")]
    NodeOffline { peer_id: String },
    /// A peer left the controller's list: removed by hand, or pruned after
    /// staying unreachable.
    #[serde(rename = "node.removed")]
    NodeRemoved { peer_id: String },
    /// A schedule was added, changed or removed, or one of its runs started
    /// or ended (`run`).
    #[serde(rename = "schedule.updated")]
    ScheduleUpdated {
        schedule_id: String,
        name: String,
        run: Option<Box<ScheduleRunDto>>,
    },
}

/// One `/ws` message: an event and the node it describes. Source and session
/// ids inside it are local to that node.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct NodeEvent<'a> {
    pub node_id: &'a str,
    #[serde(flatten)]
    pub event: &'a WsEvent,
}

/// The sources a thumbnail socket's client shows, and the most frames per
/// second it wants of each. Sent as a text message; each replaces the last.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ThumbnailSubscription {
    pub sources: Vec<ThumbnailSource>,
    /// `None`: as fast as the monitors make them.
    #[serde(default)]
    pub fps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ThumbnailSource {
    pub node_id: String,
    pub source_id: String,
}

/// Most severe first, so `a <= b` reads "`a` is at least as severe as `b`".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// One of a node's log lines (see [`crate::logs`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct LogLine {
    /// Numbers the node's lines since it started; 0 for a stand-in line.
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub seq: u64,
    /// RFC 3339, UTC, to the millisecond.
    pub at: String,
    pub level: LogLevel,
    /// The Rust module (or crate) that logged it.
    pub target: String,
    pub message: String,
    /// The line's other fields, as `key=value` pairs.
    pub fields: String,
    /// The node it came from; set on every line a node serves.
    #[serde(default)]
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct LogPage {
    /// Oldest first.
    pub lines: Vec<LogLine>,
    /// Where the node writes every line, when it runs as the service.
    pub log_file: Option<String>,
}

/// What a log socket's client wants: lines from `nodes` (ignored by a node's
/// own socket) at `level` or more severe. Each replaces the last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct LogSubscription {
    #[serde(default)]
    pub nodes: Vec<String>,
    pub level: LogLevel,
}
