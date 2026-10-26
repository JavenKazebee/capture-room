//! GStreamer pipelines. Each source has one always-on [`MonitorPipeline`];
//! everything that consumes it (a recording leg, an output, a replay buffer)
//! runs in its own pipeline fed by the monitor's [`StreamProducer`]s, an
//! appsink fanning out to appsrcs. Not a tee: a tee passes a branch's error
//! back to the source, so one failing leg would stop the monitor and every
//! other leg.
//!
//! Consumers share the monitor's clock and base time. Each appsrc holds
//! 500 ms and drops the oldest when full, so a slow encoder drops frames in
//! its own leg only.

pub mod monitor;
pub mod output;
pub mod profile;
pub mod recording;
pub mod replay;
pub mod rtsp;
pub mod whep;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tokio::sync::watch;

use monitor::MonitorPipeline;

use crate::api::types::ChannelLevelDto;

/// The most recent value a pipeline produced, shared between the GStreamer
/// thread that writes it and the API that reads it. Clones share storage.
#[derive(Debug)]
pub struct Latest<T>(Arc<Mutex<Option<T>>>);

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Clone> Latest<T> {
    pub fn set(&self, value: T) {
        *self.0.lock().unwrap() = Some(value);
    }

    /// `None` until the first value arrives.
    pub fn get(&self) -> Option<T> {
        self.0.lock().unwrap().clone()
    }
}

/// Latest per-channel audio levels from the `level` element.
pub type AudioMeter = Latest<Vec<ChannelLevelDto>>;
/// Latest thumbnail JPEG.
pub type ThumbnailStore = Latest<Thumbnail>;

/// A thumbnail JPEG, numbered so a client can tell a new one from one it has.
/// Numbers are unique across monitors, so a restarted one never repeats one.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub seq: u64,
    pub jpeg: axum::body::Bytes,
}

static NEXT_THUMBNAIL: AtomicU64 = AtomicU64::new(1);
/// Marked whenever any monitor stores a thumbnail, so thumbnail sockets wake
/// for new frames rather than poll.
static THUMBNAILS_CHANGED: LazyLock<watch::Sender<()>> = LazyLock::new(|| watch::channel(()).0);

impl ThumbnailStore {
    fn store(&self, jpeg: &[u8]) {
        self.set(Thumbnail {
            seq: NEXT_THUMBNAIL.fetch_add(1, Ordering::Relaxed),
            jpeg: axum::body::Bytes::copy_from_slice(jpeg),
        });
        THUMBNAILS_CHANGED.send_replace(());
    }
}

/// Changes whenever any monitor has a new thumbnail.
pub fn thumbnails_changed() -> watch::Receiver<()> {
    THUMBNAILS_CHANGED.subscribe()
}

/// The first error a pipeline posted, kept to show: a pipeline stops after
/// one, so later errors only follow from it. Clones share storage.
#[derive(Debug, Clone, Default)]
pub struct FirstError(Arc<Mutex<Option<String>>>);

impl FirstError {
    /// Keep `err` (see [`describe_error`]) unless one is already kept:
    /// whether it was the first.
    pub fn set(&self, err: &gst::message::Error) -> bool {
        let mut first = self.0.lock().unwrap();
        if first.is_some() {
            return false;
        }
        *first = Some(describe_error(err));
        true
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Set `pipeline` playing. If it won't, stop it and fail with the error its
/// bus handler kept in `first` (so the handler must be a sync one), or
/// `fallback` when it posted none.
pub(crate) fn start_or_reason(
    pipeline: &gst::Pipeline,
    first: &FirstError,
    fallback: &str,
) -> Result<()> {
    if set_state(pipeline, gst::State::Playing).is_ok() {
        return Ok(());
    }
    let _ = set_state(pipeline, gst::State::Null);
    Err(anyhow::anyhow!(first
        .get()
        .unwrap_or_else(|| fallback.into())))
}

/// GStreamer's streaming errors are a generic "Internal data stream error"
/// with the real reason only in the debug text ("… reason not-negotiated
/// (-4)"); surface it.
pub(crate) fn describe_error(err: &gst::message::Error) -> String {
    let reason = err.debug().and_then(|d| {
        d.split("reason ")
            .nth(1)
            .map(|r| r.split(' ').next().unwrap_or(r).to_string())
    });
    match reason.as_deref() {
        Some("not-negotiated") => format!(
            "{} (not-negotiated: the encoder's output isn't accepted by this container or format)",
            err.error()
        ),
        Some(reason) => format!("{} ({reason})", err.error()),
        None => err.error().to_string(),
    }
}

/// Change `pipeline`'s state on a thread outside the tokio runtime. Some
/// elements run their own runtime and block on it while changing state
/// (`whipserversrc`), which panics — and aborts, across the FFI boundary —
/// on a thread that's already inside one, as ours are.
pub(crate) fn set_state(
    pipeline: &gst::Pipeline,
    state: gst::State,
) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
    if tokio::runtime::Handle::try_current().is_err() {
        return pipeline.set_state(state);
    }
    std::thread::scope(|s| {
        s.spawn(|| pipeline.set_state(state))
            .join()
            .unwrap_or(Err(gst::StateChangeError))
    })
}

pub(crate) fn make_el(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("create {factory} (is its GStreamer plugin installed?)"))
}

/// A free loopback port for a signaller (`whipserversrc`, `whepserversink`).
pub(crate) fn loopback_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

/// For requests to the node's own signallers on loopback. A system or env
/// proxy would otherwise be sent them.
pub(crate) fn loopback_http() -> &'static reqwest::Client {
    static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build the loopback HTTP client")
    });
    &HTTP
}

pub(crate) fn capsfilter(name: &str, caps: gst::Caps) -> Result<gst::Element> {
    gst::ElementFactory::make("capsfilter")
        .name(name)
        .property("caps", caps)
        .build()
        .with_context(|| format!("create {name}"))
}

/// Set `audioconvert`'s `mix-matrix`: a row of input gains per output
/// channel. It has to be set before the element negotiates.
pub(crate) fn set_mix_matrix(aconv: &gst::Element, rows: &[Vec<f32>]) {
    let matrix = gst::Array::new(
        rows.iter()
            .map(|row| gst::Array::new(row.iter().copied()).to_send_value()),
    );
    aconv.set_property("mix-matrix", matrix);
}

/// Link a new request pad of `tee` to `sink`'s static sink pad.
pub(super) fn link_tee(tee: &gst::Element, sink: &gst::Element) -> Result<()> {
    tee.request_pad_simple("src_%u")
        .with_context(|| format!("{} request pad", tee.name()))?
        .link(
            &sink
                .static_pad("sink")
                .with_context(|| format!("{} sink pad", sink.name()))?,
        )
        .with_context(|| format!("link {} → {}", tee.name(), sink.name()))?;
    Ok(())
}

/// An appsrc for a monitor's stream: live, time format, and a leaky 500 ms
/// queue so a slow encoder only drops frames here. Configured at build time
/// because a base source only adopts its format before it starts.
pub(crate) fn consumer(name: &str) -> gst_app::AppSrc {
    let src = gst_app::AppSrc::builder().name(name).build();
    StreamProducer::configure_consumer(&src);
    src
}

/// A pipeline the monitor feeds at two appsrcs (see [`consumer`]): an
/// output's, a recording leg's or a replay buffer's. Dropping it stops the
/// feed, then the pipeline.
pub(crate) struct ConsumerPipeline {
    pub pipeline: gst::Pipeline,
    pub video_src: gst_app::AppSrc,
    pub audio_src: gst_app::AppSrc,
    /// The first error the pipeline hit, as its bus handler keeps it.
    pub error: FirstError,
    /// Connections to the monitor's producers; dropping one stops the feed.
    links: Vec<ConsumptionLink>,
}

impl ConsumerPipeline {
    /// A pipeline called `name` and its appsrcs, not yet added to it.
    pub fn new(name: &str) -> Self {
        Self {
            pipeline: gst::Pipeline::with_name(name),
            video_src: consumer("video-src"),
            audio_src: consumer("audio-src"),
            error: FirstError::default(),
            links: Vec::new(),
        }
    }

    /// Start taking `monitor`'s streams.
    pub fn connect(&mut self, monitor: &MonitorPipeline) -> Result<()> {
        self.links = monitor.feed(&self.video_src, &self.audio_src)?;
        Ok(())
    }

    /// Link the video appsrc through a new `videoconvert`, then through
    /// `then` (already in the pipeline), in order: the last element linked.
    pub fn video_chain(&self, then: &[&gst::Element]) -> Result<gst::Element> {
        let convert = [make_el("videoconvert", "vconv")?];
        self.chain(self.video_src.upcast_ref(), &convert, then, "video")
    }

    /// Link the audio appsrc through a new `audioconvert → audioresample`,
    /// so `then` can take any format, then through `then` (already in the
    /// pipeline), in order: the last element linked.
    pub fn audio_chain(&self, then: &[&gst::Element]) -> Result<gst::Element> {
        let convert = [
            make_el("audioconvert", "aconv")?,
            make_el("audioresample", "aresample")?,
        ];
        self.chain(self.audio_src.upcast_ref(), &convert, then, "audio")
    }

    fn chain(
        &self,
        src: &gst::Element,
        convert: &[gst::Element],
        then: &[&gst::Element],
        kind: &str,
    ) -> Result<gst::Element> {
        self.pipeline
            .add_many(convert)
            .with_context(|| format!("add {kind} convert"))?;
        let chain: Vec<&gst::Element> = std::iter::once(src)
            .chain(convert)
            .chain(then.iter().copied())
            .collect();
        gst::Element::link_many(chain.iter().copied())
            .with_context(|| format!("link {} {kind}", self.pipeline.name()))?;
        Ok(chain[chain.len() - 1].clone())
    }

    pub fn disconnect(&mut self) {
        self.links.clear();
    }

    /// Stop the feed, then the pipeline.
    pub fn stop(&mut self) {
        self.disconnect();
        let _ = set_state(&self.pipeline, gst::State::Null);
    }

    /// Video frames dropped because the appsrc's 500 ms queue overflowed.
    /// Read from the appsrc, not the `ConsumptionLink`, whose count also
    /// includes keyframe waits.
    pub fn dropped_frames(&self) -> u64 {
        self.video_src.property::<u64>("dropped")
    }
}

impl Drop for ConsumerPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run `pipeline` on the monitor's `clock` and `base_time`, so forwarded
/// timestamps mean the same running time on both sides.
pub(super) fn share_clock(pipeline: &gst::Pipeline, clock: &gst::Clock, base_time: gst::ClockTime) {
    pipeline.use_clock(Some(clock));
    pipeline.set_base_time(base_time);
    pipeline.set_start_time(gst::ClockTime::NONE);
}

/// Clear the delta-unit flag on everything `vtenc_prores` puts out. Every
/// ProRes frame is a keyframe, but it marks them deltas: left as is, the
/// muxer's keyframe table lists almost none of them and splitmuxsink never
/// finds a frame to split at.
pub(super) fn mark_prores_keyframes(venc: &gst::Element) -> Result<()> {
    venc.static_pad("src")
        .context("encoder src pad")?
        .add_probe(gst::PadProbeType::BUFFER, |_, info| {
            if let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() {
                buffer.make_mut().unset_flags(gst::BufferFlags::DELTA_UNIT);
            }
            gst::PadProbeReturn::Ok
        });
    Ok(())
}

/// A 10 s audio queue, so the muxer can hold audio while the video encoder
/// produces its first frames without the appsrc's 500 ms limit dropping it.
pub(super) fn audio_hold_queue() -> Result<gst::Element> {
    gst::ElementFactory::make("queue")
        .name("aq")
        .property("max-size-time", gst::ClockTime::from_seconds(10).nseconds())
        .property("max-size-bytes", 0u32)
        .property("max-size-buffers", 0u32)
        .build()
        .context("create audio queue")
}

/// qtmux fails on PCM that jumps ("timestamps are drifting"): a file
/// source's loop, a dropout. This fills gaps with silence and trims
/// overlaps, from the first sample on.
pub(super) fn pcm_audiorate() -> Result<gst::Element> {
    gst::ElementFactory::make("audiorate")
        .name("arate")
        .property("skip-to-first", true)
        .build()
        .context("create audiorate")
}

pub(super) fn handle_level_message(s: &gst::StructureRef, meter: &AudioMeter) {
    use gstreamer::glib;

    let Ok(peak_arr) = s.get::<glib::ValueArray>("peak") else {
        return;
    };
    let Ok(rms_arr) = s.get::<glib::ValueArray>("rms") else {
        return;
    };

    let channels = peak_arr
        .iter()
        .zip(rms_arr.iter())
        .filter_map(|(p, r)| {
            Some(ChannelLevelDto {
                peak_db: p.get::<f64>().ok()?,
                rms_db: r.get::<f64>().ok()?,
            })
        })
        .collect();

    meter.set(channels);
}
