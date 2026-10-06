//! Live external sources (streams, devices, WHIP).
//!
//! Each one runs through `fallbacksrc` (gst-plugins-rs), which plays black
//! and silence while the source is missing or stalled and keeps retrying it.
//! A dropout therefore never stops the monitor or its recordings: they record
//! black and silence until the source comes back.
//!
//! The fallback frames have their own format, so both streams are normalized
//! after `fallbacksrc`: video is deinterlaced and held to the source's first
//! live size, rate, pixel format and colorimetry (or a configured size and rate), audio to 48 kHz at a
//! fixed channel count. A recording therefore never sees the format change.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use tracing::{info, warn};

use super::add_ghost_pad;
use crate::api::types::{LinkState, LiveVideoFormat};
use crate::pipeline::{capsfilter, make_el, set_mix_matrix};

/// Rate every live source's audio is resampled to.
pub const AUDIO_RATE: i32 = 48_000;
/// Rate used when the source doesn't say (or reports a variable rate).
const DEFAULT_FPS: (i32, i32) = (30, 1);
/// How long the source may stall before the fallback takes over.
/// Kept under the 500 ms a recording's input queue holds: the frames
/// `videorate` fills the gap with arrive in a burst when it ends.
const SWITCH_TIMEOUT: Duration = Duration::from_millis(2000);
/// How long a stalled source runs before it's restarted.
const RESTART_TIMEOUT: Duration = Duration::from_secs(5);
/// Latency every live source reports, fallback or not. Covers a stream's
/// jitter buffer (`latency_ms`, 200 by default) plus decoding.
const MIN_LATENCY: Duration = Duration::from_secs(1);
/// Without a real frame for this long, the source counts as down. HLS
/// arrives a segment at a time, so the frames come in bursts.
const LIVE_WINDOW: Duration = Duration::from_secs(3);

// ── Link state ────────────────────────────────────────────────────────────────

/// Tracks whether a live source is delivering real frames (as opposed to the
/// fallback's). Read by the 1 Hz `feed.status` emitter.
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

// ── Audio ─────────────────────────────────────────────────────────────────────

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

// ── Bin ───────────────────────────────────────────────────────────────────────

/// What [`build_bin`] wraps.
pub struct LiveInput<'a> {
    pub id: &'a str,
    /// An element with raw video (and maybe audio) src pads, static or
    /// dynamic: a decoding bin for streams, a device's element for devices.
    pub source: gst::Element,
    pub audio: LiveAudio,
    /// Hold video to this size and rate instead of the source's first.
    pub format: Option<&'a LiveVideoFormat>,
    pub tracker: &'a Arc<LinkTracker>,
    /// Link the source's pads straight in rather than through
    /// `fallbacksrc`: for a server that comes and goes with its publisher
    /// (WHIP) and mustn't be restarted. A black background keeps the video
    /// running while it has no pads, and its end-of-streams are dropped so
    /// the next publisher's pads link in.
    pub direct: bool,
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
/// The compositor and mixer run on the clock: when the source is late or
/// gone they repeat the last frame or play silence instead of leaving a gap,
/// so a recording never gets the burst of catch-up data its input queue
/// would drop (nor, for audio, overlaps from network jitter).
pub fn build_bin(input: LiveInput) -> Result<gst::Bin> {
    let LiveInput {
        id,
        source,
        audio,
        format,
        tracker,
        direct,
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

    // ── Video ────────────────────────────────────────────────────────────────
    let compositor = make_el("compositor", &format!("live-vsync-{id}"))?;
    compositor.set_property_from_str("background", "black");
    let out_vcaps = capsfilter(&format!("live-outvcaps-{id}"), initial_video_caps(format))?;
    bin.add_many([&compositor, &out_vcaps])
        .context("add live video output")?;
    compositor
        .link(&out_vcaps)
        .context("link live video output")?;
    add_ghost_pad(&bin, &out_vcaps, "video")?;
    let video_format = Arc::new(Mutex::new(VideoFormat {
        pre: pre_compositor_caps(&initial_video_caps(format)),
        inputs: Vec::new(),
        out: out_vcaps.downgrade(),
    }));
    let video = Inputs {
        bin: bin.downgrade(),
        id: id.to_string(),
        aggregator: compositor.downgrade(),
        kind: Kind::Video(video_format.clone()),
    };

    // ── Audio ────────────────────────────────────────────────────────────────
    let channels = match &audio {
        LiveAudio::Source { channels } | LiveAudio::Silence { channels } => *channels,
        LiveAudio::Element { channels, .. } => channels.len() as u32,
    };
    let audio_caps = audio_caps(channels);
    let mixer = make_el("audiomixer", &format!("live-amix-{id}"))?;
    let out_acaps = capsfilter(&format!("live-acaps-{id}"), audio_caps.clone())?;
    let silence = make_el("audiotestsrc", &format!("live-silence-{id}"))?;
    silence.set_property("is-live", true);
    silence.set_property_from_str("wave", "silence");
    let silence_caps = capsfilter(&format!("live-scaps-{id}"), audio_caps.clone())?;
    bin.add_many([&mixer, &out_acaps, &silence, &silence_caps])
        .context("add live audio output")?;
    gst::Element::link_many([&silence, &silence_caps, &mixer, &out_acaps])
        .context("link live audio output")?;
    add_ghost_pad(&bin, &out_acaps, "audio")?;
    let audio_inputs = Inputs {
        bin: bin.downgrade(),
        id: id.to_string(),
        aggregator: mixer.downgrade(),
        kind: Kind::Audio(audio_caps, None),
    };

    // ── Inputs ───────────────────────────────────────────────────────────────
    watch_source(&source, tracker.clone(), format.copied(), move |lock| {
        video_format.lock().unwrap().lock(lock);
    });
    if direct {
        // A black layer under the source keeps the compositor producing
        // frames while nobody publishes.
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

fn audio_caps(channels: u32) -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "F32LE")
        .field("rate", AUDIO_RATE)
        .field("channels", channels.max(1) as i32)
        .field("layout", "interleaved")
        .build()
}

/// The video format inputs are converted to, and who has to know when it's
/// locked.
struct VideoFormat {
    /// Caps for each input's converters (see [`pre_compositor_caps`]).
    pre: gst::Caps,
    /// The inputs' capsfilters.
    inputs: Vec<glib::WeakRef<gst::Element>>,
    out: glib::WeakRef<gst::Element>,
}

impl VideoFormat {
    fn lock(&mut self, lock: &gst::Caps) {
        // Before the compositor, only what the converters can always
        // produce: stricter caps would reach fallbacksrc's black-frame
        // source, which then fails to negotiate.
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

#[derive(Clone)]
enum Kind {
    Video(Arc<Mutex<VideoFormat>>),
    /// The mixer's caps, and a channel-picking matrix.
    Audio(gst::Caps, Option<Vec<Vec<f32>>>),
}

/// Makes input chains into one of the bin's aggregators (the compositor or
/// the mixer).
#[derive(Clone)]
struct Inputs {
    bin: glib::WeakRef<gst::Bin>,
    id: String,
    aggregator: glib::WeakRef<gst::Element>,
    kind: Kind,
}

/// One input chain, and the aggregator pad it feeds.
struct Chain {
    elements: Vec<gst::Element>,
    pad: gst::Pad,
}

impl Chain {
    fn sink(&self) -> Option<gst::Pad> {
        self.elements.first().and_then(|e| e.static_pad("sink"))
    }
}

impl Inputs {
    fn audio_caps(&self) -> gst::Caps {
        match &self.kind {
            Kind::Audio(caps, _) => caps.clone(),
            Kind::Video(_) => gst::Caps::new_empty_simple("audio/x-raw"),
        }
    }

    /// Add a chain converting into the aggregator's format, named by `tag`
    /// (unique per bin).
    fn add(&self, tag: &str) -> Result<Chain> {
        let bin = self.bin.upgrade().context("live bin gone")?;
        let aggregator = self.aggregator.upgrade().context("aggregator gone")?;
        let id = &self.id;
        let name = |el: &str| format!("live-{tag}-{el}-{id}");
        let elements = match &self.kind {
            Kind::Video(format) => {
                let caps = capsfilter(&name("vcaps"), format.lock().unwrap().pre.clone())?;
                format.lock().unwrap().inputs.push(caps.downgrade());
                let scale = make_el("videoscale", &name("vscale"))?;
                scale.set_property("add-borders", true);
                vec![
                    make_el("deinterlace", &name("deint"))?,
                    make_el("videoconvert", &name("vconv"))?,
                    scale,
                    caps,
                ]
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
        if matches!(self.kind, Kind::Video(_)) {
            // Above the background.
            pad.set_property("zorder", 1u32);
        }
        for el in &elements {
            el.sync_state_with_parent().context("start live input")?;
        }
        Ok(Chain { elements, pad })
    }

    /// Take `chain` out of the bin. On its own thread: it runs from a
    /// source's signal, whose thread the state changes could wait on.
    fn remove(&self, chain: Chain) {
        let (bin, aggregator) = (self.bin.clone(), self.aggregator.clone());
        std::thread::spawn(move || {
            for el in &chain.elements {
                let _ = el.set_state(gst::State::Null);
                if let Some(bin) = bin.upgrade() {
                    let _ = bin.remove(el);
                }
            }
            if let Some(aggregator) = aggregator.upgrade() {
                aggregator.release_request_pad(&chain.pad);
            }
        });
    }
}

/// A `fallbacksrc` that keeps retrying forever.
fn fallback(name: &str, video: bool, audio: bool) -> Result<gst::Element> {
    let fb = make_el("fallbacksrc", name)?;
    fb.set_property("enable-video", video);
    fb.set_property("enable-audio", audio);
    fb.set_property("timeout", SWITCH_TIMEOUT.as_nanos() as u64);
    fb.set_property("restart-timeout", RESTART_TIMEOUT.as_nanos() as u64);
    fb.set_property("retry-timeout", u64::MAX - 1);
    fb.set_property("restart-on-eos", true);
    // Report the same latency whether the source or the fallback plays.
    // Otherwise every switch and retry changes it, the pipeline re-queries
    // latency (which stalls the stream while it waits on the aggregators),
    // and recordings drop the frames that pile up.
    fb.set_property("min-latency", MIN_LATENCY.as_nanos() as u64);
    // Output black right away, so the monitor has frames before the source
    // connects.
    fb.set_property("immediate-fallback", true);
    Ok(fb)
}

/// Link `src`'s `kind` pad (`"video"` / `"audio"`) to `chain` when it
/// appears (fallbacksrc's pads appear once it starts).
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

/// A direct source's sessions: each publisher's pads get fresh input
/// chains, removed when its pads go. Then the source is restarted:
/// `whipserversrc` doesn't serve a second session's media from the same
/// element. `audio` is `None` when the audio comes from elsewhere.
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
        // A session's end-of-stream would end the aggregator's input; the
        // chain is removed with the pad instead.
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

/// Restart a direct source after its publisher left. On its own thread:
/// this runs from the element's own signal.
fn restart(src: gst::Element) {
    std::thread::spawn(move || {
        let _ = src.set_state(gst::State::Null);
        if let Err(e) = src.sync_state_with_parent() {
            warn!(error = ?e, source = %src.name(), "restarting live source failed");
        }
    });
}

/// A small live black source at the bottom of `compositor`, so it keeps
/// producing frames while a direct source has no pads. The compositor's own
/// (black) background fills the rest of the frame.
fn add_background(bin: &gst::Bin, id: &str, compositor: &gst::Element) -> Result<()> {
    let src = make_el("videotestsrc", &format!("live-bg-{id}"))?;
    src.set_property("is-live", true);
    src.set_property_from_str("pattern", "black");
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

/// Caps before the source's format is known: the configured format, or
/// any size at the default rate (the fallback's black frames, until it
/// locks). The rate has to be set: the compositor would otherwise take the
/// slowest a branch accepts (the thumbnails' 1 fps).
fn initial_video_caps(format: Option<&LiveVideoFormat>) -> gst::Caps {
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

/// Watch `source`'s video pads: count real frames for the link state, and
/// lock the output caps to the first video format (a configured size and
/// rate win). The lock holds for the bin's life, so a source that reconnects
/// in another format is scaled into the first.
fn watch_source(
    source: &gst::Element,
    tracker: Arc<LinkTracker>,
    format: Option<LiveVideoFormat>,
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
            move |_, info| {
                match &info.data {
                    Some(gst::PadProbeData::Buffer(_)) => {
                        if is_video.load(Ordering::Relaxed) {
                            tracker.frame();
                        }
                    }
                    Some(gst::PadProbeData::Event(ev)) => {
                        if let gst::EventView::Caps(c) = ev.view() {
                            let caps = c.caps();
                            let video = caps
                                .structure(0)
                                .is_some_and(|s| s.name().starts_with("video/"));
                            is_video.store(video, Ordering::Relaxed);
                            if video && !locked.swap(true, Ordering::SeqCst) {
                                let lock = lock_caps(caps, format.as_ref());
                                info!(caps = %lock, "live source format locked");
                                on_lock(&lock);
                            }
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
fn lock_caps(caps: &gst::CapsRef, format: Option<&LiveVideoFormat>) -> gst::Caps {
    let s = caps.structure(0);
    let int = |name: &str| s.and_then(|s| s.get::<i32>(name).ok());
    let frac = |name: &str| s.and_then(|s| s.get::<gst::Fraction>(name).ok());
    let text = |name: &str| s.and_then(|s| s.get::<&str>(name).ok());
    // Every field the fallback's frames could differ in is fixed, so the
    // caps never change: an encoder given new caps mid-recording can emit
    // new codec data, which the muxers refuse.
    let mut caps: gst::Caps = "video/x-raw, interlace-mode=progressive, multiview-mode=mono, \
        multiview-flags=(GstVideoMultiviewFlagsSet)0:ffffffff"
        .parse()
        .expect("valid caps");
    let st = caps.make_mut().structure_mut(0).expect("one structure");
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

    /// What a test sink saw.
    #[derive(Default)]
    struct Sink {
        frames: AtomicU64,
        /// Each distinct caps, in order.
        caps: Mutex<Vec<String>>,
        last: Mutex<Option<(gst::ClockTime, Instant)>>,
        /// Timestamps that went backwards, or jumped over 100 ms.
        bad_pts: Mutex<Vec<String>>,
        /// Wall-clock waits over 250 ms between buffers: a recording's
        /// input queue (500 ms) would drop the burst that follows.
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

    /// A sender dropping out gives fallback frames in the same format and
    /// without a gap, then the source comes back; the link state follows.
    /// Needs `srtsrc`.
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
        // Continuous on both streams, in timestamps and in time, dropout
        // included.
        for (name, sink) in [("video", &v), ("audio", &a)] {
            assert_eq!(
                *sink.bad_pts.lock().unwrap(),
                Vec::<String>::new(),
                "{name} timestamps"
            );
            assert_eq!(*sink.stalls.lock().unwrap(), vec![], "{name} stalls");
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
