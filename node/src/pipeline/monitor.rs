use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::StreamProducer;
use tracing::{error, warn};

use crate::api::types::MonitorSettingsDto;
use crate::sources::InputSource;

use super::{capsfilter, handle_level_message, link_tee, make_el, AudioMeter, ThumbnailStore};

// ── MonitorPipeline ───────────────────────────────────────────────────────────

/// The always-on pipeline for one source: thumbnails, audio levels, and the
/// raw video/audio producers that recordings consume. Recordings run in their
/// own pipelines (see [`super::recording`]), so a failing recording can never
/// stall the source or the other recordings.
pub struct MonitorPipeline {
    pipeline: gst::Pipeline,
    pub thumbnail: ThumbnailStore,
    pub audio_meter: AudioMeter,
    pub video: StreamProducer,
    pub audio: StreamProducer,
    /// Video frames the source has delivered since the monitor started.
    video_frames: Arc<FrameCount>,
    /// The first error the pipeline posted. An errored pipeline has stopped
    /// producing, so this is shown on the source until its monitor restarts.
    error: Arc<Mutex<Option<String>>>,
    /// Reads the bus until aborted on drop: the bus stream never ends by
    /// itself, since the stream keeps the bus alive.
    bus_task: tokio::task::JoinHandle<()>,
    // Live-reconfigurable elements.
    thumb_rate_caps: gst::Element,
    thumb_scale_caps: gst::Element,
    level_el: gst::Element,
}

/// Frames a source has delivered, for telling whether it keeps up. Between
/// two readings, the frames counted against the time their timestamps span
/// (less any gaps the source marked) shows a source falling behind — without
/// the jitter of comparing against the wall clock, since frames arrive in
/// bursts.
#[derive(Debug, Clone, Copy, Default)]
pub struct VideoProgress {
    pub frames: u64,
    /// The last frame's timestamp (running time).
    pub last_pts: Option<gst::ClockTime>,
    /// Total length of the gaps before frames marked `DISCONT` — a looping
    /// file's restart, not a frame lost.
    pub skipped: gst::ClockTime,
}

/// A source's negotiated format; each part `None` until negotiated (or, for
/// the rate, if it's variable).
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceFormat {
    pub size: Option<(u32, u32)>,
    /// (numerator, denominator)
    pub rate: Option<(u32, u32)>,
    /// Channel count, and whether the channels have positions (a channel
    /// mask) rather than being plain numbered channels.
    pub audio: Option<(u32, bool)>,
}

impl MonitorPipeline {
    /// Build and start an always-on monitor pipeline for `source`.
    ///
    /// The pipeline runs immediately: thumbnail frames are produced at the
    /// configured rate and audio levels are metered continuously.
    pub fn new(source: &dyn InputSource, config: &MonitorSettingsDto) -> Result<Self> {
        let thumbnail = ThumbnailStore::default();
        let audio_meter = AudioMeter::default();

        let pipeline = gst::Pipeline::new();
        // Always the system clock: a device in the source bin (an audio
        // interface) would otherwise provide it, and recordings take the
        // monitor's clock.
        pipeline.use_clock(Some(&gst::SystemClock::obtain()));
        let src_bin = source.build_bin()?;
        pipeline.add(&src_bin).context("add source bin")?;

        // ── Tees ──────────────────────────────────────────────────────────────
        let vtee = make_el("tee", "vtee")?;
        let atee = make_el("tee", "atee")?;
        pipeline.add_many([&vtee, &atee]).context("add tees")?;
        for (kind, tee) in [("video", &vtee), ("audio", &atee)] {
            src_bin
                .static_pad(kind)
                .with_context(|| format!("source {kind} pad"))?
                .link(&tee.static_pad("sink").context("tee sink")?)
                .with_context(|| format!("link source {kind} → tee"))?;
        }

        // ── Always-on branches ────────────────────────────────────────────────
        let (thumb_rate_caps, thumb_scale_caps) =
            add_thumbnail_branch(&pipeline, &vtee, thumbnail.clone(), config)?;
        let level_el = add_level_branch(&pipeline, &atee, config)?;
        let video = add_producer_branch(&pipeline, &vtee, "video")?;
        let audio = add_producer_branch(&pipeline, &atee, "audio")?;
        let video_frames = count_buffers(&video)?;

        // ── Bus task ──────────────────────────────────────────────────────────
        let bus = pipeline.bus().context("pipeline has no bus")?;
        let audio_meter_ref = audio_meter.clone();
        let error = Arc::new(Mutex::new(None));
        let error_ref = error.clone();
        let bus_task = tokio::spawn(async move {
            let mut stream = bus.stream();
            while let Some(msg) = stream.next().await {
                match msg.view() {
                    gst::MessageView::Error(err) => {
                        error!(
                            src = %err.src().map(|s| s.name().to_string()).unwrap_or_default(),
                            msg = %err.error(),
                            "monitor pipeline error"
                        );
                        error_ref
                            .lock()
                            .unwrap()
                            .get_or_insert_with(|| err.error().to_string());
                    }
                    gst::MessageView::Warning(w) => {
                        warn!(msg = %w.error(), "monitor pipeline warning");
                    }
                    gst::MessageView::Element(el) => {
                        if let Some(s) = el.structure().filter(|s| s.name() == "level") {
                            handle_level_message(s, &audio_meter_ref);
                        }
                    }
                    _ => {}
                }
            }
        });

        // set_state kicks off the async GStreamer state machine in its own
        // threads. We deliberately do NOT wait on pipeline.state(): that
        // blocks, and new() runs under the SourceManager write lock — blocking
        // would starve the WS emitter. Later errors are reported by the bus task.
        set_state(&pipeline, gst::State::Playing).map_err(|e| anyhow!("set PLAYING: {e:?}"))?;

        Ok(Self {
            pipeline,
            thumbnail,
            audio_meter,
            video,
            audio,
            video_frames,
            error,
            bus_task,
            thumb_rate_caps,
            thumb_scale_caps,
            level_el,
        })
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// Video frames the source has delivered so far.
    pub fn video_frames(&self) -> u64 {
        self.video_frames.frames.load(Ordering::Relaxed)
    }

    /// How far the source's video has got; see [`VideoProgress`].
    pub fn video_progress(&self) -> VideoProgress {
        // A frame can land between these reads, leaving the count one ahead
        // of the timestamp: readers allow a frame of slack.
        let count = &self.video_frames;
        let skipped = count.skipped_ns.load(Ordering::Acquire);
        let pts = count.last_pts.load(Ordering::Acquire);
        let frames = count.frames.load(Ordering::Acquire);
        VideoProgress {
            frames,
            last_pts: (pts != u64::MAX).then(|| gst::ClockTime::from_nseconds(pts)),
            skipped: gst::ClockTime::from_nseconds(skipped),
        }
    }

    pub fn stop(&self) -> Result<()> {
        set_state(&self.pipeline, gst::State::Null)
            .map(|_| ())
            .map_err(|e| anyhow!("set NULL: {e:?}"))
    }

    /// The clock and base time a consumer pipeline should share, so buffer
    /// timestamps mean the same thing on both sides.
    pub fn timing(&self) -> (Option<gst::Clock>, Option<gst::ClockTime>) {
        (self.pipeline.clock(), self.pipeline.base_time())
    }

    /// The format the source is currently producing.
    pub fn source_format(&self) -> SourceFormat {
        let current = |p: &StreamProducer| {
            p.appsink()
                .static_pad("sink")
                .and_then(|pad| pad.current_caps())
        };
        let audio = current(&self.audio).and_then(|caps| {
            let s = caps.structure(0)?;
            let channels = s.get::<i32>("channels").ok().filter(|&c| c > 0)? as u32;
            let mask = s.get::<gst::Bitmask>("channel-mask").map_or(0, |m| m.0);
            // Mono and stereo without a mask have their standard positions.
            Some((channels, mask != 0 || channels <= 2))
        });
        let caps = current(&self.video);
        let Some(s) = caps.as_ref().and_then(|c| c.structure(0)) else {
            return SourceFormat {
                audio,
                ..Default::default()
            };
        };
        let size = match (s.get::<i32>("width"), s.get::<i32>("height")) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
            _ => None,
        };
        let rate = s
            .get::<gst::Fraction>("framerate")
            .ok()
            .filter(|f| f.numer() > 0 && f.denom() > 0)
            .map(|f| (f.numer() as u32, f.denom() as u32));
        SourceFormat { size, rate, audio }
    }

    /// Apply a new config to the running pipeline without restarting it.
    /// The level interval and thumbnail caps are updated in-place; GStreamer
    /// re-negotiates the affected branches within the current pipeline run.
    pub fn reconfigure(&self, config: &MonitorSettingsDto) {
        self.level_el
            .set_property("interval", level_interval_ns(config));
        self.thumb_rate_caps
            .set_property("caps", thumb_rate_caps(config));
        self.thumb_scale_caps
            .set_property("caps", thumb_scale_caps(config));
    }
}

impl Drop for MonitorPipeline {
    fn drop(&mut self) {
        let _ = self.stop();
        self.bus_task.abort();
    }
}

// ── Always-on branch builders ─────────────────────────────────────────────────

/// vtee → queue → videorate → capsfilter(fps) → videoscale
///       → capsfilter(WxH) → videoconvert → jpegenc → appsink
///
/// Returns (fps_capsfilter, scale_capsfilter) for live reconfiguration.
fn add_thumbnail_branch(
    pipeline: &gst::Pipeline,
    vtee: &gst::Element,
    store: ThumbnailStore,
    config: &MonitorSettingsDto,
) -> Result<(gst::Element, gst::Element)> {
    let rate_caps = capsfilter("thumb-rate-caps", thumb_rate_caps(config))?;
    let scale_caps = capsfilter("thumb-scale-caps", thumb_scale_caps(config))?;

    // Not synced to the clock: videorate picks frames by timestamp anyway,
    // and a synced sink holds each frame for the pipeline's latency (about
    // 1 s for live sources). Its queue then fills and blocks the tee, which
    // stalls the producers and makes recordings drop frames.
    let appsink = gst_app::AppSink::builder()
        .name("thumb-sink")
        .caps(&gst::Caps::builder("image/jpeg").build())
        .max_buffers(1)
        .drop(true)
        .sync(false)
        .build();
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_preroll(|_| Ok(gst::FlowSuccess::Ok))
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                store.set(map.to_vec());
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // Leaky, so a slow thumbnail never holds up the other branches.
    let tq = make_el("queue", "tq")?;
    tq.set_property_from_str("leaky", "downstream");
    tq.set_property("max-size-buffers", 2u32);
    let chain = [
        tq,
        make_el("videorate", "thumb-rate")?,
        rate_caps.clone(),
        make_el("videoscale", "thumb-scale")?,
        scale_caps.clone(),
        make_el("videoconvert", "thumb-conv")?,
        make_el("jpegenc", "thumb-enc")?,
        appsink.upcast(),
    ];
    pipeline.add_many(&chain).context("add thumbnail branch")?;
    gst::Element::link_many(&chain).context("link thumbnail branch")?;
    link_tee(vtee, &chain[0])?;

    Ok((rate_caps, scale_caps))
}

/// atee → queue → audioconvert → level → fakesink
///
/// Returns the level element for live reconfiguration.
fn add_level_branch(
    pipeline: &gst::Pipeline,
    atee: &gst::Element,
    config: &MonitorSettingsDto,
) -> Result<gst::Element> {
    let level = make_el("level", "level")?;
    level.set_property("interval", level_interval_ns(config));
    level.set_property("post-messages", true);
    let fakesink = make_el("fakesink", "level-sink")?;
    fakesink.set_property("sync", false);

    let chain = [
        make_el("queue", "lq")?,
        make_el("audioconvert", "level-conv")?,
        level.clone(),
        fakesink,
    ];
    pipeline.add_many(&chain).context("add level branch")?;
    gst::Element::link_many(&chain).context("link level branch")?;
    link_tee(atee, &chain[0])?;

    Ok(level)
}

/// tee → queue → appsink, wrapped in a [`StreamProducer`] that fans buffers
/// out to recording pipelines. A consumer's errors are only logged by the
/// producer — they never travel back up to the tee.
fn add_producer_branch(
    pipeline: &gst::Pipeline,
    tee: &gst::Element,
    kind: &str,
) -> Result<StreamProducer> {
    let queue = make_el("queue", &format!("{kind}-producer-queue"))?;
    let appsink = gst_app::AppSink::builder()
        .name(format!("{kind}-producer"))
        // Forward as soon as buffers arrive; recordings don't render.
        .sync(false)
        .build();
    pipeline
        .add_many([&queue, appsink.upcast_ref()])
        .with_context(|| format!("add {kind} producer"))?;
    queue
        .link(&appsink)
        .with_context(|| format!("link {kind} queue → producer"))?;
    link_tee(tee, &queue)?;
    Ok(StreamProducer::from(&appsink))
}

/// Buffers that have reached a producer; see [`VideoProgress`].
struct FrameCount {
    frames: AtomicU64,
    /// Nanoseconds; `u64::MAX` until a timestamped buffer arrives.
    last_pts: AtomicU64,
    skipped_ns: AtomicU64,
}

/// Change `pipeline`'s state on a thread outside the tokio runtime. Some
/// elements run their own runtime and block on it while changing state
/// (`whipserversrc`), which panics — and aborts, across the FFI boundary —
/// on a thread that's already inside one, as ours are.
fn set_state(
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

/// Count the buffers reaching `producer`'s appsink.
fn count_buffers(producer: &StreamProducer) -> Result<Arc<FrameCount>> {
    let count = Arc::new(FrameCount {
        frames: AtomicU64::new(0),
        last_pts: AtomicU64::new(u64::MAX),
        skipped_ns: AtomicU64::new(0),
    });
    let counter = Arc::clone(&count);
    producer
        .appsink()
        .static_pad("sink")
        .context("producer sink pad")?
        .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            if let Some(pts) = buffer.pts() {
                let prev = counter.last_pts.swap(pts.nseconds(), Ordering::AcqRel);
                if buffer.flags().contains(gst::BufferFlags::DISCONT) && prev != u64::MAX {
                    // The gap, less the frame interval that would be there anyway.
                    let interval = buffer.duration().map_or(0, |d| d.nseconds());
                    let gap = pts.nseconds().saturating_sub(prev).saturating_sub(interval);
                    counter.skipped_ns.fetch_add(gap, Ordering::AcqRel);
                }
            }
            counter.frames.fetch_add(1, Ordering::Release);
            gst::PadProbeReturn::Ok
        });
    Ok(count)
}

fn thumb_rate_caps(config: &MonitorSettingsDto) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("framerate", gst::Fraction::new(config.thumb_fps, 1))
        .build()
}

fn thumb_scale_caps(config: &MonitorSettingsDto) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("width", config.thumb_width)
        .field("height", config.thumb_height)
        .build()
}

fn level_interval_ns(config: &MonitorSettingsDto) -> u64 {
    config.level_interval_ms * 1_000_000
}
