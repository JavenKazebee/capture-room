//! Instant replay prototype (ROADMAP #12): can GStreamer scrub a rolling
//! buffer of all-intra segments well enough to build replay on?
//!
//! For each codec it records a 1080p59.94 test feed into 1 s segments
//! (`splitmuxsink`), then plays them back through `splitmuxsrc` the way a
//! channel's player would (decode → appsink, paced by the clock) and measures:
//!
//! - encode speed (× real time) and data rate;
//! - accurate seek latency, and whether it lands on the requested frame;
//! - playback at 1×, 0.5×, 0.25×, -1× and -0.5× across segment boundaries:
//!   frames shown, skipped frames, frames late at the sink;
//! - frame stepping forwards and backwards;
//! - clip export by remuxing, cut on a frame, without re-encoding;
//!
//! and once, against a recorder that's still running, how fresh the
//! replayable buffer is.
//!
//! cargo run --release --example replay_spike -- [codec…] [--dir DIR] [--footage FILE]
//! cargo run --release --example replay_spike -- --reverse 'DIR/CODEC/seg*.mov' \
//!     [--decoder 'h264parse ! vtdec_hw' --decoder 'h264parse ! avdec_h264' …]
//!
//! A test pattern compresses far better than real pictures, so data rates
//! and decode costs only mean something with `--footage` (any video file;
//! two minutes in, 20 s of it is rendered at 1080p59.94 into DIR first).
//! Codecs: prores-hw prores-sw mjpeg h264-intra h264-gop (the last is the
//! long-GOP baseline the all-intra ones are compared against).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;

const FPS_N: u64 = 60000;
const FPS_D: u64 = 1001;
/// Footage recorded per codec.
const FRAMES: u64 = 600;
const SEGMENT: gst::ClockTime = gst::ClockTime::SECOND;

fn frame() -> gst::ClockTime {
    gst::ClockTime::from_nseconds(gst::ClockTime::SECOND.nseconds() * FPS_D / FPS_N)
}

fn frame_at(n: u64) -> gst::ClockTime {
    gst::ClockTime::from_nseconds(n * gst::ClockTime::SECOND.nseconds() * FPS_D / FPS_N)
}

fn frames(t: gst::ClockTime) -> f64 {
    t.nseconds() as f64 / frame().nseconds() as f64
}

struct Codec {
    name: &'static str,
    /// Raw video in, encoded out.
    encode: &'static str,
    /// The encoder's keyframes need marking (see [`recorder`]).
    mark_keyframes: bool,
    /// Encoded in, raw out. Software: VideoToolbox's decoder loses a frame
    /// now and then playing backwards (see `--reverse`).
    decode: &'static str,
}

const CODECS: &[Codec] = &[
    Codec {
        name: "prores-hw",
        encode: "videoconvert ! vtenc_prores ! video/x-prores,variant=proxy",
        mark_keyframes: true,
        decode: "avdec_prores",
    },
    Codec {
        name: "prores-sw",
        encode: "videoconvert ! avenc_prores_ks profile=proxy",
        mark_keyframes: false,
        decode: "avdec_prores",
    },
    Codec {
        name: "mjpeg",
        encode: "videoconvert ! video/x-raw,format=I420 ! jpegenc quality=85",
        mark_keyframes: false,
        decode: "jpegparse ! jpegdec",
    },
    Codec {
        name: "h264-intra",
        encode: "videoconvert ! vtenc_h264 realtime=true allow-frame-reordering=false \
                 max-keyframe-interval=1 bitrate=120000 ! h264parse",
        mark_keyframes: false,
        decode: "h264parse ! avdec_h264",
    },
    Codec {
        name: "h264-gop",
        encode: "videoconvert ! vtenc_h264 realtime=true allow-frame-reordering=false \
                 max-keyframe-interval=120 bitrate=20000 ! h264parse",
        mark_keyframes: false,
        decode: "h264parse ! avdec_h264",
    },
];

fn main() -> Result<()> {
    gst::init()?;
    let mut args = std::env::args().skip(1);
    let mut names = Vec::new();
    let mut root = std::env::temp_dir().join("replay-spike");
    let mut footage = None;
    let mut reverse = None;
    let mut decoders = Vec::new();
    while let Some(a) = args.next() {
        if a == "--dir" {
            root = PathBuf::from(args.next().context("--dir needs a path")?);
        } else if a == "--footage" {
            footage = Some(args.next().context("--footage needs a file")?);
        } else if a == "--reverse" {
            reverse = Some(args.next().context("--reverse needs a glob")?);
        } else if a == "--decoder" {
            decoders.push(args.next().context("--decoder needs a chain")?);
        } else {
            names.push(a);
        }
    }
    let footage = footage
        .map(|f| prepare(&f, &root).with_context(|| format!("prepare {f}")))
        .transpose()?;
    FOOTAGE.set(footage).unwrap();
    let codecs: Vec<&Codec> = if names.is_empty() {
        CODECS.iter().collect()
    } else {
        names
            .iter()
            .map(|n| {
                CODECS
                    .iter()
                    .find(|c| c.name == n)
                    .ok_or_else(|| anyhow!("unknown codec {n}"))
            })
            .collect::<Result<_>>()?
    };

    if let Some(location) = reverse {
        return reverse_by_decoder(&location, &decoders);
    }

    let base = source_speed()?;
    println!("test source alone: {base:.0} fps\n");

    for codec in &codecs {
        println!("━━ {} ━━", codec.name);
        let dir = root.join(codec.name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        if let Err(e) = run_codec(codec, &dir) {
            println!("  FAILED: {e:#}");
        }
        println!();
    }

    // Freshness against a live recorder, with the first all-intra codec.
    if let Some(codec) = codecs.iter().find(|c| c.name != "h264-gop") {
        println!("━━ growing buffer ({}) ━━", codec.name);
        let dir = root.join(format!("{}-live", codec.name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        if let Err(e) = growing(codec, &dir) {
            println!("  FAILED: {e:#}");
        }
    }
    Ok(())
}

fn run_codec(codec: &Codec, dir: &Path) -> Result<()> {
    let (fps, bytes, segments) = record(codec, dir)?;
    let secs = FRAMES as f64 * FPS_D as f64 / FPS_N as f64;
    println!(
        "  encode     {fps:.0} fps ({:.1}× real time), {:.0} Mbit/s, {segments} segments, \
         {:.1} GB/hour",
        fps * FPS_D as f64 / FPS_N as f64,
        bytes as f64 * 8.0 / secs / 1e6,
        bytes as f64 / secs * 3600.0 / 1e9,
    );

    let mut player = Player::new(&glob(dir), codec.decode)?;
    player.seeks()?;
    // Starts that cross segment boundaries (every 1 s) in either direction.
    for (rate, at) in [
        (1.0, 4.5),
        (0.5, 4.8),
        (0.25, 4.9),
        (-1.0, 6.5),
        (-0.5, 6.2),
    ] {
        player.play(rate, gst::ClockTime::from_mseconds((at * 1000.0) as u64))?;
    }
    player.steps()?;
    drop(player);

    export(dir)?;
    Ok(())
}

fn glob(dir: &Path) -> String {
    dir.join("seg*.mov").to_string_lossy().into_owned()
}

// ── Recording ─────────────────────────────────────────────────────────────────

/// The footage clip made by [`prepare`], recorded instead of a test pattern.
static FOOTAGE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
/// Where in the footage the clip starts, and how long it is.
const FOOTAGE_AT: gst::ClockTime = gst::ClockTime::from_seconds(120);
const FOOTAGE_LEN: gst::ClockTime = gst::ClockTime::from_seconds(20);

fn footage() -> Option<&'static Path> {
    FOOTAGE.get().and_then(|f| f.as_deref())
}

/// Render a clip of `file` at 1080p59.94 as high-quality MJPEG, once, so
/// every codec records the same pictures and recorders never need a seek
/// (a flushing seek through splitmuxsink spoils its first file, as through
/// any muxer).
fn prepare(file: &str, root: &Path) -> Result<PathBuf> {
    let out = root.join("footage.mov");
    if out.exists() {
        return Ok(out);
    }
    std::fs::create_dir_all(root)?;
    let p = launch(&format!(
        "uridecodebin uri=\"{}\" ! video/x-raw ! videoconvert name=first ! videoscale ! \
         videorate ! video/x-raw,width=1920,height=1080,framerate={FPS_N}/{FPS_D},\
         pixel-aspect-ratio=1/1 ! videoconvert ! video/x-raw,format=I420 ! \
         jpegenc quality=95 ! qtmux ! filesink async=false location=\"{}\"",
        gst::glib::filename_to_uri(file, None)?,
        out.display()
    ))?;
    let pad = p
        .by_name("first")
        .and_then(|e| e.static_pad("sink"))
        .context("first")?;
    gate(&pad, FOOTAGE_AT);
    settle(&p, gst::State::Paused)?;
    // The decoder's pads come late: seek once the chain has its input.
    let until = Instant::now() + Duration::from_secs(10);
    while pad.peer().is_none() || pad.current_caps().is_none() {
        if Instant::now() > until {
            bail!("{file}: no video");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    p.seek(
        1.0,
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::SeekType::Set,
        Some(FOOTAGE_AT),
        gst::SeekType::Set,
        Some(FOOTAGE_AT + FOOTAGE_LEN),
    )?;
    let result = run_to_eos(&p, Duration::from_secs(300));
    let _ = p.set_state(gst::State::Null);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out);
        return Err(e);
    }
    Ok(out)
}

/// Hold back everything flowing through `pad` until the segment of a seek
/// to `from`: buffers and segments before it, and flushes always. Lets a
/// muxer downstream take only what the seek asked for.
fn gate(pad: &gst::Pad, from: gst::ClockTime) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let open = AtomicBool::new(false);
    pad.add_probe(gst::PadProbeType::DATA_DOWNSTREAM, move |_, info| {
        match &info.data {
            Some(gst::PadProbeData::Event(e)) => match e.view() {
                gst::EventView::FlushStart(_) | gst::EventView::FlushStop(_) => {
                    gst::PadProbeReturn::Drop
                }
                gst::EventView::Segment(seg) => {
                    // Compared in stream time: splitmuxsrc's segments start
                    // at 1000 s plus the position.
                    let at = seg
                        .segment()
                        .downcast_ref::<gst::ClockTime>()
                        .and_then(|s| s.to_stream_time(s.start()?));
                    if at.is_some_and(|at| at + frame() > from && at < from + frame()) {
                        open.store(true, Ordering::SeqCst);
                        gst::PadProbeReturn::Ok
                    } else {
                        gst::PadProbeReturn::Drop
                    }
                }
                _ => gst::PadProbeReturn::Ok,
            },
            Some(gst::PadProbeData::Buffer(_)) if !open.load(Ordering::SeqCst) => {
                gst::PadProbeReturn::Drop
            }
            _ => gst::PadProbeReturn::Ok,
        }
    });
}

/// 1080p59.94 video: the footage clip or a test pattern, [`FRAMES`] of it
/// unless it's live. A live source runs in real time.
fn source(live: bool) -> Result<String> {
    let caps = format!(
        "video/x-raw,width=1920,height=1080,framerate={FPS_N}/{FPS_D},pixel-aspect-ratio=1/1"
    );
    Ok(match footage() {
        Some(path) => format!(
            "filesrc location=\"{}\" ! qtdemux ! jpegdec ! videoconvert ! {caps} ! \
             timeoverlay ! {}",
            path.display(),
            if live {
                "clocksync".to_string()
            } else {
                format!("identity eos-after={FRAMES}")
            },
        ),
        None => format!(
            "videotestsrc is-live={live} pattern=ball {} ! {caps} ! timeoverlay",
            if live {
                String::new()
            } else {
                format!("num-buffers={FRAMES}")
            }
        ),
    })
}

fn start(p: &gst::Pipeline) -> Result<()> {
    p.set_state(gst::State::Playing)?;
    Ok(())
}

fn source_speed() -> Result<f64> {
    let p = launch(&format!("{} ! fakesink", source(false)?))?;
    start(&p)?;
    let start = Instant::now();
    run_to_eos(&p, Duration::from_secs(120))?;
    Ok(FRAMES as f64 / start.elapsed().as_secs_f64())
}

fn recorder(codec: &Codec, dir: &Path, live: bool) -> Result<gst::Pipeline> {
    let p = launch(&format!(
        "{} ! {} ! splitmuxsink name=mux muxer-factory=qtmux max-size-time={} \
         location={}/seg%05d.mov",
        source(live)?,
        codec.encode,
        SEGMENT.nseconds(),
        dir.display()
    ))?;
    if codec.mark_keyframes {
        // vtenc_prores flags its frames as deltas, so splitmuxsink never
        // finds a place to split. Every ProRes frame is a keyframe.
        let enc = p
            .iterate_recurse()
            .into_iter()
            .flatten()
            .find(|e| e.factory().is_some_and(|f| f.name() == "vtenc_prores"))
            .context("no vtenc_prores")?;
        enc.static_pad("src").context("encoder src")?.add_probe(
            gst::PadProbeType::BUFFER,
            |_, info| {
                if let Some(gst::PadProbeData::Buffer(buffer)) = &mut info.data {
                    buffer.make_mut().unset_flags(gst::BufferFlags::DELTA_UNIT);
                }
                gst::PadProbeReturn::Ok
            },
        );
    }
    Ok(p)
}

/// Record [`FRAMES`] as fast as they encode. Returns fps, bytes, segments.
fn record(codec: &Codec, dir: &Path) -> Result<(f64, u64, usize)> {
    let p = recorder(codec, dir, false)?;
    let begun = Instant::now();
    start(&p)?;
    run_to_eos(&p, Duration::from_secs(300))?;
    let fps = FRAMES as f64 / begun.elapsed().as_secs_f64();
    let files: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    let bytes = files
        .iter()
        .map(|f| f.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    Ok((fps, bytes, files.len()))
}

// ── Playback ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Shown {
    /// Position in the footage.
    stream: gst::ClockTime,
    /// How long after its due time it reached the sink.
    late: gst::ClockTime,
}

/// splitmuxsrc → decodebin → videoconvert → appsink (sync), like a
/// channel's player.
struct Player {
    pipeline: gst::Pipeline,
    /// Encoded frames reaching the decoder (stream time), to tell a frame
    /// the demuxer never sent from one the decoder lost.
    demuxed: Arc<Mutex<Vec<gst::ClockTime>>>,
    sink: gst_app::AppSink,
    shown: Arc<Mutex<Vec<Shown>>>,
    duration: gst::ClockTime,
}

impl Player {
    fn new(location: &str, decode: &str) -> Result<Self> {
        Self::wrap(launch(&format!(
            "splitmuxsrc location=\"{location}\" ! {}",
            Self::tail(decode)
        ))?)
    }

    /// Everything after the source: `decode` and the sink.
    fn tail(decode: &str) -> String {
        format!(
            "identity name=dec ! {decode} ! videoconvert ! \
             appsink name=sink sync=true max-buffers=2 caps=video/x-raw"
        )
    }

    fn wrap(pipeline: gst::Pipeline) -> Result<Self> {
        let sink = pipeline
            .by_name("sink")
            .context("sink")?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("not an appsink"))?;
        let shown = Arc::new(Mutex::new(Vec::new()));
        let shown_ref = shown.clone();
        let demuxed = Arc::new(Mutex::new(Vec::new()));
        if let Some(pad) = pipeline.by_name("dec").and_then(|d| d.static_pad("sink")) {
            let demuxed = demuxed.clone();
            let segment = Mutex::new(None::<gst::FormattedSegment<gst::ClockTime>>);
            pad.add_probe(
                gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
                move |_, info| {
                    match &info.data {
                        Some(gst::PadProbeData::Event(e)) => {
                            if let gst::EventView::Segment(seg) = e.view() {
                                *segment.lock().unwrap() =
                                    seg.segment().downcast_ref::<gst::ClockTime>().cloned();
                            }
                        }
                        Some(gst::PadProbeData::Buffer(b)) => {
                            if let (Some(seg), Some(pts)) = (&*segment.lock().unwrap(), b.pts()) {
                                if let Some(t) = seg.to_stream_time(pts) {
                                    demuxed.lock().unwrap().push(t);
                                }
                            }
                        }
                        _ => {}
                    }
                    gst::PadProbeReturn::Ok
                },
            );
        }
        let weak = pipeline.downgrade();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let (Some(p), Some(pts), Some(seg)) = (
                        weak.upgrade(),
                        sample.buffer().and_then(|b| b.pts()),
                        sample
                            .segment()
                            .and_then(|s| s.downcast_ref::<gst::ClockTime>()),
                    ) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let (Some(stream), Some(rt), Some(clock), Some(base)) = (
                        seg.to_stream_time(pts),
                        seg.to_running_time(pts),
                        p.clock(),
                        p.base_time(),
                    ) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let now = clock.time().unwrap_or_default().saturating_sub(base);
                    shown_ref.lock().unwrap().push(Shown {
                        stream,
                        late: now.saturating_sub(rt),
                    });
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let duration = settle(&pipeline, gst::State::Paused).and_then(|_| {
            pipeline
                .query_duration::<gst::ClockTime>()
                .context("no duration")
        });
        let duration = match duration {
            Ok(d) => d,
            Err(e) => {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(e);
            }
        };
        Ok(Self {
            pipeline,
            demuxed,
            sink,
            shown,
            duration,
        })
    }

    /// The prerolled frame's position.
    fn position(&self) -> Result<gst::ClockTime> {
        let sample = self.sink.pull_preroll().context("no preroll")?;
        let pts = sample.buffer().and_then(|b| b.pts()).context("no pts")?;
        sample
            .segment()
            .and_then(|s| s.downcast_ref::<gst::ClockTime>())
            .and_then(|s| s.to_stream_time(pts))
            .context("no stream time")
    }

    /// A flushing, accurate seek while paused. For a reverse rate, playback
    /// runs backwards from `at`.
    fn seek(&self, rate: f64, at: gst::ClockTime) -> Result<Duration> {
        let start = Instant::now();
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
        if rate > 0.0 {
            self.pipeline.seek(
                rate,
                flags,
                gst::SeekType::Set,
                Some(at),
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )?;
        } else {
            self.pipeline.seek(
                rate,
                flags,
                gst::SeekType::Set,
                Some(gst::ClockTime::ZERO),
                gst::SeekType::Set,
                Some(at),
            )?;
        }
        wait_async(&self.pipeline)?;
        Ok(start.elapsed())
    }

    fn seeks(&mut self) -> Result<()> {
        let n = 20;
        let total = (self.duration.nseconds() / frame().nseconds()).saturating_sub(60);
        let mut times = Vec::new();
        let mut off = 0;
        // Spread over the footage, on frame boundaries, not in order.
        for i in 0..n {
            let target = frame_at(30 + (i * 7919) % total);
            times.push(self.seek(1.0, target)?);
            let got = self.position()?;
            if got != target {
                off += 1;
            }
        }
        times.sort();
        println!(
            "  seek       median {} ms, max {} ms, {off}/{n} off the requested frame",
            times[n as usize / 2].as_millis(),
            times.last().unwrap().as_millis(),
        );
        Ok(())
    }

    /// Play at `rate` from `at` for two seconds of wall time.
    fn play(&mut self, rate: f64, at: gst::ClockTime) -> Result<()> {
        let seek = self.seek(rate, at)?;
        self.shown.lock().unwrap().clear();
        self.demuxed.lock().unwrap().clear();
        self.pipeline.set_state(gst::State::Playing)?;
        let wall = Duration::from_secs(2);
        let err = bus_error(&self.pipeline, wall);
        settle(&self.pipeline, gst::State::Paused)?;
        if let Some(e) = err {
            println!("  {rate:>5}×     ERROR {e}");
            return Ok(());
        }
        let shown = std::mem::take(&mut *self.shown.lock().unwrap());
        let expected = wall.as_secs_f64() * FPS_N as f64 / FPS_D as f64 * rate.abs();
        let mut wrong_way = 0;
        let mut skipped = 0.0;
        let mut skips = Vec::new();
        for w in shown.windows(2) {
            let step = frames(w[1].stream) - frames(w[0].stream);
            let step = if rate > 0.0 { step } else { -step };
            if step <= 0.0 {
                wrong_way += 1;
            } else if step > 1.5 {
                skipped += step.round() - 1.0;
                skips.push(w[0].stream);
            }
        }
        let late = shown.iter().filter(|s| s.late > frame()).count();
        let max_late = shown.iter().map(|s| s.late).max().unwrap_or_default();
        let (first, last) = (
            shown.first().map(|s| s.stream),
            shown.last().map(|s| s.stream),
        );
        println!(
            "  {rate:>5}×     {} frames (expect ~{expected:.0}), {skipped:.0} skipped, \
             {wrong_way} out of order, {late} late (max {} ms), {:.3}s → {:.3}s, seek {} ms",
            shown.len(),
            max_late.mseconds(),
            first.map_or(0.0, |t| t.seconds_f64()),
            last.map_or(0.0, |t| t.seconds_f64()),
            seek.as_millis(),
        );
        if !skips.is_empty() {
            let demuxed = self.demuxed.lock().unwrap();
            for skip in &skips {
                // The frame after `skip` in the direction of play.
                let missing = if rate > 0.0 {
                    *skip + frame()
                } else {
                    skip.saturating_sub(frame())
                };
                let sent = demuxed
                    .iter()
                    .filter(|t| t.nseconds().abs_diff(missing.nseconds()) < frame().nseconds() / 2)
                    .count();
                println!(
                    "             frame at {:.3}s: demuxer sent it {sent}×",
                    missing.seconds_f64()
                );
            }
            let at: Vec<_> = skips
                .iter()
                .map(|t| format!("{:.3}s", t.seconds_f64()))
                .collect();
            println!("             skipped after {}", at.join(", "));
        }
        Ok(())
    }

    /// Step one frame at a time from a paused position: forwards, then
    /// (after a reverse seek) backwards.
    fn steps(&mut self) -> Result<()> {
        let n = 30;
        let at = frame_at(300);
        for (rate, label) in [(1.0, "step fwd"), (-1.0, "step back")] {
            let switch = self.seek(rate, at)?;
            let mut prev = self.position()?;
            let mut times = Vec::new();
            let mut wrong = 0;
            for _ in 0..n {
                let start = Instant::now();
                self.pipeline.send_event(gst::event::Step::new(
                    gst::format::Buffers::from_u64(1),
                    1.0,
                    true,
                    false,
                ));
                wait_for(&self.pipeline, gst::MessageType::StepDone)?;
                settle(&self.pipeline, gst::State::Paused)?;
                times.push(start.elapsed());
                let now = self.position()?;
                let moved = (frames(now) - frames(prev)) * rate;
                if (moved - 1.0).abs() > 0.5 {
                    wrong += 1;
                }
                prev = now;
            }
            times.sort();
            println!(
                "  {label:<10} median {} ms, max {} ms, {wrong}/{n} not exactly one frame, \
                 direction change {} ms, ended at {:.3}s",
                times[n / 2].as_millis(),
                times.last().unwrap().as_millis(),
                switch.as_millis(),
                prev.seconds_f64(),
            );
        }
        Ok(())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// Play the segments at `location` backwards a dozen times through each
/// decoder chain, counting frames lost. (VideoToolbox's decoder loses one
/// now and then; software decoders don't.)
fn reverse_by_decoder(location: &str, decoders: &[String]) -> Result<()> {
    let default = ["decodebin".to_string()];
    let decoders = if decoders.is_empty() {
        &default[..]
    } else {
        decoders
    };
    for chain in decoders {
        let mut player = Player::new(location, chain)?;
        println!("━━ {chain} ━━");
        let end = player.duration.seconds();
        if end < 4 {
            bail!("{location}: too short");
        }
        for i in 0..12 {
            player.play(-1.0, gst::ClockTime::from_seconds(end - 1 - i % (end - 3)))?;
        }
    }
    Ok(())
}

// ── Export ────────────────────────────────────────────────────────────────────

/// Remux frames [in, out) into one file, without decoding.
fn export(dir: &Path) -> Result<()> {
    let out = dir.join("export.mov");
    // Frames 195 → 449 (3.25 s → 7.5 s). Recorded timestamps are rounded
    // down to the ns, so a frame can start just before its exact time: cut
    // half a frame early at both ends.
    let half = frame() / 2;
    let (from, to) = (frame_at(195) - half, frame_at(450) - half);
    let want = 255;
    let p = gst::Pipeline::new();
    let src = gst::ElementFactory::make("splitmuxsrc")
        .property("location", glob(dir))
        .build()?;
    let mux = gst::ElementFactory::make("qtmux").build()?;
    // Not prerolled: nothing reaches it until the seek has been made.
    let file = gst::ElementFactory::make("filesink")
        .property("location", out.to_string_lossy().as_ref())
        .property("async", false)
        .build()?;
    p.add_many([&src, &mux, &file])?;
    mux.link(&file)?;
    let mux_weak = mux.downgrade();
    src.connect_pad_added(move |_, pad| {
        let Some(mux) = mux_weak.upgrade() else {
            return;
        };
        gate(pad, from);
        if let Some(sink) = mux.request_pad_simple("video_%u") {
            let _ = pad.link(&sink);
        }
    });
    settle(&p, gst::State::Paused)?;
    let start = Instant::now();
    p.seek(
        1.0,
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::SeekType::Set,
        Some(from),
        gst::SeekType::Set,
        Some(to),
    )?;
    let result = run_to_eos(&p, Duration::from_secs(60));
    let _ = p.set_state(gst::State::Null);
    result?;
    let took = start.elapsed();

    // The exported frames must be the source's frames [from, to), byte for
    // byte.
    let exported = frame_hashes(&format!(
        "filesrc location=\"{}\" ! qtdemux ! fakesink name=sink signal-handoffs=true",
        out.display()
    ))?;
    let source: Vec<_> = frame_hashes(&format!(
        "splitmuxsrc location=\"{}\" ! fakesink name=sink signal-handoffs=true",
        glob(dir)
    ))?
    .into_iter()
    .skip(195)
    .take(want)
    .collect();
    let exact = exported.iter().map(|e| e.1).eq(source.iter().map(|s| s.1));
    // …and decode on their own: a cut that starts after a keyframe doesn't.
    let decoded = frame_hashes(&format!(
        "filesrc location=\"{}\" ! qtdemux ! decodebin ! fakesink name=sink signal-handoffs=true",
        out.display()
    ))?
    .len();
    println!(
        "  export     {} frames (want {want}), {}, {decoded} decode, {} ms, {:.1} MB",
        exported.len(),
        if exact {
            "identical to the source's"
        } else {
            "NOT the source's frames"
        },
        took.as_millis(),
        std::fs::metadata(&out)?.len() as f64 / 1e6,
    );
    Ok(())
}

/// Each encoded frame's pts and a hash of its bytes, in order.
fn frame_hashes(desc: &str) -> Result<Vec<(Option<gst::ClockTime>, u64)>> {
    use std::hash::{Hash, Hasher};
    let p = launch(desc)?;
    let frames = Arc::new(Mutex::new(Vec::new()));
    let frames_ref = frames.clone();
    p.by_name("sink")
        .context("sink")?
        .connect("handoff", false, move |args| {
            let buffer = args[1].get::<gst::Buffer>().ok()?;
            let map = buffer.map_readable().ok()?;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            map.as_slice().hash(&mut h);
            frames_ref.lock().unwrap().push((buffer.pts(), h.finish()));
            None
        });
    run_to_eos(&p, Duration::from_secs(60))?;
    let _ = p.set_state(gst::State::Null);
    let frames = std::mem::take(&mut *frames.lock().unwrap());
    Ok(frames)
}

// ── Growing buffer ────────────────────────────────────────────────────────────

/// Record live, and open the buffer while it's still being written: once
/// by glob (which includes the segment being written), then by the list of
/// closed segments, as a replay node would.
fn growing(codec: &Codec, dir: &Path) -> Result<()> {
    let rec = recorder(codec, dir, true)?;
    let closed = Arc::new(Mutex::new(Vec::<(String, Instant)>::new()));
    let closed_ref = closed.clone();
    let bus = rec.bus().context("bus")?;
    start(&rec)?;
    let started = Instant::now();
    // splitmuxsink-fragment-closed tells which files are complete.
    let collect = |until: Instant| -> Result<()> {
        while Instant::now() < until {
            let left = until.saturating_duration_since(Instant::now());
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_nseconds(left.as_nanos() as u64))
            else {
                break;
            };
            match msg.view() {
                gst::MessageView::Element(e) => {
                    let Some(s) = e.structure() else { continue };
                    if s.name() == "splitmuxsink-fragment-closed" {
                        if let Ok(loc) = s.get::<String>("location") {
                            closed_ref.lock().unwrap().push((loc, Instant::now()));
                        }
                    }
                }
                gst::MessageView::Error(e) => bail!("recorder: {}", e.error()),
                _ => {}
            }
        }
        Ok(())
    };
    collect(started + Duration::from_secs(6))?;

    match Player::new(&glob(dir), codec.decode) {
        Ok(p) => println!(
            "  by glob    opened, {:.2}s long while recording",
            p.duration.seconds_f64()
        ),
        Err(e) => println!("  by glob    fails while recording: {e:#}"),
    }

    let list: Vec<String> = closed.lock().unwrap().iter().map(|c| c.0.clone()).collect();
    if list.is_empty() {
        bail!("no segment closed in 6 s");
    }
    let newest = closed.lock().unwrap().last().map(|c| c.1);
    let opened = Instant::now();
    let player = by_list(list.clone(), codec.decode)?;
    let open_ms = opened.elapsed().as_millis();
    let recorded = started.elapsed().as_secs_f64();
    println!(
        "  by list    {} closed segments, {:.2}s replayable of {recorded:.2}s recorded \
         (newest closed {} ms ago), opened in {open_ms} ms",
        list.len(),
        player.duration.seconds_f64(),
        newest.map_or(0, |t| t.elapsed().as_millis()),
    );
    // Does an open player see segments closed after it opened?
    collect(Instant::now() + Duration::from_secs(3))?;
    let after = player
        .pipeline
        .query_duration::<gst::ClockTime>()
        .unwrap_or_default();
    println!(
        "  3 s later  same player reports {:.2}s; {} segments closed now",
        after.seconds_f64(),
        closed.lock().unwrap().len(),
    );
    // Play the newest second backwards while the recorder carries on.
    let mut player = player;
    player.play(-1.0, player.duration.saturating_sub(frame()))?;
    drop(player);
    let _ = rec.send_event(gst::event::Eos::new());
    let _ = run_to_eos(&rec, Duration::from_secs(10));
    let _ = rec.set_state(gst::State::Null);
    Ok(())
}

/// A player over exactly these files.
fn by_list(files: Vec<String>, decode: &str) -> Result<Player> {
    let pipeline = launch(&format!("splitmuxsrc name=src ! {}", Player::tail(decode)))?;
    let src = pipeline.by_name("src").context("src")?;
    src.connect("format-location", false, move |_| Some(files.to_value()));
    Player::wrap(pipeline)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn launch(desc: &str) -> Result<gst::Pipeline> {
    gst::parse::launch(desc)
        .with_context(|| desc.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("not a pipeline"))
}

fn run_to_eos(p: &gst::Pipeline, timeout: Duration) -> Result<()> {
    p.set_state(gst::State::Playing)?;
    let bus = p.bus().context("bus")?;
    let msg = bus
        .timed_pop_filtered(
            gst::ClockTime::from_nseconds(timeout.as_nanos() as u64),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .context("timed out")?;
    let result = match msg.view() {
        gst::MessageView::Error(e) => Err(anyhow!("{} ({:?})", e.error(), e.debug())),
        _ => Ok(()),
    };
    p.set_state(gst::State::Null)?;
    result
}

/// Change state and wait for it to finish (a sink may have to preroll).
fn settle(p: &gst::Pipeline, state: gst::State) -> Result<()> {
    p.set_state(state)?;
    let (result, now, _) = p.state(gst::ClockTime::from_seconds(10));
    result.map_err(|_| anyhow!("{state:?} failed: {:?}", bus_error(p, Duration::ZERO)))?;
    if now != state {
        bail!("stuck in {now:?} on the way to {state:?}");
    }
    Ok(())
}

fn wait_async(p: &gst::Pipeline) -> Result<()> {
    wait_for(p, gst::MessageType::AsyncDone)
}

fn wait_for(p: &gst::Pipeline, kind: gst::MessageType) -> Result<()> {
    let bus = p.bus().context("bus")?;
    let msg = bus
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(10),
            &[kind, gst::MessageType::Error],
        )
        .with_context(|| format!("timed out waiting for {kind:?}"))?;
    if let gst::MessageView::Error(e) = msg.view() {
        bail!("{} ({:?})", e.error(), e.debug());
    }
    Ok(())
}

/// Wait `wall`, returning the first error the pipeline posts meanwhile.
/// (Polled: `timed_pop_filtered` restarts its timeout on every message it
/// discards.)
fn bus_error(p: &gst::Pipeline, wall: Duration) -> Option<String> {
    let bus = p.bus()?;
    let until = Instant::now() + wall;
    loop {
        while let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(e) = msg.view() {
                return Some(format!("{} ({:?})", e.error(), e.debug()));
            }
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
