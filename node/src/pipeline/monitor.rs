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

impl MonitorPipeline {
    /// Build and start an always-on monitor pipeline for `source`.
    ///
    /// The pipeline runs immediately: thumbnail frames are produced at the
    /// configured rate and audio levels are metered continuously.
    pub fn new(source: &dyn InputSource, config: &MonitorSettingsDto) -> Result<Self> {
        let thumbnail = ThumbnailStore::default();
        let audio_meter = AudioMeter::default();

        let pipeline = gst::Pipeline::new();
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
                        error_ref.lock().unwrap().get_or_insert_with(|| err.error().to_string());
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
        pipeline.set_state(gst::State::Playing).map_err(|e| anyhow!("set PLAYING: {e:?}"))?;

        Ok(Self {
            pipeline,
            thumbnail,
            audio_meter,
            video,
            audio,
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

    pub fn stop(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Null).map(|_| ()).map_err(|e| anyhow!("set NULL: {e:?}"))
    }

    /// The clock and base time a consumer pipeline should share, so buffer
    /// timestamps mean the same thing on both sides.
    pub fn timing(&self) -> (Option<gst::Clock>, Option<gst::ClockTime>) {
        (self.pipeline.clock(), self.pipeline.base_time())
    }

    /// Apply a new config to the running pipeline without restarting it.
    /// The level interval and thumbnail caps are updated in-place; GStreamer
    /// re-negotiates the affected branches within the current pipeline run.
    pub fn reconfigure(&self, config: &MonitorSettingsDto) {
        self.level_el.set_property("interval", level_interval_ns(config));
        self.thumb_rate_caps.set_property("caps", thumb_rate_caps(config));
        self.thumb_scale_caps.set_property("caps", thumb_scale_caps(config));
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

    let appsink = gst_app::AppSink::builder()
        .name("thumb-sink")
        .caps(&gst::Caps::builder("image/jpeg").build())
        .max_buffers(1)
        .drop(true)
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

    let chain = [
        make_el("queue", "tq")?,
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

    let chain = [make_el("queue", "lq")?, make_el("audioconvert", "level-conv")?, level.clone(), fakesink];
    pipeline.add_many(&chain).context("add level branch")?;
    gst::Element::link_many(&chain).context("link level branch")?;
    link_tee(atee, &chain[0])?;

    Ok(level)
}

/// tee → queue → appsink, wrapped in a [`StreamProducer`] that fans buffers
/// out to recording pipelines. A consumer's errors are only logged by the
/// producer — they never travel back up to the tee.
fn add_producer_branch(pipeline: &gst::Pipeline, tee: &gst::Element, kind: &str) -> Result<StreamProducer> {
    let queue = make_el("queue", &format!("{kind}-producer-queue"))?;
    let appsink = gst_app::AppSink::builder()
        .name(format!("{kind}-producer"))
        // Forward as soon as buffers arrive; recordings don't render.
        .sync(false)
        .build();
    pipeline.add_many([&queue, appsink.upcast_ref()]).with_context(|| format!("add {kind} producer"))?;
    queue.link(&appsink).with_context(|| format!("link {kind} queue → producer"))?;
    link_tee(tee, &queue)?;
    Ok(StreamProducer::from(&appsink))
}

fn thumb_rate_caps(config: &MonitorSettingsDto) -> gst::Caps {
    gst::Caps::builder("video/x-raw").field("framerate", gst::Fraction::new(config.thumb_fps, 1)).build()
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
