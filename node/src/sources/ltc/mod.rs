//! LTC: timecode read from one audio channel of a live source, so files from
//! machines on one house clock line up.
//!
//! ```text
//! compositor → [source timecode] → [LTC timecode] → out caps → video
//!                                        ↑ the last LTC frame
//! mixer → out caps → tee ──────────────────────────────────→ audio
//!                     └→ queue → fakesink: the channel → Decoder
//! ```
//!
//! The decoder ([`decode`]) is ours rather than `timecodestamper`'s libltc:
//! the official Windows GStreamer builds libltc with `-D__BIG_ENDIAN__`
//! (cerbero's meson patch, for lack of `endian.h`), so it reads the user bits
//! as the timecode, and Homebrew's has no libltc at all.
//!
//! Each decoded frame is kept with the running time it started at. A video
//! frame with no timecode of its own (a Decklink's RP188 wins) gets the last
//! LTC frame's, counted on by the frames between them, as RP188 is carried
//! through the compositor. That's exact however late the LTC is decoded, so
//! video waits for nothing. Within [`LOCK_WINDOW`] of the last LTC frame it's
//! locked; past that it freewheels, as a camera does, until the bin is
//! rebuilt. With no LTC since then, frames carry none and a recording gives
//! them the time of day.

mod decode;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_video as gst_video;

use self::decode::{Decoder, LtcFrame};
use super::live::{count_on, AUDIO_RATE};
use crate::api::types::LtcState;
use crate::pipeline::timecode::{drop_frame_flags, running_time};
use crate::pipeline::{make_el, set_property};

/// A frame this close to the last LTC frame counts as locked: a few LTC
/// frames, as each is decoded only once it ends and audio may arrive late.
const LOCK_WINDOW: Duration = Duration::from_millis(250);

/// Check a source's LTC channel against its `channels` of audio (`None`
/// for silence).
pub fn validate(channel: Option<u32>, channels: Option<u32>) -> Result<(), String> {
    let Some(channel) = channel else {
        return Ok(());
    };
    let channels = channels.ok_or("LTC needs audio: pick the source's or a device's")?;
    if channel == 0 || channel > channels {
        return Err(format!(
            "LTC channel {channel} isn't one of the {channels} audio channels"
        ));
    }
    Ok(())
}

/// How a source's LTC is doing, and the timecode its frames last left with.
/// Read by the 1 Hz `feed.status` emitter.
#[derive(Default)]
pub struct LtcTracker {
    state: AtomicU8,
    timecode: Mutex<Option<String>>,
}

impl LtcTracker {
    fn reset(&self) {
        self.state.store(0, Ordering::Relaxed);
        *self.timecode.lock().unwrap() = None;
    }

    fn frame(&self, state: LtcState, tc: Option<String>) {
        let n = match state {
            LtcState::Missing => 0,
            LtcState::Locked => 1,
            LtcState::Lost => 2,
            LtcState::SourceTimecode => 3,
        };
        self.state.store(n, Ordering::Relaxed);
        *self.timecode.lock().unwrap() = tc;
    }

    pub fn state(&self) -> LtcState {
        match self.state.load(Ordering::Relaxed) {
            1 => LtcState::Locked,
            2 => LtcState::Lost,
            3 => LtcState::SourceTimecode,
            _ => LtcState::Missing,
        }
    }

    /// `HH:MM:SS:FF` of the newest frame, if it had timecode.
    pub fn timecode(&self) -> Option<String> {
        self.timecode.lock().unwrap().clone()
    }
}

/// The last LTC frame decoded, at the running time it started.
type LastLtc = Arc<Mutex<Option<(gst::ClockTime, LtcFrame)>>>;

/// Decode `channel` (1-based) of the bin's audio and stamp the frames leaving
/// `video` (the compositor's src pad) with it.
pub(crate) fn attach(
    bin: &gst::Bin,
    id: &str,
    video: &gst::Pad,
    channel: u32,
    tracker: Arc<LtcTracker>,
) -> Result<()> {
    tracker.reset();
    let last = LastLtc::default();
    stamp(video, last.clone(), tracker);

    // A tee behind the audio ghost pad.
    let ghost = bin
        .static_pad("audio")
        .and_then(|p| p.downcast::<gst::GhostPad>().ok())
        .context("audio ghost pad")?;
    let out = ghost.target().context("audio ghost target")?;
    ghost
        .set_target(None::<&gst::Pad>)
        .context("unset audio ghost target")?;
    let tee = make_el("tee", &format!("live-ltc-tee-{id}"))?;
    let queue = make_el("queue", &format!("live-ltc-queue-{id}"))?;
    // The program audio never waits on the decoder.
    set_property(&queue, "leaky", "downstream")?;
    queue.set_property("max-size-buffers", 0u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", gst::ClockTime::SECOND.nseconds());
    let sink = make_el("fakesink", &format!("live-ltc-sink-{id}"))?;
    sink.set_property("sync", false);
    sink.set_property("async", false);
    bin.add_many([&tee, &queue, &sink])
        .context("add LTC branch")?;
    out.link(&tee.static_pad("sink").context("tee sink pad")?)
        .context("link audio to LTC tee")?;
    let program = tee.request_pad_simple("src_%u").context("tee pad")?;
    ghost
        .set_target(Some(&program))
        .context("retarget audio ghost pad")?;
    gst::Element::link_many([&tee, &queue, &sink]).context("link LTC branch")?;
    decode_into(
        &sink.static_pad("sink").context("LTC sink pad")?,
        channel,
        last,
    );
    Ok(())
}

/// Decode the LTC in `channel` (1-based) of the interleaved F32 audio
/// reaching `pad`, keeping the last frame.
fn decode_into(pad: &gst::Pad, channel: u32, last: LastLtc) {
    let decoder = Mutex::new(Decoder::default());
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(buffer) = info.buffer() else {
            return gst::PadProbeReturn::Ok;
        };
        let channels = pad
            .current_caps()
            .and_then(|c| c.structure(0)?.get::<i32>("channels").ok())
            .unwrap_or(1)
            .max(1) as usize;
        let Some(rt) = buffer.pts().and_then(|pts| running_time(pad, pts)) else {
            return gst::PadProbeReturn::Ok;
        };
        let Ok(map) = buffer.map_readable() else {
            return gst::PadProbeReturn::Ok;
        };
        let mut decoder = decoder.lock().unwrap();
        if buffer.flags().contains(gst::BufferFlags::DISCONT) {
            *decoder = Decoder::default();
        }
        let base = decoder.position();
        let pick = (channel as usize - 1).min(channels - 1);
        for sample in map.as_chunks::<4>().0.iter().skip(pick).step_by(channels) {
            if let Some(frame) = decoder.push(f32::from_le_bytes(*sample)) {
                // Its first bit may have been in an earlier buffer.
                let before = (base as i64 - frame.start as i64) * 1_000_000_000 / AUDIO_RATE as i64;
                let at = rt.nseconds() as i64 - before;
                if at >= 0 {
                    *last.lock().unwrap() = Some((gst::ClockTime::from_nseconds(at as u64), frame));
                }
            }
        }
        gst::PadProbeReturn::Ok
    });
}

/// Stamp frames leaving `pad` with no timecode of their own from the last
/// LTC frame, and keep the tracker current. After `carry_source_timecode`'s
/// probe on the same pad.
fn stamp(pad: &gst::Pad, last: LastLtc, tracker: Arc<LtcTracker>) {
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(buffer) = info.buffer_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        if let Some(meta) = buffer.meta::<gst_video::VideoTimeCodeMeta>() {
            tracker.frame(LtcState::SourceTimecode, Some(meta.tc().to_string()));
            return gst::PadProbeReturn::Ok;
        }
        let rt = buffer.pts().and_then(|pts| running_time(pad, pts));
        let fps = pad
            .current_caps()
            .and_then(|c| c.structure(0)?.get::<gst::Fraction>("framerate").ok());
        let Some((at, frame)) = *last.lock().unwrap() else {
            tracker.frame(LtcState::Missing, None);
            return gst::PadProbeReturn::Ok;
        };
        let tc = rt.zip(fps).and_then(|(rt, fps)| {
            let tc = timecode(&frame, fps)?;
            Some((rt, count_on(&tc, at, rt, fps)?))
        });
        match tc {
            Some((rt, tc)) => {
                let since = rt.nseconds().abs_diff(at.nseconds());
                let state = if since <= LOCK_WINDOW.as_nanos() as u64 {
                    LtcState::Locked
                } else {
                    LtcState::Lost
                };
                gst_video::VideoTimeCodeMeta::add(buffer.make_mut(), &tc);
                tracker.frame(state, Some(tc.to_string()));
            }
            None => tracker.frame(LtcState::Missing, None),
        }
        gst::PadProbeReturn::Ok
    });
}

/// `frame` as timecode at its own rate, given the video's `fps`.
fn timecode(frame: &LtcFrame, video: gst::Fraction) -> Option<gst_video::ValidVideoTimeCode> {
    let fps = ltc_rate(frame, video);
    let flags = if frame.drop_frame {
        drop_frame_flags(fps)
    } else {
        gst_video::VideoTimeCodeFlags::empty()
    };
    gst_video::ValidVideoTimeCode::new(
        fps,
        None,
        flags,
        frame.hours,
        frame.minutes,
        frame.seconds,
        frame.frames,
        0,
    )
    .ok()
}

/// The LTC's frame rate: 24, 25 or 30 by its length, taken as the video's
/// (or half the video's, for 50 and 60) when they match, so 29.97 and 23.976
/// keep their exact rate.
fn ltc_rate(frame: &LtcFrame, video: gst::Fraction) -> gst::Fraction {
    let measured = AUDIO_RATE as f64 / frame.samples.max(1) as f64;
    let base = [24, 25, 30]
        .into_iter()
        .min_by(|a, b| {
            (*a as f64 - measured)
                .abs()
                .total_cmp(&(*b as f64 - measured).abs())
        })
        .unwrap_or(25);
    let nominal = |f: gst::Fraction| (f.numer() as f64 / f.denom().max(1) as f64).round() as i32;
    if nominal(video) == base {
        video
    } else if nominal(video) == 2 * base {
        gst::Fraction::new(video.numer(), video.denom() * 2)
    } else if base == 30 && frame.drop_frame {
        gst::Fraction::new(30000, 1001)
    } else {
        gst::Fraction::new(base, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_checks_the_channel() {
        assert!(validate(None, None).is_ok());
        assert!(validate(Some(2), Some(2)).is_ok());
        assert!(validate(Some(3), Some(2)).is_err());
        assert!(validate(Some(0), Some(2)).is_err());
        assert!(validate(Some(1), None).is_err());
    }

    fn frame(samples: u32, drop_frame: bool) -> LtcFrame {
        LtcFrame {
            hours: 1,
            minutes: 2,
            seconds: 3,
            frames: 4,
            drop_frame,
            start: 0,
            samples,
        }
    }

    #[test]
    fn ltc_rate_follows_the_video() {
        let f = gst::Fraction::new;
        assert_eq!(ltc_rate(&frame(1920, false), f(25, 1)), f(25, 1));
        assert_eq!(ltc_rate(&frame(1920, false), f(50, 1)), f(25, 1));
        assert_eq!(ltc_rate(&frame(1602, true), f(30000, 1001)), f(30000, 1001));
        assert_eq!(ltc_rate(&frame(1602, true), f(60000, 1001)), f(30000, 1001));
        assert_eq!(
            ltc_rate(&frame(2002, false), f(24000, 1001)),
            f(24000, 1001)
        );
        // Another rate than the video's: its own.
        assert_eq!(ltc_rate(&frame(1600, false), f(25, 1)), f(30, 1));
        assert_eq!(ltc_rate(&frame(1602, true), f(25, 1)), f(30000, 1001));
    }

    #[test]
    fn timecode_keeps_drop_frame() {
        gst::init().unwrap();
        let tc = timecode(&frame(1602, true), gst::Fraction::new(30000, 1001)).unwrap();
        assert_eq!(tc.to_string(), "01:02:03;04");
        let tc = timecode(&frame(1920, false), gst::Fraction::new(25, 1)).unwrap();
        assert_eq!(tc.to_string(), "01:02:03:04");
    }

    /// What a live bin's video frames carried: each frame's timecode (`-`
    /// for none), and the tracker's state at the end.
    struct Run {
        frames: Vec<String>,
        /// Each frame's running time, in ms.
        times: Vec<u64>,
        state: LtcState,
    }

    /// A direct live bin over test video at 25 fps (with its own timecode
    /// from 20:00:00:00 if `own`) and stereo audio whose channel 2 is LTC from
    /// 10:00:00:00 for `ltc_frames` frames, then silence, for `secs`.
    fn run(own: bool, ltc_frames: u32, secs: u64) -> Run {
        use gstreamer_app as gst_app;
        use std::time::Instant;

        use crate::sources::live::{build_bin, LinkTracker, LiveAudio, LiveInput};

        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        let stamp = if own {
            " ! timecodestamper name=own set=always"
        } else {
            ""
        };
        let capture = gst::parse::bin_from_description(
            &format!(
                "videotestsrc is-live=1 ! video/x-raw,width=320,height=180,framerate=25/1 \
                 ! identity name=v{stamp} \
                 appsrc name=a is-live=1 format=time \
                 caps=audio/x-raw,format=F32LE,rate=48000,channels=2,layout=interleaved"
            ),
            false,
        )
        .unwrap();
        if own {
            let start = gst_video::VideoTimeCode::new(
                gst::Fraction::new(25, 1),
                None,
                gst_video::VideoTimeCodeFlags::empty(),
                20,
                0,
                0,
                0,
                0,
            );
            capture
                .by_name("own")
                .unwrap()
                .set_property("set-internal-timecode", &start);
        }
        let link = Arc::new(LinkTracker::default());
        let ltc = Arc::new(LtcTracker::default());
        let bin = build_bin(LiveInput {
            id: "t",
            source: capture.clone().upcast(),
            audio: LiveAudio::Source { channels: 2 },
            format: None,
            tracker: &link,
            direct: true,
            latency: None,
            ltc: Some((2, &ltc)),
        })
        .unwrap();
        let vlast = capture.by_name(if own { "own" } else { "v" }).unwrap();
        let appsrc = capture.by_name("a").unwrap();
        for (el, name) in [(&vlast, "video"), (&appsrc, "audio")] {
            let target = el.static_pad("src").unwrap();
            let ghost = gst::GhostPad::builder_with_target(&target)
                .unwrap()
                .name(name)
                .build();
            capture.add_pad(&ghost).unwrap();
        }

        let pipeline = gst::Pipeline::new();
        let frames = Arc::new(Mutex::new(Vec::<(u64, String)>::new()));
        let frames_ = frames.clone();
        let vsink = gst_app::AppSink::builder()
            .sync(false)
            .callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |s| {
                        let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                        let tc = buffer
                            .meta::<gst_video::VideoTimeCodeMeta>()
                            .map_or("-".into(), |m| m.tc().to_string());
                        let rt = sample
                            .segment()
                            .and_then(|s| s.downcast_ref::<gst::ClockTime>())
                            .zip(buffer.pts())
                            .and_then(|(s, pts)| s.to_running_time(pts))
                            .map_or(0, |t| t.mseconds());
                        frames_.lock().unwrap().push((rt, tc));
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            )
            .build();
        let asink = make_el("fakesink", "asink").unwrap();
        pipeline
            .add_many([bin.upcast_ref(), vsink.upcast_ref(), &asink])
            .unwrap();
        bin.link_pads(Some("video"), &vsink, None).unwrap();
        bin.link_pads(Some("audio"), &asink, None).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();

        // Audio in real time, a frame (40 ms) a buffer.
        let ltc_samples = decode::encode([10, 0, 0, 0], 25, false, 1920.0, ltc_frames, 0.5);
        let appsrc = appsrc.downcast::<gst_app::AppSrc>().unwrap();
        let started = Instant::now();
        let per = 1920usize;
        for n in 0..(secs * 25) as usize {
            let mut data = Vec::with_capacity(per * 8);
            for i in 0..per {
                let ltc = ltc_samples.get(n * per + i).copied().unwrap_or(0.0);
                data.extend_from_slice(&0f32.to_le_bytes());
                data.extend_from_slice(&ltc.to_le_bytes());
            }
            let mut buffer = gst::Buffer::from_mut_slice(data);
            {
                let b = buffer.get_mut().unwrap();
                b.set_pts(gst::ClockTime::from_mseconds(40 * n as u64));
                b.set_duration(gst::ClockTime::from_mseconds(40));
            }
            if appsrc.push_buffer(buffer).is_err() {
                break;
            }
            let next = Duration::from_millis(40 * (n as u64 + 1));
            if let Some(wait) = next.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        // While audio still flows: once it stops, so does the LTC.
        let state = ltc.state();
        std::thread::sleep(Duration::from_millis(300));
        pipeline.set_state(gst::State::Null).unwrap();
        let (times, frames): (Vec<_>, Vec<_>) = frames.lock().unwrap().iter().cloned().unzip();
        println!("{} frames: {:?}", frames.len(), frames);
        Run {
            frames,
            times,
            state,
        }
    }

    /// Frame numbers of `HH:MM:SS:FF` timecodes at 25 fps.
    fn frame_number(t: &str) -> u32 {
        let n: Vec<u32> = t.split([':', ';']).map(|p| p.parse().unwrap()).collect();
        ((n[0] * 60 + n[1]) * 60 + n[2]) * 25 + n[3]
    }

    fn assert_counts(frames: &[&String]) {
        for pair in frames.windows(2) {
            assert_eq!(
                frame_number(pair[1]),
                frame_number(pair[0]) + 1,
                "{frames:?}"
            );
        }
    }

    /// Frames carry the LTC on channel 2, counting frame by frame.
    #[test]
    fn frames_carry_ltc() {
        let run = run(false, 200, 3);
        assert_eq!(run.state, LtcState::Locked);
        assert!(
            run.frames
                .iter()
                .all(|t| t == "-" || t.starts_with("10:00:0")),
            "no zeroes or other timecode: {:?}",
            run.frames
        );
        let stamped: Vec<_> = run.frames.iter().skip_while(|t| *t == "-").collect();
        assert!(stamped.len() >= 40, "{:?}", run.frames);
        assert_counts(&stamped);
        // The frame at 40 ms × n carries the LTC frame sent then.
        let start = frame_number("10:00:00:00");
        for (t, tc) in run
            .times
            .iter()
            .zip(&run.frames)
            .filter(|(_, tc)| *tc != "-")
        {
            assert_eq!(
                (frame_number(tc) - start) as u64,
                (t + 20) / 40,
                "frame at {t} ms has {tc}"
            );
        }
    }

    /// Without LTC frames carry no timecode: a recording gives them the time
    /// of day.
    #[test]
    fn no_ltc_leaves_frames_unstamped() {
        let run = run(false, 0, 2);
        assert_eq!(run.state, LtcState::Missing);
        assert!(run.frames.len() >= 30, "{:?}", run.frames);
        assert!(run.frames.iter().all(|t| t == "-"), "{:?}", run.frames);
    }

    /// LTC that stops: frames count on from the last of it.
    #[test]
    fn lost_ltc_counts_on() {
        let run = run(false, 40, 4);
        assert_eq!(run.state, LtcState::Lost);
        let stamped: Vec<_> = run.frames.iter().skip_while(|t| *t == "-").collect();
        assert!(stamped.len() >= 70, "{:?}", run.frames);
        assert!(
            stamped.iter().all(|t| t.starts_with("10:00:0")),
            "{stamped:?}"
        );
        assert_counts(&stamped);
    }

    /// A frame's own timecode wins over LTC.
    #[test]
    fn own_timecode_wins() {
        let run = run(true, 100, 3);
        assert_eq!(run.state, LtcState::SourceTimecode);
        let stamped: Vec<_> = run.frames.iter().skip_while(|t| *t == "-").collect();
        assert!(stamped.len() >= 40, "{:?}", run.frames);
        assert!(
            stamped.iter().all(|t| t.starts_with("20:00:0")),
            "{stamped:?}"
        );
    }
}
