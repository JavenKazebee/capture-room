//! Measuring a step from counters taken across every feed.

use std::time::Instant;

use crate::api::types::{BenchmarkRunDto, BenchmarkStepDto};
use crate::capacity::Setup;
use crate::pipeline::monitor::VideoProgress;
use crate::sources::channel::PlayedProgress;

use super::run::{Feed, Work};

/// Counters across every feed at one moment.
pub(super) struct Snapshot {
    at: Instant,
    /// Per feed.
    frames: Vec<VideoProgress>,
    /// Per channel feed, how its clip has reached its program.
    played: Vec<VideoProgress>,
    /// Clip frames that reached a program too late to show, across feeds.
    late: u64,
    dropped: u64,
    /// Per output: bytes written so far, across every feed.
    bytes: Vec<u64>,
}

impl Snapshot {
    /// Replay buffers' one "output" counts what they've written, since old segments go.
    pub(super) async fn take(feeds: &[Feed], setup: Setup<'_>) -> anyhow::Result<Self> {
        let frames = feeds.iter().map(|f| f.monitor.video_progress()).collect();
        let played: Vec<PlayedProgress> = feeds.iter().filter_map(Feed::played).collect();
        let late = played.iter().map(|p| p.late).sum();
        let played = played.into_iter().map(|p| p.progress).collect();
        let dropped = feeds.iter().map(Feed::dropped_frames).sum();
        if let Setup::Replay(_) = setup {
            let written = feeds
                .iter()
                .map(|f| match &f.work {
                    Work::Replay(_, index) => index.lock().unwrap().written(),
                    Work::Record(_) | Work::Channel(..) => 0,
                })
                .sum();
            return Ok(Self {
                at: Instant::now(),
                frames,
                played,
                late,
                dropped,
                bytes: vec![written],
            });
        }
        // Per output, every feed's files.
        let mut files: Vec<Vec<String>> = Vec::new();
        for feed in feeds {
            if let Work::Record(legs) = &feed.work {
                files.resize(legs.len(), Vec::new());
                for (list, leg) in files.iter_mut().zip(legs) {
                    list.extend(leg.files());
                }
            }
        }
        let at = Instant::now();
        let bytes = tokio::task::spawn_blocking(move || {
            files
                .iter()
                .map(|list| {
                    list.iter()
                        .filter_map(|f| std::fs::metadata(f).ok())
                        .map(|m| m.len())
                        .sum()
                })
                .collect()
        })
        .await?;
        Ok(Self {
            at,
            frames,
            played,
            late,
            dropped,
            bytes,
        })
    }
}

/// `run`'s step at `feeds`, from the counters `before` and `after` it and the
/// CPU sampled between.
pub(super) fn measure(
    run: &BenchmarkRunDto,
    feeds: u32,
    before: &Snapshot,
    after: &Snapshot,
    cpu: &[f64],
    memory_pct: f64,
) -> BenchmarkStepDto {
    let fps = run.media.fps_num as f64 / run.media.fps_den as f64;
    let outputs = Setup::of(run).measured_outputs();
    let secs = (after.at - before.at).as_secs_f64();
    let (produced, program_expected, program_shortfall) =
        shortfall(&before.frames, &after.frames, fps, secs);
    // A channel's clip: frames that didn't arrive, or came too late to show.
    let (_, clip_expected, clip_shortfall) = shortfall(&before.played, &after.played, fps, secs);
    let clip_shortfall = clip_shortfall + after.late.saturating_sub(before.late);
    let expected = program_expected + clip_expected;
    let source_shortfall = program_shortfall + clip_shortfall;
    let output_dropped = after.dropped.saturating_sub(before.dropped);
    let pct = |n: u64, of: u64| n as f64 / of.max(1) as f64 * 100.0;
    let source_pct =
        pct(program_shortfall, program_expected).max(pct(clip_shortfall, clip_expected));
    let output_pct = output_dropped as f64 / (produced * outputs as u64).max(1) as f64 * 100.0;
    let drop_pct = source_pct.max(output_pct);
    let per_sec = |bytes: u64| (bytes as f64 / secs.max(0.001)) as u64;
    let written: Vec<u64> = after
        .bytes
        .iter()
        .zip(&before.bytes)
        .map(|(a, b)| a.saturating_sub(*b))
        .collect();
    BenchmarkStepDto {
        feeds,
        secs,
        expected_frames: expected,
        source_shortfall,
        output_dropped,
        drop_pct,
        cpu_pct: if cpu.is_empty() {
            0.0
        } else {
            cpu.iter().sum::<f64>() / cpu.len() as f64
        },
        memory_pct,
        write_bytes_per_sec: per_sec(written.iter().sum()),
        output_bytes_per_sec: written.iter().map(|&b| per_sec(b) / feeds as u64).collect(),
        passed: drop_pct <= run.drop_threshold_pct,
        quick: false,
    }
}

/// Per feed: frames delivered against the time their timestamps span, less
/// loop gaps. A frame of slack covers one landing between the count and the
/// timestamp reads. Returns delivered, due, and missing.
fn shortfall(
    before: &[VideoProgress],
    after: &[VideoProgress],
    fps: f64,
    secs: f64,
) -> (u64, u64, u64) {
    let (mut produced, mut expected, mut missing) = (0, 0, 0);
    for (p0, p1) in before.iter().zip(after) {
        let delivered = p1.frames.saturating_sub(p0.frames);
        let span = match (p0.last_pts, p1.last_pts) {
            (Some(t0), Some(t1)) => {
                let skipped = p1.skipped.saturating_sub(p0.skipped);
                t1.saturating_sub(t0).saturating_sub(skipped).nseconds() as f64 / 1e9
            }
            _ => secs,
        };
        let should = (span * fps).round() as u64;
        produced += delivered;
        expected += should;
        missing += should.saturating_sub(delivered + 1);
    }
    (produced, expected, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{
        BenchmarkStatus, ChromaSubsampling, Container, MediaInfo, OutputAdvanced,
        PresetOutputInput, VideoCodec,
    };
    use gstreamer as gst;
    use std::time::Duration;

    fn secs(s: f64) -> gst::ClockTime {
        gst::ClockTime::from_nseconds((s * 1e9) as u64)
    }

    fn ts(frames: u64, at: f64) -> VideoProgress {
        VideoProgress {
            frames,
            last_pts: Some(secs(at)),
            skipped: gst::ClockTime::ZERO,
        }
    }

    /// A recording run at 30 fps with `outputs` outputs.
    fn run(outputs: usize) -> BenchmarkRunDto {
        let output = PresetOutputInput {
            name: "Main".into(),
            codec: VideoCodec::H264,
            container: Container::Mov,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: "~/{source}.{ext}".into(),
            preview: false,
            advanced: OutputAdvanced::default(),
        };
        BenchmarkRunDto {
            id: "run".into(),
            started_at: String::new(),
            finished_at: None,
            status: BenchmarkStatus::Running,
            preset_id: None,
            preset_name: None,
            outputs: vec![output; outputs],
            replay: None,
            channel: None,
            media_path: "/f.mov".into(),
            media: MediaInfo {
                width: 1920,
                height: 1080,
                fps_num: 30,
                fps_den: 1,
                audio_channels: 2,
                duration_ms: None,
            },
            max_feeds: 64,
            step_secs: 20,
            drop_threshold_pct: 0.5,
            feeds_running: 0,
            steps: Vec::new(),
            sustainable_feeds: 0,
            encoders: Vec::new(),
            message: None,
            scratch_dirs: Vec::new(),
        }
    }

    fn snapshot(
        at: Instant,
        frames: Vec<VideoProgress>,
        dropped: u64,
        bytes: Vec<u64>,
    ) -> Snapshot {
        Snapshot {
            at,
            frames,
            played: Vec::new(),
            late: 0,
            dropped,
            bytes,
        }
    }

    #[test]
    fn measures_a_step() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(500, 100.0), ts(500, 100.0)], 0, vec![0, 0]);
        // 2 feeds at 30 fps for 10 s: 600 frames expected, all delivered.
        let after = snapshot(
            t + Duration::from_secs(10),
            vec![ts(800, 110.0), ts(800, 110.0)],
            3,
            vec![20_000_000, 2_000_000],
        );
        let step = measure(&run(2), 2, &before, &after, &[50.0, 70.0], 40.0);
        assert_eq!(step.expected_frames, 600);
        assert_eq!(step.source_shortfall, 0);
        assert_eq!(step.output_dropped, 3);
        // 3 of 1200 encoded frames.
        assert!((step.drop_pct - 0.25).abs() < 1e-9);
        assert!(step.passed);
        assert_eq!(step.cpu_pct, 60.0);
        assert_eq!(step.write_bytes_per_sec, 2_200_000);
        assert_eq!(step.output_bytes_per_sec, [1_000_000, 100_000]);
    }

    #[test]
    fn timestamps_not_the_clock_decide_the_shortfall() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(0, 0.0)], 0, vec![0]);
        // The window ran 10 s by the clock, but the last frame counted is
        // from 9.9 s: 297 frames is everything that was due.
        let after = snapshot(t + Duration::from_secs(10), vec![ts(297, 9.9)], 0, vec![0]);
        let step = measure(&run(1), 1, &before, &after, &[], 0.0);
        assert_eq!(step.source_shortfall, 0);
        assert!(step.passed);
    }

    #[test]
    fn loop_gaps_are_not_a_shortfall() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(0, 0.0)], 0, vec![0]);
        // 10 s, but the footage's audio runs 0.9 s past its video, so each
        // loop leaves a gap: 273 frames is everything that was due.
        let after = VideoProgress {
            frames: 273,
            last_pts: Some(secs(10.0)),
            skipped: secs(0.9),
        };
        let after = snapshot(t + Duration::from_secs(10), vec![after], 0, vec![0]);
        let step = measure(&run(1), 1, &before, &after, &[], 0.0);
        assert_eq!(step.expected_frames, 273);
        assert_eq!(step.source_shortfall, 0);
    }

    #[test]
    fn source_shortfall_fails_a_step() {
        let t = Instant::now();
        let before = snapshot(t, vec![ts(0, 0.0); 4], 0, vec![0]);
        // Each of 4 feeds should deliver 300 frames; one delivered 250.
        let after = snapshot(
            t + Duration::from_secs(10),
            vec![ts(300, 10.0), ts(300, 10.0), ts(300, 10.0), ts(250, 10.0)],
            0,
            vec![0],
        );
        let step = measure(&run(1), 4, &before, &after, &[], 0.0);
        assert_eq!(step.source_shortfall, 49);
        assert!(!step.passed);
    }

    #[test]
    fn late_clip_frames_fail_a_channel_step() {
        let t = Instant::now();
        let mut before = snapshot(t, vec![ts(0, 0.0)], 0, vec![]);
        before.played = vec![ts(0, 0.1)];
        // The program kept up, and the clip delivered every frame, but 30 of
        // its 300 came too late to show.
        let mut after = snapshot(t + Duration::from_secs(10), vec![ts(300, 10.0)], 0, vec![]);
        after.played = vec![ts(300, 10.1)];
        after.late = 30;
        let mut channel = run(0);
        channel.channel = Some(Vec::new());
        let step = measure(&channel, 1, &before, &after, &[], 0.0);
        assert_eq!(step.expected_frames, 600);
        assert_eq!(step.source_shortfall, 30);
        // Against the clip's frames, not diluted by the program's.
        assert!((step.drop_pct - 10.0).abs() < 1e-9);
        assert!(!step.passed);
        assert!(step.output_bytes_per_sec.is_empty());
    }
}
