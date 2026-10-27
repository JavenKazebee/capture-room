//! Benchmarks, and the capacity checks built from their results.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

use super::*;

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
    /// Benchmark replay buffers in this codec instead: each feed keeps a
    /// buffer rather than recording, and `outputs` is empty.
    #[serde(default)]
    pub replay: Option<ReplayCodec>,
    /// Benchmark playout channels with these outputs instead (NDI and SRT
    /// only; none measures the program alone): each feed is a channel at
    /// the footage's format playing it on loop, and `outputs` is empty.
    #[serde(default)]
    pub channel: Option<Vec<OutputConfig>>,
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
    /// Empty for a replay buffer run.
    pub outputs: Vec<PresetOutputInput>,
    /// What a replay buffer run's feeds keep their buffers in.
    #[serde(default)]
    pub replay: Option<ReplayCodec>,
    /// A playout channel run's outputs (see [`crate::capacity::channel_key`]).
    #[serde(default)]
    pub channel: Option<Vec<OutputConfig>>,
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
    /// The encoder element each output used, ordered like `outputs` (for a
    /// replay buffer run, the buffer's; for a channel run, its outputs'
    /// labels).
    pub encoders: Vec<String>,
    /// Why the run ended: the failed step, the feed limit, a cancel, or an
    /// error.
    pub message: Option<String>,
    /// Where the run writes its files (deleted when it ends).
    pub scratch_dirs: Vec<String>,
}

/// What one benchmarked setup sustains on this node: the latest finished
/// run for those outputs (or that replay codec) and that footage format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ProfileCapacityDto {
    pub run_id: String,
    pub finished_at: String,
    pub preset_name: Option<String>,
    /// Empty for replay buffers.
    pub outputs: Vec<PresetOutputInput>,
    /// Set for replay buffers in this codec.
    pub replay: Option<ReplayCodec>,
    /// Set for playout channels with these outputs (NDI and SRT only).
    pub channel: Option<Vec<OutputConfig>>,
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
    /// Active recordings.
    pub active_feeds: u32,
    /// Replay buffers recording.
    pub active_buffers: u32,
    /// Playout channels running.
    pub active_channels: u32,
    /// Share of the benchmarked capacity the active recordings, replay
    /// buffers and playout channels use (1 = full), counting only those with
    /// a matching benchmark.
    pub load: f64,
    /// Active recordings, replay buffers and channels with no benchmark to
    /// judge them by.
    pub unknown_feeds: u32,
    /// The benchmark running now, if any.
    pub running: Option<BenchmarkRunDto>,
}

/// Would recording `source_ids` with `outputs` (or giving them replay buffers
/// in `replay`, or running the playout `channel`) fit on top of what runs?
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct CapacityCheckRequest {
    /// Empty when checking replay buffers or a channel.
    pub outputs: Vec<PresetOutputInput>,
    /// For a channel, the channel being changed (it stops counting as it
    /// was), or none for a new one.
    pub source_ids: Vec<String>,
    /// Check replay buffers in this codec instead of recordings.
    #[serde(default)]
    pub replay: Option<ReplayCodec>,
    /// Check a playout channel with this format and outputs instead.
    #[serde(default)]
    pub channel: Option<ChannelConfig>,
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
    /// Volumes the new recordings would write to (none for replay buffers,
    /// which keep a fixed length).
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
