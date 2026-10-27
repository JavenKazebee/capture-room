//! Live external sources (streams, devices, WHIP). Each runs through
//! `fallbacksrc`, which plays black and silence while the source is missing
//! or stalled and keeps retrying, so a dropout never stops the monitor or its
//! recordings. Both streams are normalized after it (video held to the first
//! live format or a configured one, audio to 48 kHz at a fixed channel count)
//! so a recording never sees the format change.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_video as gst_video;
use tracing::{info, warn};

use super::add_chain;
use crate::api::types::{LinkState, VideoFormat};
use crate::pipeline::timecode::{drop_frame_flags, running_time};
use crate::pipeline::{capsfilter, make_el, set_mix_matrix, set_property};

/// Rate every live source's audio is resampled to.
pub const AUDIO_RATE: i32 = 48_000;
/// Rate used when the source doesn't say (or reports a variable rate).
const DEFAULT_FPS: (i32, i32) = (30, 1);
/// How long the source may stall before the fallback (black) takes over. The
/// compositor repeats the last frame meanwhile, so recordings see no gap.
const SWITCH_TIMEOUT: Duration = Duration::from_millis(2000);
/// How long a stalled source runs before it's restarted.
const RESTART_TIMEOUT: Duration = Duration::from_secs(5);
/// Latency every live source reports, fallback or not: a stream's jitter
/// buffer (`latency_ms`, 200 by default) plus decoding.
const MIN_LATENCY: Duration = Duration::from_secs(1);
/// A source's timecode is carried this far past its last frame that had one,
/// counting on; after that the output gets none (a recording falls back to
/// time of day).
const TIMECODE_HOLD: Duration = Duration::from_secs(1);
/// Without a real frame for this long the source counts as down. HLS arrives
/// a segment at a time, in bursts.
const LIVE_WINDOW: Duration = Duration::from_secs(3);

/// Whether a live source is delivering real frames, not the fallback's.
/// Read by the 1 Hz `feed.status` emitter.
#[derive(Default)]
pub struct LinkTracker {
    inner: Mutex<Link>,
}

#[derive(Default)]
struct Link {
    last_frame: Option<Instant>,
    /// The source has delivered a frame since the bin was built.
    seen: bool,
}

impl LinkTracker {
    fn reset(&self) {
        *self.inner.lock().unwrap() = Link::default();
    }

    fn frame(&self) {
        let mut link = self.inner.lock().unwrap();
        link.last_frame = Some(Instant::now());
        link.seen = true;
    }

    /// `passive` sources wait for a sender to connect (listeners, WHIP)
    /// rather than connecting out.
    pub fn state(&self, passive: bool) -> LinkState {
        let link = self.inner.lock().unwrap();
        match link.last_frame {
            Some(t) if t.elapsed() < LIVE_WINDOW => LinkState::Live,
            _ if passive => LinkState::Waiting,
            _ if link.seen => LinkState::Reconnecting,
            _ => LinkState::Connecting,
        }
    }
}

/// Where a live source's audio comes from, resolved for building.
pub enum LiveAudio {
    /// The source's own audio (silence if it has none), at `channels`.
    Source {
        channels: u32,
    },
    /// Another element's audio (a device): the picked `channels` (1-based,
    /// in output order) out of its `inputs`.
    Element {
        element: gst::Element,
        inputs: u32,
        channels: Vec<u32>,
    },
    Silence {
        channels: u32,
    },
}

pub struct LiveInput<'a> {
    pub id: &'a str,
    /// An element with raw video (and maybe audio) src pads, static or dynamic.
    pub source: gst::Element,
    pub audio: LiveAudio,
    /// Hold video to this size and rate instead of the source's first.
    pub format: Option<&'a VideoFormat>,
    pub tracker: &'a Arc<LinkTracker>,
    /// Link the source's pads straight in, not through `fallbacksrc`: for a
    /// server that comes and goes with its publisher (WHIP) and mustn't be
    /// restarted. A black background keeps video running while it has no
    /// pads; its EOSes are dropped so the next publisher's pads link in.
    pub direct: bool,
    /// Extra time the compositor and mixer wait for late input, beyond what
    /// the source reports (a `fallbacksrc` reports [`MIN_LATENCY`] already).
    pub latency: Option<Duration>,
}

/// Build a source bin (`"video"` / `"audio"` ghost pads) around `input`.
///
/// ```text
/// source → [fallbacksrc] → input chain ─┐
///             black (direct sources only) ┴→ compositor → locked caps → video
/// source → [fallbacksrc] → input chain ─┐
///                         silence (always) ┴→ audiomixer → 48 kHz F32 → audio
/// ```
///
/// The compositor and mixer run on the clock, repeating the last frame or
/// playing silence when the source is late, so a recording's input queue
/// never gets a catch-up burst to drop (nor audio overlaps from jitter).
pub fn build_bin(input: LiveInput) -> Result<gst::Bin> {
    let LiveInput {
        id,
        source,
        audio,
        format,
        tracker,
        direct,
        latency,
    } = input;
    tracker.reset();
    let bin = gst::Bin::with_name(&format!("live-bin-{id}"));

    // The element whose `video` / `audio` pads feed the input chains.
    let main = if direct {
        source.clone()
    } else {
        let own_audio = matches!(audio, LiveAudio::Source { .. });
        fallback(&format!("live-fb-{id}"), true, own_audio)?
    };
    bin.add(&main).context("add live source")?;

    let (video, video_format) = video_output(&bin, id, initial_video_caps(format))?;
    let compositor = video.aggregator.upgrade().context("compositor")?;
    let channels = match &audio {
        LiveAudio::Source { channels } | LiveAudio::Silence { channels } => *channels,
        LiveAudio::Element { channels, .. } => channels.len() as u32,
    };
    // Picked device channels are numbered; a source's own are laid out.
    let numbered = matches!(audio, LiveAudio::Element { .. });
    let audio_inputs = audio_output(&bin, id, channels, numbered)?;
    if let Some(latency) = latency {
        let mixer = audio_inputs.aggregator().context("audio mixer")?;
        for agg in [&compositor, &mixer] {
            agg.set_property("latency", latency.as_nanos() as u64);
        }
    }

    watch_source(&source, tracker.clone(), format.copied(), move |lock| {
        video_format.lock().unwrap().lock(lock);
    });
    if direct {
        // Keeps the compositor producing frames while nobody publishes.
        add_background(&bin, id, &compositor)?;
        let own_audio = matches!(audio, LiveAudio::Source { .. });
        link_sessions(&main, video, own_audio.then_some(audio_inputs.clone()));
    } else {
        main.set_property("source", &source);
        let chain = video.add("main")?;
        link_when_added(&main, "video", chain);
    }
    match audio {
        LiveAudio::Source { .. } if !direct => {
            let chain = audio_inputs.add("src")?;
            link_when_added(&main, "audio", chain);
        }
        LiveAudio::Element {
            element,
            inputs,
            channels,
        } => {
            let fb = fallback(&format!("live-afb-{id}"), false, true)?;
            fb.set_property("source", &element);
            bin.add(&fb).context("add audio device")?;
            let picker = Inputs {
                kind: Kind::Audio(
                    audio_inputs.audio_caps(),
                    Some(pick_matrix(inputs, &channels)),
                ),
                ..audio_inputs
            };
            let chain = picker.add("dev")?;
            link_when_added(&fb, "audio", chain);
        }
        _ => {}
    }

    Ok(bin)
}

/// The bin's video output: a compositor over black, held to `caps`. Inputs
/// are added through the returned [`Inputs`].
pub(crate) fn video_output(
    bin: &gst::Bin,
    id: &str,
    caps: gst::Caps,
) -> Result<(Inputs, Arc<Mutex<InputCaps>>)> {
    let compositor = make_el("compositor", &format!("live-vsync-{id}"))?;
    set_property(&compositor, "background", "black")?;
    let timecode = LastTimecode::default();
    carry_source_timecode(
        &compositor.static_pad("src").context("compositor src pad")?,
        timecode.clone(),
    );
    let out_vcaps = capsfilter(&format!("live-outvcaps-{id}"), caps.clone())?;
    add_chain(bin, &[&compositor, &out_vcaps], "video")?;
    let format = Arc::new(Mutex::new(InputCaps {
        pre: pre_compositor_caps(&caps),
        inputs: Vec::new(),
        out: out_vcaps.downgrade(),
    }));
    let inputs = Inputs {
        bin: bin.downgrade(),
        id: id.to_string(),
        aggregator: compositor.downgrade(),
        kind: Kind::Video(format.clone(), timecode),
    };
    Ok((inputs, format))
}

/// The bin's audio output: a mixer over silence at 48 kHz F32 with `channels`.
pub(crate) fn audio_output(
    bin: &gst::Bin,
    id: &str,
    channels: u32,
    numbered: bool,
) -> Result<Inputs> {
    let audio_caps = audio_caps(channels, numbered);
    let mixer = make_el("audiomixer", &format!("live-amix-{id}"))?;
    let out_acaps = capsfilter(&format!("live-acaps-{id}"), audio_caps.clone())?;
    let silence = make_el("audiotestsrc", &format!("live-silence-{id}"))?;
    silence.set_property("is-live", true);
    set_property(&silence, "wave", "silence")?;
    let silence_caps = capsfilter(&format!("live-scaps-{id}"), audio_caps.clone())?;
    add_chain(bin, &[&silence, &silence_caps, &mixer, &out_acaps], "audio")?;
    Ok(Inputs {
        bin: bin.downgrade(),
        id: id.to_string(),
        aggregator: mixer.downgrade(),
        kind: Kind::Audio(audio_caps, None),
    })
}

/// Audio at `channels`. Beyond stereo the mixer needs a layout: `numbered`
/// (picked) channels have none; others get GStreamer's usual layout so stereo
/// is spread onto it and outputs can mix it down.
fn audio_caps(channels: u32, numbered: bool) -> gst::Caps {
    let channels = channels.max(1);
    let mut caps = gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("rate", AUDIO_RATE)
        .field("channels", channels as i32)
        .field("layout", "interleaved");
    if channels > 2 {
        let mask = if numbered {
            0
        } else {
            channel_layout(channels)
        };
        caps = caps.field("channel-mask", gst::Bitmask::new(mask));
    }
    caps.build()
}

/// GStreamer's fallback channel mask (as `gst_audio_channel_get_fallback_mask`);
/// numbered (0) beyond 8.
pub(crate) fn channel_layout(channels: u32) -> u64 {
    const FL: u64 = 1 << 0;
    const FR: u64 = 1 << 1;
    const FC: u64 = 1 << 2;
    const LFE: u64 = 1 << 3;
    const RL: u64 = 1 << 4;
    const RR: u64 = 1 << 5;
    const RC: u64 = 1 << 8;
    const SL: u64 = 1 << 10;
    const SR: u64 = 1 << 11;
    match channels {
        2 => FL | FR,
        3 => FL | FR | LFE,
        4 => FL | FR | RL | RR,
        5 => FL | FR | FC | RL | RR,
        6 => FL | FR | FC | LFE | RL | RR,
        7 => FL | FR | FC | LFE | RL | RR | RC,
        8 => FL | FR | FC | LFE | RL | RR | SL | SR,
        _ => 0,
    }
}

/// The video format inputs are converted to, and who has to know when it's
/// locked.
pub(crate) struct InputCaps {
    /// Caps for each input's converters (see [`pre_compositor_caps`]).
    pre: gst::Caps,
    inputs: Vec<glib::WeakRef<gst::Element>>,
    out: glib::WeakRef<gst::Element>,
}

impl InputCaps {
    pub(crate) fn lock(&mut self, lock: &gst::Caps) {
        // Before the compositor, only what the converters can always produce:
        // stricter caps reach fallbacksrc's black source, which fails to negotiate.
        self.pre = pre_compositor_caps(lock);
        self.inputs.retain(|w| w.upgrade().is_some());
        for el in self.inputs.iter().filter_map(|w| w.upgrade()) {
            el.set_property("caps", &self.pre);
        }
        if let Some(el) = self.out.upgrade() {
            el.set_property("caps", lock);
        }
    }
}

/// The newest input frame's timecode, at its running time.
type LastTimecode = Arc<Mutex<Option<(gst::ClockTime, gst_video::ValidVideoTimeCode)>>>;

#[derive(Clone)]
enum Kind {
    /// The format inputs are converted to, and their timecode.
    Video(Arc<Mutex<InputCaps>>, LastTimecode),
    /// The mixer's caps, and a channel-picking matrix.
    Audio(gst::Caps, Option<Vec<Vec<f32>>>),
}

/// Makes input chains into one of the bin's aggregators.
#[derive(Clone)]
pub(crate) struct Inputs {
    bin: glib::WeakRef<gst::Bin>,
    id: String,
    aggregator: glib::WeakRef<gst::Element>,
    kind: Kind,
}

pub(crate) struct Chain {
    elements: Vec<gst::Element>,
    pad: gst::Pad,
}

impl Chain {
    pub(crate) fn sink(&self) -> Option<gst::Pad> {
        self.elements.first().and_then(|e| e.static_pad("sink"))
    }

    /// Put `head` (already in the bin and linked to [`Chain::sink`]) at the
    /// front, so it's removed with the chain.
    pub(crate) fn prepend(&mut self, head: gst::Element) {
        self.elements.insert(0, head);
    }
}

impl Inputs {
    pub(crate) fn bin(&self) -> Option<gst::Bin> {
        self.bin.upgrade()
    }

    pub(crate) fn aggregator(&self) -> Option<gst::Element> {
        self.aggregator.upgrade()
    }

    pub(crate) fn audio_caps(&self) -> gst::Caps {
        match &self.kind {
            Kind::Audio(caps, _) => caps.clone(),
            Kind::Video(..) => gst::Caps::new_empty_simple("audio/x-raw"),
        }
    }

    /// Add a chain converting into the aggregator's format; `tag` is unique per bin.
    pub(crate) fn add(&self, tag: &str) -> Result<Chain> {
        let bin = self.bin.upgrade().context("live bin gone")?;
        let aggregator = self.aggregator.upgrade().context("aggregator gone")?;
        let id = &self.id;
        let name = |el: &str| format!("live-{tag}-{el}-{id}");
        let elements = match &self.kind {
            Kind::Video(format, timecode) => {
                let caps = capsfilter(&name("vcaps"), format.lock().unwrap().pre.clone())?;
                format.lock().unwrap().inputs.push(caps.downgrade());
                let scale = make_el("videoscale", &name("vscale"))?;
                scale.set_property("add-borders", true);
                let deint = make_el("deinterlace", &name("deint"))?;
                // Before deinterlacing, which may make new frames.
                note_timecode(
                    &deint.static_pad("sink").context("deinterlace sink pad")?,
                    timecode.clone(),
                );
                vec![deint, make_el("videoconvert", &name("vconv"))?, scale, caps]
            }
            Kind::Audio(caps, matrix) => {
                let conv = make_el("audioconvert", &name("aconv"))?;
                if let Some(rows) = matrix {
                    set_mix_matrix(&conv, rows);
                }
                vec![
                    conv,
                    make_el("audioresample", &name("aresample"))?,
                    capsfilter(&name("acaps"), caps.clone())?,
                ]
            }
        };
        bin.add_many(&elements).context("add live input")?;
        gst::Element::link_many(&elements).context("link live input")?;
        let pad = aggregator
            .request_pad_simple("sink_%u")
            .context("aggregator pad")?;
        elements
            .last()
            .and_then(|e| e.static_pad("src"))
            .context("input src pad")?
            .link(&pad)
            .context("link input to aggregator")?;
        if matches!(self.kind, Kind::Video(..)) {
            // Above the background.
            pad.set_property("zorder", 1u32);
        }
        for el in &elements {
            el.sync_state_with_parent().context("start live input")?;
        }
        Ok(Chain { elements, pad })
    }

    /// Take `chain` out of the bin, on its own thread: it runs from a source's
    /// signal, whose thread the state changes could wait on.
    pub(crate) fn remove(&self, chain: Chain) {
        // Drop what the chain still sends: once its aggregator pad is gone it
        // would get not-linked, failing a source pushing into it (a channel's
        // lane) and the program with it.
        if let Some(tail) = chain.elements.last().and_then(|e| e.static_pad("src")) {
            tail.add_probe(gst::PadProbeType::DATA_DOWNSTREAM, |_, _| {
                gst::PadProbeReturn::Drop
            });
        }
        let (bin, aggregator) = (self.bin.clone(), self.aggregator.clone());
        std::thread::spawn(move || {
            // The aggregator's pad first: it flushes, releasing a source thread
            // waiting in it, which stopping the source waits for.
            if let Some(aggregator) = aggregator.upgrade() {
                aggregator.release_request_pad(&chain.pad);
            }
            for el in &chain.elements {
                let _ = el.set_state(gst::State::Null);
                if let Some(bin) = bin.upgrade() {
                    let _ = bin.remove(el);
                }
            }
        });
    }
}

/// Keep the timecode (e.g. a Decklink's RP188) of each frame entering an
/// input chain; a frame without one clears it.
fn note_timecode(pad: &gst::Pad, last: LastTimecode) {
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(buffer) = info.buffer() else {
            return gst::PadProbeReturn::Ok;
        };
        let tc = buffer
            .meta::<gst_video::VideoTimeCodeMeta>()
            .map(|m| m.tc());
        let rt = tc
            .as_ref()
            .and(buffer.pts())
            .and_then(|pts| running_time(pad, pts));
        *last.lock().unwrap() = rt.zip(tc);
        gst::PadProbeReturn::Ok
    });
}

/// The compositor makes new frames, dropping their timecode: give each the
/// newest input frame's, counted on by the frames between them (it repeats a
/// late input's frame). Recordings keep it instead of the time of day.
fn carry_source_timecode(pad: &gst::Pad, last: LastTimecode) {
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some((at, tc)) = last.lock().unwrap().clone() else {
            return gst::PadProbeReturn::Ok;
        };
        let rt = info
            .buffer()
            .and_then(|b| b.pts())
            .and_then(|pts| running_time(pad, pts));
        let fps = pad
            .current_caps()
            .and_then(|c| c.structure(0)?.get::<gst::Fraction>("framerate").ok());
        let tc = rt
            .zip(fps)
            .and_then(|(rt, fps)| timecode_at(&tc, at, rt, fps));
        if let (Some(tc), Some(buffer)) = (tc, info.buffer_mut()) {
            gst_video::VideoTimeCodeMeta::add(buffer.make_mut(), &tc);
        }
        gst::PadProbeReturn::Ok
    });
}

/// `tc` (a frame at running time `at`) carried to a frame at `rt` and `fps`;
/// `None` past [`TIMECODE_HOLD`] or if it can't be had at `fps`.
fn timecode_at(
    tc: &gst_video::ValidVideoTimeCode,
    at: gst::ClockTime,
    rt: gst::ClockTime,
    fps: gst::Fraction,
) -> Option<gst_video::ValidVideoTimeCode> {
    if fps.numer() <= 0 || fps.denom() <= 0 {
        return None;
    }
    let since = rt.nseconds() as i128 - at.nseconds() as i128;
    if since.unsigned_abs() > TIMECODE_HOLD.as_nanos() {
        return None;
    }
    let mut tc = if tc.fps() == fps {
        tc.clone()
    } else {
        // Another rate (an interlaced source at a configured progressive
        // one): the same second, its frame scaled.
        let from = tc.fps();
        let frames = tc.frames() as i64 * fps.numer() as i64 * from.denom() as i64
            / (fps.denom() as i64 * from.numer() as i64);
        gst_video::ValidVideoTimeCode::new(
            fps,
            None,
            drop_frame_flags(fps),
            tc.hours(),
            tc.minutes(),
            tc.seconds(),
            frames as u32,
            0,
        )
        .ok()?
    };
    // Rounded: the compositor's grid and the source's differ by under a frame.
    let ns_per_frame = 1_000_000_000 * fps.denom() as i128;
    let frames = (2 * since * fps.numer() as i128 + ns_per_frame).div_euclid(2 * ns_per_frame);
    tc.add_frames(frames as i64);
    Some(tc)
}

/// A `fallbacksrc` that keeps retrying forever.
pub(super) fn fallback(name: &str, video: bool, audio: bool) -> Result<gst::Element> {
    let fb = make_el("fallbacksrc", name)?;
    fb.set_property("enable-video", video);
    fb.set_property("enable-audio", audio);
    fb.set_property("timeout", SWITCH_TIMEOUT.as_nanos() as u64);
    fb.set_property("restart-timeout", RESTART_TIMEOUT.as_nanos() as u64);
    fb.set_property("retry-timeout", u64::MAX - 1);
    fb.set_property("restart-on-eos", true);
    // Same latency whether source or fallback plays; otherwise every switch
    // re-queries latency, stalling the stream, and recordings drop the pileup.
    fb.set_property("min-latency", MIN_LATENCY.as_nanos() as u64);
    // Black right away, so the monitor has frames before the source connects.
    fb.set_property("immediate-fallback", true);
    Ok(fb)
}

/// Link `src`'s `kind` pad to `chain` when it appears (fallbacksrc's pads
/// appear once it starts).
fn link_when_added(src: &gst::Element, kind: &'static str, chain: Chain) {
    let Some(sink) = chain.sink() else { return };
    src.connect_pad_added(move |_, pad| {
        if pad.name().starts_with(kind) && !sink.is_linked() {
            if let Err(e) = pad.link(&sink) {
                warn!(kind, error = ?e, "live source pad link failed");
            }
        }
    });
}

/// A direct source's sessions: each publisher's pads get fresh input chains,
/// removed with its pads. Then the source restarts: `whipserversrc` won't
/// serve a second session from the same element. `audio` is `None` when
/// audio comes from elsewhere.
fn link_sessions(source: &gst::Element, video: Inputs, audio: Option<Inputs>) {
    let current: Arc<Mutex<HashMap<String, Chain>>> = Arc::default();
    let session = Arc::new(AtomicU32::new(0));
    let added = current.clone();
    let (video_in, audio_in) = (video.clone(), audio.clone());
    source.connect_pad_added(move |_, pad| {
        let name = pad.name().to_string();
        let inputs = if name.starts_with("video") {
            &video_in
        } else if let (true, Some(audio)) = (name.starts_with("audio"), &audio_in) {
            audio
        } else {
            return;
        };
        let tag = format!("s{}{name}", session.fetch_add(1, Ordering::Relaxed));
        let chain = match inputs.add(&tag) {
            Ok(chain) => chain,
            Err(e) => {
                warn!(error = %e, "live source input failed");
                return;
            }
        };
        // A session's EOS would end the aggregator's input; the chain goes with the pad instead.
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, |_, info| {
            match &info.data {
                Some(gst::PadProbeData::Event(e)) if e.type_() == gst::EventType::Eos => {
                    gst::PadProbeReturn::Drop
                }
                _ => gst::PadProbeReturn::Ok,
            }
        });
        if let Some(sink) = chain.sink() {
            if let Err(e) = pad.link(&sink) {
                warn!(pad = %name, error = ?e, "live source pad link failed");
            }
        }
        if let Some(old) = added.lock().unwrap().insert(name, chain) {
            inputs.remove(old);
        }
    });
    source.connect_pad_removed(move |src, pad| {
        let name = pad.name().to_string();
        let Some(chain) = current.lock().unwrap().remove(&name) else {
            return;
        };
        if name.starts_with("video") {
            video.remove(chain);
            restart(src.clone());
        } else if let Some(audio) = &audio {
            audio.remove(chain);
        }
    });
}

/// Restart a direct source after its publisher left, on its own thread: this
/// runs from the element's own signal.
fn restart(src: gst::Element) {
    std::thread::spawn(move || {
        let _ = src.set_state(gst::State::Null);
        if let Err(e) = src.sync_state_with_parent() {
            warn!(error = ?e, source = %src.name(), "restarting live source failed");
        }
    });
}

/// A small live black source at the bottom of `compositor`, so it keeps
/// producing frames while a direct source has no pads.
pub(crate) fn add_background(bin: &gst::Bin, id: &str, compositor: &gst::Element) -> Result<()> {
    let src = make_el("videotestsrc", &format!("live-bg-{id}"))?;
    src.set_property("is-live", true);
    set_property(&src, "pattern", "black")?;
    let caps = capsfilter(
        &format!("live-bgcaps-{id}"),
        gst::Caps::builder("video/x-raw")
            .field("width", 16)
            .field("height", 16)
            .build(),
    )?;
    bin.add_many([&src, &caps]).context("add background")?;
    src.link(&caps).context("link background")?;
    let pad = compositor
        .request_pad_simple("sink_%u")
        .context("compositor background pad")?;
    caps.static_pad("src")
        .context("background src pad")?
        .link(&pad)
        .context("link background to compositor")?;
    pad.set_property("zorder", 0u32);
    Ok(())
}

/// Check a configured format: a zero rate or size would fail (or panic) building caps.
pub fn validate_format(format: Option<&VideoFormat>) -> Result<(), String> {
    match format {
        Some(f) if f.width == 0 || f.height == 0 || f.fps_num == 0 || f.fps_den == 0 => {
            Err("the format needs a size and frame rate".into())
        }
        _ => Ok(()),
    }
}

/// Caps before the source's format is known: the configured format, or any
/// size at the default rate. The rate must be set, or the compositor takes the
/// slowest a branch accepts (the thumbnails' 1 fps).
fn initial_video_caps(format: Option<&VideoFormat>) -> gst::Caps {
    match format {
        Some(f) => gst::Caps::builder("video/x-raw")
            .field("width", f.width as i32)
            .field("height", f.height as i32)
            .field(
                "framerate",
                gst::Fraction::new(f.fps_num as i32, f.fps_den as i32),
            )
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .build(),
        None => gst::Caps::builder("video/x-raw")
            .field(
                "framerate",
                gst::Fraction::new(DEFAULT_FPS.0, DEFAULT_FPS.1),
            )
            .build(),
    }
}

/// Count real frames for the link state, and lock output caps to the first
/// video format (a configured one wins). The lock holds for the bin's life;
/// a source that reconnects in another format is scaled into it.
///
/// It locks on the first real frame, not the first caps: a Decklink detecting
/// its signal sends caps for its first mode (NTSC) before the signal's own.
fn watch_source(
    source: &gst::Element,
    tracker: Arc<LinkTracker>,
    format: Option<VideoFormat>,
    on_lock: impl Fn(&gst::Caps) + Send + Sync + 'static,
) {
    let locked = Arc::new(AtomicBool::new(false));
    let on_lock = Arc::new(on_lock);
    let watch = move |pad: &gst::Pad| {
        let tracker = tracker.clone();
        let on_lock = on_lock.clone();
        let locked = locked.clone();
        let is_video = AtomicBool::new(false);
        pad.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
            move |pad, info| {
                match &info.data {
                    Some(gst::PadProbeData::Buffer(buffer)) => {
                        if !is_video.load(Ordering::Relaxed)
                            || buffer.flags().contains(gst::BufferFlags::GAP)
                        {
                            return gst::PadProbeReturn::Ok;
                        }
                        tracker.frame();
                        if !locked.load(Ordering::SeqCst) {
                            if let Some(caps) = pad.current_caps() {
                                if !locked.swap(true, Ordering::SeqCst) {
                                    let lock = lock_caps(&caps, format.as_ref());
                                    info!(caps = %lock, "live source format locked");
                                    on_lock(&lock);
                                }
                            }
                        }
                    }
                    Some(gst::PadProbeData::Event(ev)) => {
                        if let gst::EventView::Caps(c) = ev.view() {
                            let video = c
                                .caps()
                                .structure(0)
                                .is_some_and(|s| s.name().starts_with("video/"));
                            is_video.store(video, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
                gst::PadProbeReturn::Ok
            },
        );
    };
    for pad in source.src_pads() {
        watch(&pad);
    }
    source.connect_pad_added(move |_, pad| {
        if pad.direction() == gst::PadDirection::Src {
            watch(pad);
        }
    });
}

/// The size and pixel format of `lock`, for the converters' output.
fn pre_compositor_caps(lock: &gst::Caps) -> gst::Caps {
    let mut caps = gst::Caps::new_empty_simple("video/x-raw");
    if let (Some(from), Some(to)) = (lock.structure(0), caps.make_mut().structure_mut(0)) {
        for name in ["format", "width", "height", "pixel-aspect-ratio"] {
            if let Ok(value) = from.value(name) {
                to.set_value(name, value.clone());
            }
        }
    }
    caps
}

/// Output caps for a source whose first video caps are `caps`.
pub(crate) fn lock_caps(caps: &gst::CapsRef, format: Option<&VideoFormat>) -> gst::Caps {
    let s = caps.structure(0);
    let int = |name: &str| s.and_then(|s| s.get::<i32>(name).ok());
    let frac = |name: &str| s.and_then(|s| s.get::<gst::Fraction>(name).ok());
    let text = |name: &str| s.and_then(|s| s.get::<&str>(name).ok());
    // Fix every field the fallback's frames could differ in: an encoder given
    // new caps mid-recording can emit new codec data, which muxers refuse.
    let mut caps: gst::Caps = "video/x-raw, interlace-mode=progressive, multiview-mode=mono"
        .parse()
        .expect("valid caps");
    let st = caps.make_mut().structure_mut(0).expect("one structure");
    st.set_value("multiview-flags", no_multiview_flags());
    for name in ["format", "colorimetry", "chroma-site"] {
        if let Some(value) = text(name) {
            st.set(name, value);
        }
    }
    let (w, h, rate, par) = match format {
        Some(f) => (
            f.width as i32,
            f.height as i32,
            gst::Fraction::new(f.fps_num as i32, f.fps_den as i32),
            gst::Fraction::new(1, 1),
        ),
        None => (
            int("width").unwrap_or(1920),
            int("height").unwrap_or(1080),
            frac("framerate")
                .filter(|r| r.numer() > 0)
                .unwrap_or(gst::Fraction::new(DEFAULT_FPS.0, DEFAULT_FPS.1)),
            frac("pixel-aspect-ratio").unwrap_or(gst::Fraction::new(1, 1)),
        ),
    };
    st.set("width", w);
    st.set("height", h);
    st.set("framerate", rate);
    st.set("pixel-aspect-ratio", par);
    caps
}

/// `multiview-flags` with no flags set and all fixed. Built, not parsed:
/// GStreamer 1.28 no longer parses flagsets from caps strings.
fn no_multiview_flags() -> glib::SendValue {
    use glib::translate::{from_glib, ToGlibPtrMut, UnsafeFrom};
    // SAFETY: the type getter registers the flagset type; the value is
    // initialized to that type before it's set, and owns its GValue.
    unsafe {
        let ty: glib::Type = from_glib(gstreamer_video_sys::gst_video_multiview_flagset_get_type());
        let mut value = glib::Value::from_type(ty);
        gstreamer_sys::gst_value_set_flagset(value.to_glib_none_mut().0, 0, u32::MAX);
        glib::SendValue::unsafe_from(value.into_raw())
    }
}

/// `audioconvert` matrix picking `channels` (1-based) out of `inputs`.
fn pick_matrix(inputs: u32, channels: &[u32]) -> Vec<Vec<f32>> {
    channels
        .iter()
        .map(|&c| {
            (1..=inputs.max(1))
                .map(|i| if i == c { 1.0 } else { 0.0 })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use gstreamer_app as gst_app;

    use super::*;

    #[test]
    fn pick_matrix_routes_channels() {
        assert_eq!(
            pick_matrix(4, &[3, 1]),
            vec![vec![0.0, 0.0, 1.0, 0.0], vec![1.0, 0.0, 0.0, 0.0]]
        );
    }

    fn tc(
        fps: (i32, i32),
        flags: gst_video::VideoTimeCodeFlags,
        at: [u32; 4],
    ) -> gst_video::ValidVideoTimeCode {
        let fps = gst::Fraction::new(fps.0, fps.1);
        gst_video::ValidVideoTimeCode::new(fps, None, flags, at[0], at[1], at[2], at[3], 0).unwrap()
    }

    /// A source's timecode counts on by the frames since it, rounded, for
    /// up to [`TIMECODE_HOLD`]; at another rate the frame is scaled.
    #[test]
    fn timecode_counts_on_from_the_source() {
        gst::init().unwrap();
        let none = gst_video::VideoTimeCodeFlags::empty();
        let drop = gst_video::VideoTimeCodeFlags::DROP_FRAME;
        let ms = gst::ClockTime::from_mseconds;
        let src = tc((25, 1), none, [10, 0, 0, 24]);
        let fps = gst::Fraction::new(25, 1);
        let at = |rt| timecode_at(&src, ms(1000), rt, fps).map(|t| t.to_string());

        assert_eq!(at(ms(1000)).as_deref(), Some("10:00:00:24"));
        // Off the source's grid by under half a frame.
        assert_eq!(at(ms(1015)).as_deref(), Some("10:00:00:24"));
        assert_eq!(at(ms(1080)).as_deref(), Some("10:00:01:01"));
        // The compositor can be a frame behind its newest input.
        assert_eq!(at(ms(960)).as_deref(), Some("10:00:00:23"));
        assert_eq!(at(ms(2500)), None, "held too long");

        // 1080i59.94 (29.97 frames) shown at a configured 59.94p.
        let src = tc((30000, 1001), drop, [10, 0, 0, 12]);
        let to = gst::Fraction::new(60000, 1001);
        let got = timecode_at(&src, ms(1000), ms(1000), to).unwrap();
        assert_eq!(got.to_string(), "10:00:00;24");
        assert_eq!(got.fps(), to);
    }

    /// A frame's timecode comes out of the compositor, and counts frames.
    #[test]
    fn source_timecode_survives_the_compositor() {
        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        let capture = gst::parse::bin_from_description(
            "videotestsrc is-live=1 ! video/x-raw,width=320,height=180,framerate=25/1 \
             ! timecodestamper name=tc set=always",
            false,
        )
        .unwrap();
        let start = gst_video::VideoTimeCode::new(
            gst::Fraction::new(25, 1),
            None,
            gst_video::VideoTimeCodeFlags::empty(),
            10,
            0,
            0,
            0,
            0,
        );
        capture
            .by_name("tc")
            .unwrap()
            .set_property("set-internal-timecode", &start);
        let tracker = Arc::new(LinkTracker::default());
        let bin = build_bin(LiveInput {
            id: "t",
            source: capture.clone().upcast(),
            audio: LiveAudio::Silence { channels: 2 },
            format: None,
            tracker: &tracker,
            direct: true,
            latency: None,
        })
        .unwrap();
        let tc_src = capture.by_name("tc").unwrap().static_pad("src").unwrap();
        let ghost = gst::GhostPad::builder_with_target(&tc_src)
            .unwrap()
            .name("video")
            .build();
        capture.add_pad(&ghost).unwrap();

        let pipeline = gst::Pipeline::new();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_ = seen.clone();
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
                        seen_.lock().unwrap().push(tc);
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
        std::thread::sleep(Duration::from_millis(1500));
        pipeline.set_state(gst::State::Null).unwrap();

        let seen = seen.lock().unwrap();
        // The first frames may be the background's, before the source links.
        let stamped: Vec<_> = seen.iter().skip_while(|t| *t == "-").collect();
        println!(
            "{} frames, {} stamped: {:?}…",
            seen.len(),
            stamped.len(),
            &stamped[..stamped.len().min(5)]
        );
        assert!(stamped.len() >= 20, "{seen:?}");
        assert!(
            stamped.iter().all(|t| t.starts_with("10:00:0")),
            "{stamped:?}"
        );
        let frame = |t: &str| {
            let n: Vec<u32> = t.split([':', ';']).map(|p| p.parse().unwrap()).collect();
            n[2] * 25 + n[3]
        };
        for pair in stamped.windows(2) {
            assert_eq!(frame(pair[1]), frame(pair[0]) + 1, "{stamped:?}");
        }
    }

    /// Caps and no-signal (gap) frames ahead of the real format don't lock it,
    /// as a Decklink sends for its first mode while it detects the signal.
    #[test]
    fn format_locks_on_the_first_real_frame() {
        gst::init().unwrap();
        let capture = gst::parse::bin_from_description(
            "videotestsrc is-live=1 \
             ! capsfilter name=cf caps=video/x-raw,width=720,height=486,framerate=30/1",
            false,
        )
        .unwrap();
        let cf = capture.by_name("cf").unwrap();
        let cf_src = cf.static_pad("src").unwrap();
        let seen = AtomicU64::new(0);
        let cf_ = cf.clone();
        // Gap frames until the source is in its real format, asked for after a few.
        cf_src.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let real = pad
                .current_caps()
                .and_then(|c| c.structure(0)?.get::<i32>("width").ok())
                == Some(1280);
            if seen.fetch_add(1, Ordering::Relaxed) == 5 {
                let cf = cf_.clone();
                std::thread::spawn(move || {
                    let caps: gst::Caps = "video/x-raw,width=1280,height=720,framerate=30/1"
                        .parse()
                        .unwrap();
                    cf.set_property("caps", &caps);
                });
            }
            if let (false, Some(buffer)) = (real, info.buffer_mut()) {
                buffer.make_mut().set_flags(gst::BufferFlags::GAP);
            }
            gst::PadProbeReturn::Ok
        });
        let tracker = Arc::new(LinkTracker::default());
        let bin = build_bin(LiveInput {
            id: "t",
            source: capture.clone().upcast(),
            audio: LiveAudio::Silence { channels: 2 },
            format: None,
            tracker: &tracker,
            direct: true,
            latency: None,
        })
        .unwrap();
        let ghost = gst::GhostPad::builder_with_target(&cf_src)
            .unwrap()
            .name("video")
            .build();
        capture.add_pad(&ghost).unwrap();

        let pipeline = gst::Pipeline::new();
        let v = Arc::new(Sink::default());
        let vsink = counting_sink(v.clone());
        let asink = make_el("fakesink", "asink").unwrap();
        pipeline
            .add_many([bin.upcast_ref(), &vsink, &asink])
            .unwrap();
        bin.link_pads(Some("video"), &vsink, None).unwrap();
        bin.link_pads(Some("audio"), &asink, None).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        pipeline.set_state(gst::State::Null).unwrap();

        // Unlocked frames pass as they are; locked caps carry the pinned multiview.
        let caps = v.caps.lock().unwrap();
        let locked: Vec<_> = caps
            .iter()
            .filter(|c| c.contains("multiview-mode"))
            .collect();
        assert!(!locked.is_empty(), "never locked: {caps:#?}");
        assert!(
            locked.iter().all(|c| c.contains("width=(int)1280")),
            "locked to the placeholder: {caps:#?}"
        );
    }

    /// Locked caps are fixed, with mono multiview and every flag pinned, so
    /// they accept a source's unflagged frames.
    #[test]
    fn lock_caps_pins_multiview() {
        gst::init().unwrap();
        let src: gst::Caps = "video/x-raw, format=I420, width=1280, height=720, framerate=30/1"
            .parse()
            .unwrap();
        let locked = lock_caps(&src, None);
        assert!(locked.is_fixed(), "{locked}");
        let text = locked.to_string();
        assert!(text.contains("multiview-flags"), "{text}");
        let s = locked.structure(0).unwrap();
        assert_eq!(s.get::<i32>("width").unwrap(), 1280);
        assert_eq!(s.get::<&str>("multiview-mode").unwrap(), "mono");
    }

    #[derive(Default)]
    struct Sink {
        frames: AtomicU64,
        /// Each distinct caps, in order.
        caps: Mutex<Vec<String>>,
        last: Mutex<Option<(gst::ClockTime, Instant)>>,
        /// Timestamps that went backwards, or jumped over 100 ms.
        bad_pts: Mutex<Vec<String>>,
        /// Waits over 250 ms between buffers: a recording's 500 ms input
        /// queue would drop the burst that follows.
        stalls: Mutex<Vec<Duration>>,
    }

    fn counting_sink(c: Arc<Sink>) -> gst::Element {
        let sink = gst_app::AppSink::builder().sync(false).build();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(caps) = sample.caps() {
                        let caps = caps.to_string();
                        let mut seen = c.caps.lock().unwrap();
                        if seen.last() != Some(&caps) {
                            seen.push(caps);
                        }
                    }
                    if let Some(pts) = sample.buffer().and_then(|b| b.pts()) {
                        let now = Instant::now();
                        let mut last = c.last.lock().unwrap();
                        if let Some((prev, at)) = *last {
                            if pts < prev || pts - prev > gst::ClockTime::from_mseconds(100) {
                                c.bad_pts.lock().unwrap().push(format!("{prev} → {pts}"));
                            }
                            if now - at > Duration::from_millis(100) {
                                c.stalls.lock().unwrap().push(now - at);
                            }
                        }
                        *last = Some((pts, now));
                    }
                    c.frames.fetch_add(1, Ordering::Relaxed);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        sink.upcast()
    }

    /// A sender dropout gives same-format fallback frames without a gap, then
    /// the source comes back; the link state follows. Needs `srtsrc`.
    #[test]
    #[ignore = "real-time (~30 s); run with --ignored"]
    fn srt_dropout_keeps_format_and_recovers() {
        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        let port = 7731;

        let tracker = Arc::new(LinkTracker::default());
        let source = gst::ElementFactory::make("uridecodebin3")
            .property("uri", format!("srt://127.0.0.1:{port}?mode=listener"))
            .build()
            .unwrap();
        let bin = build_bin(LiveInput {
            id: "t",
            source,
            audio: LiveAudio::Source { channels: 2 },
            format: None,
            tracker: &tracker,
            direct: false,
            latency: None,
        })
        .unwrap();
        let recv = gst::Pipeline::new();
        let (v, a) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
        let (vs, as_) = (counting_sink(v.clone()), counting_sink(a.clone()));
        recv.add_many([bin.upcast_ref(), &vs, &as_]).unwrap();
        bin.static_pad("video")
            .unwrap()
            .link(&vs.static_pad("sink").unwrap())
            .unwrap();
        bin.static_pad("audio")
            .unwrap()
            .link(&as_.static_pad("sink").unwrap())
            .unwrap();
        let bus = recv.bus().unwrap();
        let counts = Arc::new(Mutex::new(std::collections::BTreeMap::<String, u32>::new()));
        let c2 = counts.clone();
        bus.set_sync_handler(move |_, msg| {
            let key = match msg.view() {
                gst::MessageView::Latency(_) => "latency".to_string(),
                gst::MessageView::StreamCollection(_) => "collection".to_string(),
                gst::MessageView::Warning(w) => format!("warn {}", w.error()),
                gst::MessageView::Error(e) => format!("error {}", e.error()),
                _ => return gst::BusSyncReply::Drop,
            };
            *c2.lock().unwrap().entry(key).or_default() += 1;
            gst::BusSyncReply::Drop
        });
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        for kind in ["video", "audio"] {
            let ev = events.clone();
            bin.static_pad(kind).unwrap().add_probe(
                gst::PadProbeType::EVENT_DOWNSTREAM | gst::PadProbeType::EVENT_FLUSH,
                move |_, info| {
                    if let Some(gst::PadProbeData::Event(e)) = &info.data {
                        ev.lock().unwrap().push(format!("{kind}:{:?}", e.type_()));
                    }
                    gst::PadProbeReturn::Ok
                },
            );
        }
        recv.set_state(gst::State::Playing).unwrap();

        let send = || {
            let p = gst::parse::launch(&format!(
                "videotestsrc is-live=1 pattern=ball ! video/x-raw,width=1280,height=720,framerate=30/1,format=I420 \
                 ! x264enc tune=zerolatency key-int-max=30 ! mux. \
                 audiotestsrc is-live=1 ! audioconvert ! avenc_aac ! aacparse ! mux. \
                 mpegtsmux name=mux ! srtsink uri=srt://127.0.0.1:{port}?mode=caller"
            ))
            .unwrap();
            p.set_state(gst::State::Playing).unwrap();
            p
        };
        let frames = || v.frames.load(Ordering::Relaxed);
        let vpad = bin.static_pad("video").unwrap();
        let sleep = move |secs: u64| {
            for _ in 0..secs {
                std::thread::sleep(Duration::from_secs(1));
                let mut q = gst::query::Latency::new();
                if vpad.query(&mut q) {
                    let (live, min, max) = q.result();
                    print!(
                        "[{live} {min} {}] ",
                        max.map_or("-".into(), |m| m.to_string())
                    );
                }
            }
            println!();
        };

        sleep(3);
        assert_eq!(tracker.state(false), LinkState::Connecting);
        assert!(frames() > 30, "fallback plays before the sender connects");

        let s = send();
        sleep(5);
        assert_eq!(tracker.state(false), LinkState::Live);

        s.set_state(gst::State::Null).unwrap();
        drop(s);
        // Long enough for fallbacksrc to restart the source a few times.
        let before = frames();
        sleep(15);
        assert_eq!(tracker.state(false), LinkState::Reconnecting);
        assert!(
            frames() - before > 400,
            "fallback frames during the dropout"
        );

        let s = send();
        sleep(8);
        assert_eq!(tracker.state(false), LinkState::Live);
        s.set_state(gst::State::Null).unwrap();
        recv.set_state(gst::State::Null).unwrap();

        println!("bus: {:?}", counts.lock().unwrap());
        println!("events: {:?}", events.lock().unwrap());
        // Continuous on both streams, in timestamps and in time, dropout included.
        for (name, sink) in [("video", &v), ("audio", &a)] {
            assert_eq!(
                *sink.bad_pts.lock().unwrap(),
                Vec::<String>::new(),
                "{name} timestamps"
            );
            assert_eq!(
                *sink.stalls.lock().unwrap(),
                Vec::<std::time::Duration>::new(),
                "{name} stalls"
            );
        }
        let vcaps = v.caps.lock().unwrap();
        let acaps = a.caps.lock().unwrap();
        // Black frames pass as they are until the first live frame locks
        // the format; from then on it never changes.
        let locked = vcaps
            .iter()
            .position(|c| c.contains("width=(int)1280"))
            .expect("locked to the stream's size");
        assert_eq!(locked, vcaps.len() - 1, "video caps changed: {vcaps:#?}");
        assert_eq!(acaps.len(), 1, "audio caps changed: {acaps:#?}");
    }
}
