//! Capacity estimates from benchmark results: how many feeds a recording
//! setup sustains on this node, and how much of that the active recordings
//! use.
//!
//! A benchmark measures one setup — a set of outputs, recording footage of
//! one format — so results are looked up by both. Each feed of a setup that
//! sustains N feeds uses 1/N of the node, and the active recordings' shares
//! add up to the node's load (1 = full). That holds across setups only
//! roughly (an encoder bound by the GPU and one bound by the CPU share less
//! than this assumes), so it's a guide, not a guarantee.

use crate::api::types::{
    BenchmarkRunDto, BenchmarkStatus, CapacityVerdict, OutputAdvanced, PresetOutputInput, ProfileCapacityDto,
};
use crate::pipeline::monitor::SourceFormat;

/// Load above which a node is "tight" rather than comfortably within its
/// benchmarked capacity.
const TIGHT_LOAD: f64 = 0.8;

/// Identifies what a set of outputs encodes, ignoring where it writes and
/// how it splits files, which don't change the work.
pub fn outputs_key(outputs: &[PresetOutputInput]) -> String {
    let normalized: Vec<PresetOutputInput> = outputs
        .iter()
        .map(|o| PresetOutputInput {
            name: String::new(),
            path_template: String::new(),
            advanced: OutputAdvanced { split_minutes: None, split_gb: None, ..o.advanced.clone() },
            ..o.clone()
        })
        .collect();
    serde_json::to_string(&normalized).unwrap_or_default()
}

struct Profile {
    key: String,
    dto: ProfileCapacityDto,
    /// Per output, bytes per second one feed writes.
    output_bytes_per_sec: Vec<u64>,
}

impl Profile {
    fn pixel_rate(&self) -> f64 {
        let m = &self.dto.media;
        m.width as f64 * m.height as f64 * m.fps_num as f64 / m.fps_den.max(1) as f64
    }

    fn matches_format(&self, (w, h): (u32, u32), (n, d): (u32, u32)) -> bool {
        let m = &self.dto.media;
        // 29.97 can be 30000/1001 or 2997/100.
        let rate = |n: u32, d: u32| n as f64 / d.max(1) as f64;
        m.width == w && m.height == h && (rate(m.fps_num, m.fps_den) - rate(n, d)).abs() < 0.01
    }
}

/// What a lookup found for one feed.
pub struct Match {
    /// Feeds of this setup the node sustains (fractional when scaled).
    pub feeds: f64,
    /// Scaled from a benchmark of another footage format.
    pub estimated: bool,
    /// Per output, bytes per second one feed writes; empty if unknown.
    pub output_bytes_per_sec: Vec<u64>,
}

impl Match {
    /// The share of the node one feed uses. A setup that couldn't sustain
    /// even one feed counts as twice the node.
    fn load(&self) -> f64 {
        1.0 / self.feeds.max(0.5)
    }
}

/// Feeds added up against the node's capacity.
#[derive(Debug, Default, Clone, Copy)]
pub struct Load {
    pub load: f64,
    /// Feeds with no benchmark to judge them by.
    pub unknown: u32,
    pub estimated: bool,
}

impl Load {
    pub fn verdict(&self) -> CapacityVerdict {
        if self.load > 1.0 {
            CapacityVerdict::Over
        } else if self.unknown > 0 {
            CapacityVerdict::Unknown
        } else if self.load > TIGHT_LOAD {
            CapacityVerdict::Tight
        } else {
            CapacityVerdict::Fits
        }
    }
}

/// The latest finished benchmark for each setup on this node.
pub struct Capacity {
    profiles: Vec<Profile>,
}

impl Capacity {
    /// From stored runs, newest first. Only completed runs count: a cancelled
    /// or failed one never found its limit.
    pub fn from_runs(runs: &[BenchmarkRunDto]) -> Self {
        let mut profiles: Vec<Profile> = Vec::new();
        for run in runs.iter().filter(|r| r.status == BenchmarkStatus::Completed && !r.steps.is_empty()) {
            let key = outputs_key(&run.outputs);
            let m = &run.media;
            if profiles.iter().any(|p| p.key == key && p.matches_format((m.width, m.height), (m.fps_num, m.fps_den))) {
                continue;
            }
            // A full step at the answer measures what a feed writes when the
            // node keeps up; failing that any step that passed, and if none
            // did, the first step is all there is.
            let step = run
                .steps
                .iter()
                .rev()
                .find(|s| s.passed && !s.quick && s.feeds == run.sustainable_feeds)
                .or_else(|| run.steps.iter().rev().find(|s| s.passed))
                .or(run.steps.first())
                .expect("non-empty");
            // Every step up to the limit kept up (one re-measured after a
            // hiccup still counts).
            let at_limit = run.sustainable_feeds >= run.max_feeds;
            profiles.push(Profile {
                key,
                output_bytes_per_sec: step.output_bytes_per_sec.clone(),
                dto: ProfileCapacityDto {
                    run_id: run.id.clone(),
                    finished_at: run.finished_at.clone().unwrap_or_else(|| run.started_at.clone()),
                    preset_name: run.preset_name.clone(),
                    outputs: run.outputs.clone(),
                    media: run.media.clone(),
                    sustainable_feeds: run.sustainable_feeds,
                    at_limit,
                    encoders: run.encoders.clone(),
                    bytes_per_sec_per_feed: step.output_bytes_per_sec.iter().sum(),
                },
            });
        }
        Self { profiles }
    }

    pub fn profiles(&self) -> Vec<ProfileCapacityDto> {
        self.profiles.iter().map(|p| p.dto.clone()).collect()
    }

    /// What the node sustains of the setup `key` recording a source of
    /// `format`. A benchmark of the same format answers exactly; failing
    /// that, the nearest format's result is scaled by pixels per second, and
    /// a source whose format isn't known yet takes the newest result as is.
    pub fn lookup(&self, key: &str, format: SourceFormat) -> Option<Match> {
        let candidates: Vec<&Profile> = self.profiles.iter().filter(|p| p.key == key).collect();
        let found = |p: &Profile, feeds: f64, estimated: bool| Match {
            feeds,
            estimated,
            output_bytes_per_sec: p.output_bytes_per_sec.clone(),
        };
        let sustained = |p: &Profile| p.dto.sustainable_feeds as f64;
        let (Some(size), Some(rate)) = (format.size, format.rate) else {
            return candidates.first().map(|p| found(p, sustained(p), true));
        };
        if let Some(p) = candidates.iter().find(|p| p.matches_format(size, rate)) {
            return Some(found(p, sustained(p), false));
        }
        let pixel_rate = size.0 as f64 * size.1 as f64 * rate.0 as f64 / rate.1.max(1) as f64;
        candidates
            .iter()
            .min_by(|a, b| {
                let distance = |p: &Profile| (p.pixel_rate() / pixel_rate).ln().abs();
                distance(a).total_cmp(&distance(b))
            })
            .map(|p| found(p, sustained(p) * p.pixel_rate() / pixel_rate, true))
    }

    /// Add up feeds of `(outputs key, source format)`.
    pub fn load<'a>(&self, feeds: impl IntoIterator<Item = (&'a str, SourceFormat)>) -> Load {
        let mut total = Load::default();
        for (key, format) in feeds {
            match self.lookup(key, format) {
                Some(m) => {
                    total.load += m.load();
                    total.estimated |= m.estimated;
                }
                None => total.unknown += 1,
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{BenchmarkStepDto, ChromaSubsampling, Container, MediaInfo, VideoCodec};

    fn output(name: &str, path: &str) -> PresetOutputInput {
        PresetOutputInput {
            name: name.into(),
            codec: VideoCodec::H264,
            container: Container::Mov,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: path.into(),
            advanced: OutputAdvanced::default(),
        }
    }

    fn run(id: &str, (w, h, n, d): (u32, u32, u32, u32), sustainable: u32, passed: &[bool]) -> BenchmarkRunDto {
        BenchmarkRunDto {
            id: id.into(),
            started_at: format!("2026-10-0{}T00:00:00Z", id.len()),
            finished_at: None,
            status: BenchmarkStatus::Completed,
            preset_id: None,
            preset_name: None,
            outputs: vec![output("Main", "~/a/{source}.{ext}")],
            media_path: "/f.mov".into(),
            media: MediaInfo { width: w, height: h, fps_num: n, fps_den: d, audio_channels: 2, duration_ms: None },
            max_feeds: passed.len() as u32,
            step_secs: 20,
            drop_threshold_pct: 0.5,
            feeds_running: 0,
            steps: passed
                .iter()
                .enumerate()
                .map(|(i, &passed)| BenchmarkStepDto {
                    feeds: i as u32 + 1,
                    secs: 20.0,
                    expected_frames: 0,
                    source_shortfall: 0,
                    output_dropped: 0,
                    drop_pct: 0.0,
                    cpu_pct: 0.0,
                    memory_pct: 0.0,
                    write_bytes_per_sec: 0,
                    output_bytes_per_sec: vec![1_000 * (i as u64 + 1)],
                    passed,
                    quick: false,
                })
                .collect(),
            sustainable_feeds: sustainable,
            encoders: vec!["x264enc".into()],
            message: None,
            scratch_dirs: Vec::new(),
        }
    }

    fn format(w: u32, h: u32, n: u32, d: u32) -> SourceFormat {
        SourceFormat { size: Some((w, h)), rate: Some((n, d)), audio: None }
    }

    #[test]
    fn key_ignores_names_paths_and_splits() {
        let a = output("Main", "~/a/{source}.{ext}");
        let mut b = output("Other", "/media/{date}/{source}.{ext}");
        b.advanced.split_minutes = Some(30);
        assert_eq!(outputs_key(std::slice::from_ref(&a)), outputs_key(&[b]));
        let mut c = a.clone();
        c.bitrate_kbps = Some(8000);
        assert_ne!(outputs_key(&[a]), outputs_key(&[c]));
    }

    #[test]
    fn exact_format_wins_and_others_scale() {
        let cap = Capacity::from_runs(&[
            run("hd", (1920, 1080, 30, 1), 4, &[true, true, true, true, false]),
            run("uhd", (3840, 2160, 30, 1), 1, &[true, false]),
        ]);
        let key = outputs_key(&[output("x", "y")]);

        let m = cap.lookup(&key, format(1920, 1080, 30, 1)).unwrap();
        assert_eq!((m.feeds, m.estimated), (4.0, false));
        assert_eq!(m.output_bytes_per_sec, [4_000]);

        // 29.97 isn't 30: scaled from the 1080p30 run.
        let m = cap.lookup(&key, format(1920, 1080, 30000, 1001)).unwrap();
        assert!(m.estimated);
        assert!((m.feeds - 4.004).abs() < 1e-9);

        // 720p30 is nearest to the 1080p run: 2.25× the pixels per second.
        let m = cap.lookup(&key, format(1280, 720, 30, 1)).unwrap();
        assert!(m.estimated);
        assert!((m.feeds - 9.0).abs() < 1e-9);

        assert!(cap.lookup("other", format(1920, 1080, 30, 1)).is_none());
    }

    #[test]
    fn newest_run_per_setup_counts() {
        // Newest first: the 3-feed result replaces the older 5-feed one.
        let cap = Capacity::from_runs(&[
            run("newer", (1920, 1080, 30, 1), 3, &[true, true, true, false]),
            run("old", (1920, 1080, 30, 1), 5, &[true, true, true, true, true, false]),
        ]);
        assert_eq!(cap.profiles().len(), 1);
        assert_eq!(cap.profiles()[0].sustainable_feeds, 3);
    }

    #[test]
    fn load_and_verdict() {
        let cap = Capacity::from_runs(&[run("hd", (1920, 1080, 30, 1), 4, &[true, true, true, true, false])]);
        let key = outputs_key(&[output("x", "y")]);
        let hd = format(1920, 1080, 30, 1);

        let load = cap.load([(key.as_str(), hd); 3]);
        assert!((load.load - 0.75).abs() < 1e-9);
        assert_eq!(load.verdict(), CapacityVerdict::Fits);
        assert_eq!(cap.load([(key.as_str(), hd); 4]).verdict(), CapacityVerdict::Tight);
        assert_eq!(cap.load([(key.as_str(), hd); 5]).verdict(), CapacityVerdict::Over);
        assert_eq!(cap.load([(key.as_str(), hd), ("other", hd)]).verdict(), CapacityVerdict::Unknown);

        // Even one feed failed: one feed is already over.
        let cap = Capacity::from_runs(&[run("hd", (1920, 1080, 30, 1), 0, &[false])]);
        assert_eq!(cap.load([(key.as_str(), hd)]).verdict(), CapacityVerdict::Over);
    }
}
