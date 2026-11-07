//! LTC: timecode read from one audio channel of a live source, so files from
//! machines on one house clock line up.
//!
//! ```text
//! compositor → [source timecode] → timecodestamper → out caps → video
//!                                       ↑ ltc_sink
//! mixer → out caps → tee ──────────────────────────────────────→ audio
//!                     └→ queue → audioconvert (the channel, U8 mono)
//! ```
//!
//! A frame's own timecode (a Decklink's RP188) wins over LTC (the stamper
//! keeps it), and LTC over the node's clock: a frame leaving without any gets
//! the time of day in a recording, as before.
//!
//! `timecodestamper` stamps `00:00:00:00` on frames while it has no LTC. Its
//! LTC timecodes all carry the daily jam set here and that one none, so it's
//! removed. Its own count-on is cut short ([`LTC_TIMEOUT`]) so that a frame
//! it stamped means LTC is being read; past that, the frames here count on
//! from the last LTC instead, as a camera freewheels, until the bin is
//! rebuilt. The stamper holds video for `ltc-extra-latency` (150 ms) to
//! decode the LTC alongside it, and reports that as latency.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_video as gst_video;

use super::live::{count_on, AUDIO_RATE};
use crate::api::types::LtcState;
use crate::pipeline::timecode::running_time;
use crate::pipeline::{capsfilter, make_el, set_mix_matrix, set_property};

/// After this long without new LTC the stamper stops stamping: longer than
/// an LTC frame, so a steady signal never trips it.
const LTC_TIMEOUT: Duration = Duration::from_millis(250);

/// Whether this node's `timecodestamper` was built with libltc. Its
/// `ltc_sink` template is there either way (Homebrew's has none); only
/// asking for the pad tells.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let Ok(el) = gst::ElementFactory::make("timecodestamper").build() else {
            return false;
        };
        match el.request_pad_simple("ltc_sink") {
            Some(pad) => {
                el.release_request_pad(&pad);
                true
            }
            None => false,
        }
    })
}

/// Check a source's LTC channel against its `channels` of audio (`None`
/// for silence).
pub fn validate(channel: Option<u32>, channels: Option<u32>) -> Result<(), String> {
    validate_with(channel, channels, available())
}

fn validate_with(
    channel: Option<u32>,
    channels: Option<u32>,
    available: bool,
) -> Result<(), String> {
    let Some(channel) = channel else {
        return Ok(());
    };
    if !available {
        return Err("this node's GStreamer can't read LTC (timecodestamper without libltc)".into());
    }
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

/// The stamper, for the bin's video chain; [`attach`] feeds it.
pub(crate) fn stamper(id: &str) -> Result<gst::Element> {
    let el = make_el("timecodestamper", &format!("live-ltc-{id}"))?;
    set_property(&el, "source", "ltc")?;
    set_property(&el, "set", "keep")?;
    el.set_property("ltc-timeout", LTC_TIMEOUT.as_nanos() as u64);
    el.set_property("ltc-daily-jam", daily_jam()?);
    Ok(el)
}

/// Today's local midnight: only there to tell the stamper's LTC timecodes
/// from its zeroes.
fn daily_jam() -> Result<glib::DateTime> {
    let now = glib::DateTime::now_local().context("local time")?;
    glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.0)
        .context("local midnight")
}

/// Feed `stamper` (already in the bin's video chain) `channel` (1-based) of
/// the bin's `channels` of audio, and track what its frames come out with.
pub(crate) fn attach(
    bin: &gst::Bin,
    id: &str,
    stamper: &gst::Element,
    channel: u32,
    channels: u32,
    tracker: Arc<LtcTracker>,
) -> Result<()> {
    tracker.reset();
    watch(stamper, tracker)?;

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
    let pick = make_el("audioconvert", &format!("live-ltc-pick-{id}"))?;
    set_mix_matrix(&pick, &[pick_row(channel, channels)]);
    set_property(&pick, "dithering", "none")?;
    let caps = capsfilter(
        &format!("live-ltc-caps-{id}"),
        gst::Caps::builder("audio/x-raw")
            .field("format", "U8")
            .field("rate", AUDIO_RATE)
            .field("channels", 1)
            .field("layout", "interleaved")
            .build(),
    )?;
    bin.add_many([&tee, &queue, &pick, &caps])
        .context("add LTC branch")?;
    out.link(&tee.static_pad("sink").context("tee sink pad")?)
        .context("link audio to LTC tee")?;
    let program = tee.request_pad_simple("src_%u").context("tee pad")?;
    ghost
        .set_target(Some(&program))
        .context("retarget audio ghost pad")?;
    gst::Element::link_many([&tee, &queue, &pick, &caps]).context("link LTC branch")?;
    let ltc_sink = stamper
        .request_pad_simple("ltc_sink")
        .context("timecodestamper has no LTC input (built without libltc)")?;
    caps.static_pad("src")
        .context("LTC caps src pad")?
        .link(&ltc_sink)
        .context("link LTC audio")?;
    Ok(())
}

/// `audioconvert` mix-matrix row taking only `channel` (1-based) of `channels`.
fn pick_row(channel: u32, channels: u32) -> Vec<f32> {
    (1..=channels.max(1))
        .map(|c| if c == channel { 1.0 } else { 0.0 })
        .collect()
}

/// Around the stamper: drop its zeroes, count on from the last LTC once it
/// stops, and keep the tracker current. Both probes run on the video
/// streaming thread, one buffer at a time.
fn watch(stamper: &gst::Element, tracker: Arc<LtcTracker>) -> Result<()> {
    let had_own = Arc::new(AtomicBool::new(false));
    let had_own_ = had_own.clone();
    stamper
        .static_pad("sink")
        .context("stamper sink pad")?
        .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            let own = info
                .buffer()
                .is_some_and(|b| b.meta::<gst_video::VideoTimeCodeMeta>().is_some());
            had_own_.store(own, Ordering::Relaxed);
            gst::PadProbeReturn::Ok
        });

    // The last frame stamped from LTC, at its running time.
    let last = Mutex::new(None::<(gst::ClockTime, gst_video::ValidVideoTimeCode)>);
    stamper
        .static_pad("src")
        .context("stamper src pad")?
        .add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer_mut() else {
                return gst::PadProbeReturn::Ok;
            };
            let rt = buffer.pts().and_then(|pts| running_time(pad, pts));
            let stamped = buffer
                .meta::<gst_video::VideoTimeCodeMeta>()
                .map(|m| m.tc());
            if had_own.load(Ordering::Relaxed) {
                tracker.frame(LtcState::SourceTimecode, stamped.map(|tc| tc.to_string()));
                return gst::PadProbeReturn::Ok;
            }
            match stamped {
                Some(tc) if tc.latest_daily_jam().is_some() => {
                    tracker.frame(LtcState::Locked, Some(tc.to_string()));
                    *last.lock().unwrap() = rt.map(|rt| (rt, tc));
                }
                zero => {
                    let buffer = buffer.make_mut();
                    if zero.is_some() {
                        if let Some(meta) = buffer.meta_mut::<gst_video::VideoTimeCodeMeta>() {
                            let _ = meta.remove();
                        }
                    }
                    let fps = pad
                        .current_caps()
                        .and_then(|c| c.structure(0)?.get::<gst::Fraction>("framerate").ok());
                    let counted = last
                        .lock()
                        .unwrap()
                        .as_ref()
                        .zip(rt.zip(fps))
                        .and_then(|((at, tc), (rt, fps))| count_on(tc, *at, rt, fps));
                    match counted {
                        Some(tc) => {
                            gst_video::VideoTimeCodeMeta::add(buffer, &tc);
                            tracker.frame(LtcState::Lost, Some(tc.to_string()));
                        }
                        None => tracker.frame(LtcState::Missing, None),
                    }
                }
            }
            gst::PadProbeReturn::Ok
        });
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn validate_checks_the_channel() {
        assert!(validate_with(None, None, false).is_ok());
        assert!(validate_with(Some(2), Some(2), true).is_ok());
        assert!(validate_with(Some(3), Some(2), true).is_err());
        assert!(validate_with(Some(0), Some(2), true).is_err());
        assert!(validate_with(Some(1), None, true).is_err());
        assert!(validate_with(Some(1), Some(2), false).is_err());
    }

    #[test]
    fn pick_row_takes_one_channel() {
        assert_eq!(pick_row(2, 2), vec![0.0, 1.0]);
        assert_eq!(pick_row(1, 1), vec![1.0]);
        assert_eq!(pick_row(16, 16)[15], 1.0);
    }

    /// CI's GStreamer on each platform (the one releases ship) reads LTC;
    /// a developer's may not (Homebrew's has no libltc).
    #[test]
    fn timecodestamper_reads_ltc() {
        gst::init().unwrap();
        if std::env::var_os("CI").is_some() {
            assert!(available(), "timecodestamper was built without libltc");
        } else if !available() {
            println!("this GStreamer's timecodestamper can't read LTC (no libltc)");
        }
    }

    /// LTC audio (SMPTE 12M: 80 bits a frame, biphase mark) at 48 kHz, as
    /// `f32` samples, for frames counted from `start` (`[h, m, s, f]`).
    pub(crate) fn ltc_audio(start: [u32; 4], fps: u32, frames: u32) -> Vec<f32> {
        let samples_per_frame = AUDIO_RATE as u32 / fps;
        let mut out = Vec::with_capacity((samples_per_frame * frames) as usize);
        let mut level = 0.5f32;
        let first = start[0] * 3600 * fps + start[1] * 60 * fps + start[2] * fps + start[3];
        for total in first..first + frames {
            let f = total % fps;
            let s = total / fps % 60;
            let m = total / fps / 60 % 60;
            let h = total / fps / 3600 % 24;
            let bits = ltc_bits(h, m, s, f);
            // Each bit is two half-cells; its edges sit at sample boundaries
            // spread over the frame.
            for (i, bit) in bits.iter().enumerate() {
                let at = |half: u32| (samples_per_frame * (2 * i as u32 + half) / 160) as usize;
                let (a, b, c) = (at(0), at(1), at(2));
                level = -level;
                out.extend(std::iter::repeat_n(level, b - a));
                if *bit {
                    level = -level;
                }
                out.extend(std::iter::repeat_n(level, c - b));
            }
        }
        out
    }

    fn ltc_bits(h: u32, m: u32, s: u32, f: u32) -> [bool; 80] {
        let mut bits = [false; 80];
        let mut put = |at: usize, len: usize, v: u32| {
            for i in 0..len {
                bits[at + i] = (v >> i) & 1 == 1;
            }
        };
        put(0, 4, f % 10);
        put(8, 2, f / 10);
        put(16, 4, s % 10);
        put(24, 3, s / 10);
        put(32, 4, m % 10);
        put(40, 3, m / 10);
        put(48, 4, h % 10);
        put(56, 2, h / 10);
        // Sync word 0011 1111 1111 1101.
        put(64, 16, 0b1011_1111_1111_1100);
        bits
    }

    /// What a live bin's video frames carried: each frame's timecode (`-`
    /// for none), and the tracker's state at the end.
    struct Run {
        frames: Vec<String>,
        state: LtcState,
    }

    /// A direct live bin over test video at 25 fps (with its own timecode
    /// from 20:00:00:00 if `own`) and stereo audio whose channel 2 is LTC from
    /// 10:00:00:00 for `ltc_frames` frames, then silence, for `secs`.
    fn run(own: bool, ltc_frames: u32, secs: u64) -> Option<Run> {
        use gstreamer_app as gst_app;
        use std::time::Instant;

        use crate::sources::live::{build_bin, LinkTracker, LiveAudio, LiveInput};

        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        if !available() {
            println!("no LTC in this GStreamer: not running the bin");
            return None;
        }
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
        let frames = Arc::new(Mutex::new(Vec::<String>::new()));
        let frames_ = frames.clone();
        let vsink = gst_app::AppSink::builder()
            .sync(false)
            .callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |s| {
                        let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let tc = sample
                            .buffer()
                            .and_then(|b| b.meta::<gst_video::VideoTimeCodeMeta>())
                            .map_or("-".into(), |m| m.tc().to_string());
                        frames_.lock().unwrap().push(tc);
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
        let ltc_samples = ltc_audio([10, 0, 0, 0], 25, ltc_frames);
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
        // The stamper holds frames back a little.
        std::thread::sleep(Duration::from_millis(500));
        let state = ltc.state();
        pipeline.set_state(gst::State::Null).unwrap();
        let frames = frames.lock().unwrap().clone();
        println!("{} frames: {:?}", frames.len(), frames);
        Some(Run { frames, state })
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
        let Some(run) = run(false, 200, 3) else {
            return;
        };
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
    }

    /// Without LTC frames carry no timecode (a recording gives them the time
    /// of day), not the stamper's zeroes.
    #[test]
    fn no_ltc_leaves_frames_unstamped() {
        let Some(run) = run(false, 0, 2) else {
            return;
        };
        assert_eq!(run.state, LtcState::Missing);
        assert!(run.frames.len() >= 30, "{:?}", run.frames);
        assert!(run.frames.iter().all(|t| t == "-"), "{:?}", run.frames);
    }

    /// LTC that stops: frames count on from the last of it.
    #[test]
    fn lost_ltc_counts_on() {
        let Some(run) = run(false, 40, 4) else {
            return;
        };
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
        let Some(run) = run(true, 100, 3) else {
            return;
        };
        assert_eq!(run.state, LtcState::SourceTimecode);
        let stamped: Vec<_> = run.frames.iter().skip_while(|t| *t == "-").collect();
        assert!(stamped.len() >= 40, "{:?}", run.frames);
        assert!(
            stamped.iter().all(|t| t.starts_with("20:00:0")),
            "{stamped:?}"
        );
    }
}
