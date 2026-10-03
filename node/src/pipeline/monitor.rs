use anyhow::{Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::StreamProducer;
use tracing::{error, warn};

use crate::audio::AudioMeter;
use crate::sources::InputSource;
use crate::thumbnail::ThumbnailStore;

use super::{handle_level_message, make};

// ── Monitor config ────────────────────────────────────────────────────────────

pub struct MonitorConfig {
    pub thumb_width: i32,
    pub thumb_height: i32,
    pub thumb_fps_num: i32,
    pub thumb_fps_den: i32,
    /// GStreamer interval for the `level` element in nanoseconds. 100_000_000 = 10 fps.
    pub level_interval_ns: u64,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            thumb_width: 320,
            thumb_height: 180,
            thumb_fps_num: 1,
            thumb_fps_den: 1,
            level_interval_ns: 100_000_000,
        }
    }
}

// ── MonitorPipeline ───────────────────────────────────────────────────────────

/// The always-on pipeline for one source: thumbnails, audio levels, and the
/// raw video/audio producers that recordings consume. Recordings run in their
/// own pipelines (see [`super::recording`]), so a failing recording can never
/// stall the source or the other recordings.
pub struct MonitorPipeline {
    pipeline: gst::Pipeline,
    src_bin: gst::Element,
    pub thumbnail: ThumbnailStore,
    pub audio_meter: AudioMeter,
    pub video: StreamProducer,
    pub audio: StreamProducer,
    _bus_task: tokio::task::JoinHandle<()>,
    // Live-reconfigurable elements.
    thumb_rate_caps: gst::Element,
    thumb_scale_caps: gst::Element,
    level_el: gst::Element,
}

impl MonitorPipeline {
    /// Build and start an always-on monitor pipeline for `source`.
    ///
    /// The pipeline runs immediately: thumbnail frames are produced at the
    /// configured rate and audio levels are metered continuously.
    pub fn new(source: &dyn InputSource, config: &MonitorConfig) -> Result<Self> {
        let thumbnail = ThumbnailStore::new();
        let audio_meter = AudioMeter::new();

        let pipeline = gst::Pipeline::new();
        let src_bin = source.gst_src_element();
        pipeline.add(&src_bin).context("add source bin")?;

        // ── Video tee ─────────────────────────────────────────────────────────
        let vtee = make(&pipeline, "tee", "vtee")?;
        src_bin
            .static_pad("video")
            .context("source video pad")?
            .link(&vtee.static_pad("sink").context("vtee sink")?)
            .context("link source video → vtee")?;

        // ── Audio tee ─────────────────────────────────────────────────────────
        let atee = make(&pipeline, "tee", "atee")?;
        src_bin
            .static_pad("audio")
            .context("source audio pad")?
            .link(&atee.static_pad("sink").context("atee sink")?)
            .context("link source audio → atee")?;

        // ── Always-on branches ────────────────────────────────────────────────
        let (thumb_rate_caps, thumb_scale_caps) =
            add_thumbnail_branch(&pipeline, &vtee, thumbnail.clone(), config)?;
        let level_el = add_level_branch(&pipeline, &atee, config)?;
        let video = add_producer_branch(&pipeline, &vtee, "video")?;
        let audio = add_producer_branch(&pipeline, &atee, "audio")?;

        // ── Bus task ──────────────────────────────────────────────────────────
        let bus = pipeline.bus().context("pipeline has no bus")?;
        let audio_meter_ref = audio_meter.clone();
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

        let monitor = Self {
            pipeline,
            src_bin,
            thumbnail,
            audio_meter,
            video,
            audio,
            _bus_task: bus_task,
            thumb_rate_caps,
            thumb_scale_caps,
            level_el,
        };

        monitor.start()?;
        Ok(monitor)
    }

    fn start(&self) -> Result<()> {
        // set_state kicks off the async GStreamer state machine in its own
        // threads.  We deliberately do NOT call pipeline.state() here because
        // that is a blocking syscall and start() is called while a write lock
        // on SourceManager is held — blocking would starve the WS emitter.
        // Errors that surface later are reported by the bus task.
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| anyhow::anyhow!("set PLAYING: {e:?}"))?;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Null)
            .map_err(|e| anyhow::anyhow!("set NULL: {e:?}"))?;
        // Explicitly unparent the source bin so the same element can be added
        // to a new pipeline immediately (the bus task may still hold a ref to
        // the old pipeline C object, keeping it alive for a moment longer).
        let _ = self.pipeline.remove(&self.src_bin);
        Ok(())
    }

    /// The clock and base time a consumer pipeline should share, so buffer
    /// timestamps mean the same thing on both sides.
    pub fn timing(&self) -> (Option<gst::Clock>, Option<gst::ClockTime>) {
        (self.pipeline.clock(), self.pipeline.base_time())
    }

    /// Apply a new config to the running pipeline without restarting it.
    /// The level interval and thumbnail caps are updated in-place; GStreamer
    /// re-negotiates the affected branches within the current pipeline run.
    pub fn reconfigure(&self, config: &MonitorConfig) {
        self.level_el.set_property("interval", config.level_interval_ns);

        self.thumb_rate_caps.set_property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("framerate", gst::Fraction::new(config.thumb_fps_num, config.thumb_fps_den))
                .build(),
        );

        self.thumb_scale_caps.set_property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("width", config.thumb_width)
                .field("height", config.thumb_height)
                .build(),
        );
    }

}

impl Drop for MonitorPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        let _ = self.pipeline.remove(&self.src_bin);
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
    config: &MonitorConfig,
) -> Result<(gst::Element, gst::Element)> {
    let tq = make(pipeline, "queue", "tq")?;
    let videorate = make(pipeline, "videorate", "thumb-rate")?;

    let rate_caps = gst::ElementFactory::make("capsfilter")
        .name("thumb-rate-caps")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field(
                    "framerate",
                    gst::Fraction::new(config.thumb_fps_num, config.thumb_fps_den),
                )
                .build(),
        )
        .build()
        .context("create thumb rate capsfilter")?;
    pipeline.add(&rate_caps).context("add thumb rate capsfilter")?;

    let videoscale = make(pipeline, "videoscale", "thumb-scale")?;

    let scale_caps = gst::ElementFactory::make("capsfilter")
        .name("thumb-scale-caps")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("width", config.thumb_width)
                .field("height", config.thumb_height)
                .build(),
        )
        .build()
        .context("create thumb scale capsfilter")?;
    pipeline.add(&scale_caps).context("add thumb scale capsfilter")?;

    let vconv = make(pipeline, "videoconvert", "thumb-conv")?;
    let jpegenc = make(pipeline, "jpegenc", "thumb-enc")?;

    let appsink = gst_app::AppSink::builder()
        .name("thumb-sink")
        .caps(&gst::Caps::builder("image/jpeg").build())
        .max_buffers(1)
        .drop(true)
        .build();
    pipeline.add(&appsink).context("add thumbnail appsink")?;

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_preroll(|_| Ok(gst::FlowSuccess::Ok))
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                store.update(map.to_vec());
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    for (src, dst) in [
        (&tq, &videorate),
        (&videorate, &rate_caps),
        (&rate_caps, &videoscale),
        (&videoscale, &scale_caps),
        (&scale_caps, &vconv),
        (&vconv, &jpegenc),
    ] {
        src.link(dst)
            .with_context(|| format!("link {} → {}", src.name(), dst.name()))?;
    }
    jpegenc.link(&appsink).context("link jpegenc → appsink")?;

    vtee.request_pad_simple("src_%u")
        .context("vtee thumb pad")?
        .link(&tq.static_pad("sink").context("tq sink")?)
        .context("link vtee → tq")?;

    Ok((rate_caps, scale_caps))
}

/// atee → queue → audioconvert → level → fakesink
///
/// Returns the level element for live reconfiguration.
fn add_level_branch(
    pipeline: &gst::Pipeline,
    atee: &gst::Element,
    config: &MonitorConfig,
) -> Result<gst::Element> {
    let lq = make(pipeline, "queue", "lq")?;
    let aconv = make(pipeline, "audioconvert", "level-conv")?;

    let level = gst::ElementFactory::make("level")
        .name("level")
        .property("interval", config.level_interval_ns)
        .property("post-messages", true)
        .build()
        .context("create level")?;
    pipeline.add(&level).context("add level")?;

    let fakesink = gst::ElementFactory::make("fakesink")
        .name("level-sink")
        .property("sync", false)
        .build()
        .context("create level fakesink")?;
    pipeline.add(&fakesink).context("add level fakesink")?;

    lq.link(&aconv).context("link lq → aconv")?;
    aconv.link(&level).context("link aconv → level")?;
    level.link(&fakesink).context("link level → fakesink")?;

    atee.request_pad_simple("src_%u")
        .context("atee level pad")?
        .link(&lq.static_pad("sink").context("lq sink")?)
        .context("link atee → lq")?;

    Ok(level)
}

/// tee → queue → appsink, wrapped in a [`StreamProducer`] that fans buffers
/// out to recording pipelines. A consumer's errors are only logged by the
/// producer — they never travel back up to the tee.
fn add_producer_branch(pipeline: &gst::Pipeline, tee: &gst::Element, kind: &str) -> Result<StreamProducer> {
    let queue = make(pipeline, "queue", &format!("{kind}-producer-queue"))?;
    let appsink = gst_app::AppSink::builder()
        .name(format!("{kind}-producer"))
        // Forward as soon as buffers arrive; recordings don't render.
        .sync(false)
        .build();
    pipeline.add(&appsink).with_context(|| format!("add {kind} producer"))?;
    queue.link(&appsink).with_context(|| format!("link {kind} queue → producer"))?;
    tee.request_pad_simple("src_%u")
        .with_context(|| format!("{kind} tee producer pad"))?
        .link(&queue.static_pad("sink").context("producer queue sink")?)
        .with_context(|| format!("link {kind} tee → producer queue"))?;
    Ok(StreamProducer::from(&appsink))
}
