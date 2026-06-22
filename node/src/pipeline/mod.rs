pub mod profile;

use std::path::Path;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::audio::{AudioLevelState, AudioMeter, ChannelLevel};
use crate::sources::InputSource;
use crate::thumbnail::ThumbnailStore;
use profile::RecordingProfile;

/// Thumbnail dimensions and rate used for every pipeline.
const THUMB_WIDTH: i32 = 320;
const THUMB_HEIGHT: i32 = 180;
const THUMB_FPS_NUM: i32 = 1;
const THUMB_FPS_DEN: i32 = 1;

/// Audio level message interval (100 ms → ~10 readings/sec).
const LEVEL_INTERVAL_NS: u64 = 100_000_000;

#[derive(Debug)]
pub enum PipelineEnd {
    Eos,
    Error(String),
}

/// A single active recording pipeline.
///
/// Owns the GStreamer pipeline, thumbnail store, and audio meter.
/// Call `start()` to begin recording, `stop()` to flush and close the file.
pub struct Pipeline {
    inner: gst::Pipeline,
    eos_rx: oneshot::Receiver<PipelineEnd>,
    _bus_task: tokio::task::JoinHandle<()>,
    pub thumbnail: ThumbnailStore,
    pub audio_meter: AudioMeter,
}

impl Pipeline {
    /// Build a pipeline ready to record.
    ///
    /// `secondary` is an optional `(output_path, profile)` pair for a
    /// simultaneous proxy/redundant output at a different encode setting.
    pub fn new(
        source: &dyn InputSource,
        primary_path: &Path,
        primary_profile: &RecordingProfile,
        secondary: Option<(&Path, &RecordingProfile)>,
    ) -> Result<Self> {
        let thumbnail = ThumbnailStore::new();
        let audio_meter = AudioMeter::new();

        let pipeline = build_pipeline(
            source,
            primary_path,
            primary_profile,
            secondary,
            thumbnail.clone(),
            audio_meter.clone(),
        )?;

        let bus = pipeline.bus().context("pipeline has no bus")?;
        let (eos_tx, eos_rx) = oneshot::channel::<PipelineEnd>();

        let pipeline_ref = pipeline.clone();
        let audio_meter_ref = audio_meter.clone();
        let bus_task = tokio::spawn(async move {
            let pipeline = pipeline_ref;
            let mut stream = bus.stream();
            let mut tx = Some(eos_tx);

            while let Some(msg) = stream.next().await {
                match msg.view() {
                    gst::MessageView::Eos(_) => {
                        info!("pipeline EOS");
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(PipelineEnd::Eos);
                        }
                        break;
                    }
                    gst::MessageView::Error(err) => {
                        let text = format!(
                            "{}: {}",
                            err.src().map(|s| s.name().to_string()).unwrap_or_default(),
                            err.error()
                        );
                        error!(msg = text, "pipeline error");
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(PipelineEnd::Error(text));
                        }
                        break;
                    }
                    gst::MessageView::Warning(w) => {
                        warn!(msg = %w.error(), "pipeline warning");
                    }
                    gst::MessageView::StateChanged(sc) => {
                        if msg
                            .src()
                            .map(|s| s == pipeline.upcast_ref::<gst::Object>())
                            .unwrap_or(false)
                        {
                            info!(old = ?sc.old(), new = ?sc.current(), "pipeline state");
                        }
                    }
                    gst::MessageView::Element(el) => {
                        if let Some(s) = el.structure() {
                            if s.name() == "level" {
                                handle_level_message(s, &audio_meter_ref);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            inner: pipeline,
            eos_rx,
            _bus_task: bus_task,
            thumbnail,
            audio_meter,
        })
    }

    /// Set the pipeline to PLAYING and begin recording.
    pub fn start(&self) -> Result<()> {
        self.inner
            .set_state(gst::State::Playing)
            .map_err(|e| anyhow::anyhow!("set PLAYING: {e:?}"))?;
        // Wait up to 5 s for the async state change to complete.
        // NoPreroll is acceptable for live sources.
        let (res, _cur, _pending) = self
            .inner
            .state(Some(gst::ClockTime::from_seconds(5)));
        match res {
            Ok(_) => {}
            Err(e) => return Err(anyhow::anyhow!("pipeline failed to reach PLAYING: {e:?}")),
        }
        Ok(())
    }

    /// Send EOS, wait for flush, then set pipeline to NULL.
    pub async fn stop(mut self, timeout_secs: u64) -> Result<()> {
        self.inner.send_event(gst::event::Eos::new());

        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

        tokio::select! {
            result = &mut self.eos_rx => {
                match result {
                    Ok(PipelineEnd::Eos) => info!("pipeline stopped cleanly"),
                    Ok(PipelineEnd::Error(e)) => warn!(error = e, "pipeline stopped with error"),
                    Err(_) => warn!("bus task exited before EOS"),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                warn!("EOS timed out after {timeout_secs}s, forcing NULL");
            }
        }

        self.inner
            .set_state(gst::State::Null)
            .map_err(|e| anyhow::anyhow!("set NULL: {e:?}"))?;
        Ok(())
    }
}

// ── Audio level message parsing ───────────────────────────────────────────────

fn handle_level_message(s: &gst::StructureRef, meter: &AudioMeter) {
    use gstreamer::glib;

    let Ok(peak_arr) = s.get::<glib::ValueArray>("peak") else { return };
    let Ok(rms_arr) = s.get::<glib::ValueArray>("rms") else { return };

    let channels = peak_arr
        .iter()
        .zip(rms_arr.iter())
        .filter_map(|(p, r)| {
            Some(ChannelLevel {
                peak_db: p.get::<f64>().ok()?,
                rms_db: r.get::<f64>().ok()?,
            })
        })
        .collect();

    meter.update(AudioLevelState { channels });
}

// ── Pipeline construction ─────────────────────────────────────────────────────

fn build_pipeline(
    source: &dyn InputSource,
    primary_path: &Path,
    primary_profile: &RecordingProfile,
    secondary: Option<(&Path, &RecordingProfile)>,
    thumbnail: ThumbnailStore,
    _audio_meter: AudioMeter, // meter is updated via bus; param kept for symmetry
) -> Result<gst::Pipeline> {
    let pipeline = gst::Pipeline::new();

    let src_bin = source.gst_src_element();
    pipeline.add(&src_bin).context("add source bin")?;

    // ── Video tee ─────────────────────────────────────────────────────────────
    let vtee = make(&pipeline, "tee", "vtee")?;
    src_bin
        .static_pad("video").context("source video pad")?
        .link(&vtee.static_pad("sink").context("vtee sink")?)
        .context("link source video → vtee")?;

    // ── Audio tee ─────────────────────────────────────────────────────────────
    let atee = make(&pipeline, "tee", "atee")?;
    src_bin
        .static_pad("audio").context("source audio pad")?
        .link(&atee.static_pad("sink").context("atee sink")?)
        .context("link source audio → atee")?;

    // ── Primary recording branch ──────────────────────────────────────────────
    add_recording_branch(
        &pipeline, &vtee, &atee, primary_path, primary_profile, "p",
    )?;

    // ── Optional secondary branch ─────────────────────────────────────────────
    if let Some((sec_path, sec_profile)) = secondary {
        add_recording_branch(&pipeline, &vtee, &atee, sec_path, sec_profile, "s")?;
    }

    // ── Thumbnail branch ──────────────────────────────────────────────────────
    add_thumbnail_branch(&pipeline, &vtee, thumbnail)?;

    // ── Audio level branch ────────────────────────────────────────────────────
    add_level_branch(&pipeline, &atee)?;

    Ok(pipeline)
}

/// Attach a full encode → mux → filesink branch to the video and audio tees.
fn add_recording_branch(
    pipeline: &gst::Pipeline,
    vtee: &gst::Element,
    atee: &gst::Element,
    path: &Path,
    profile: &RecordingProfile,
    tag: &str, // "p" for primary, "s" for secondary
) -> Result<()> {
    let location = path.to_str().context("output path not valid UTF-8")?;

    // Video chain: queue → encoder
    let vq = make(pipeline, "queue", &format!("vq-{tag}"))?;
    let venc_name = profile.video_encoder_element()?;
    let venc_builder = gst::ElementFactory::make(venc_name).name(format!("venc-{tag}"));
    let venc = if let Some(kbps) = profile.bitrate_kbps {
        match venc_name {
            "x264enc" | "x265enc" => venc_builder.property("bitrate", kbps),
            "vp9enc" => venc_builder.property("target-bitrate", kbps as i32 * 1000),
            _ => venc_builder,
        }
    } else {
        venc_builder
    }
    .build()
    .with_context(|| format!("create {venc_name}"))?;

    if let Some(idx) = profile.prores_profile_index() {
        venc.set_property("profile", idx);
    }
    // Zero-latency mode: disable look-ahead so the first frame exits the encoder
    // immediately rather than buffering a full GOP (~67 frames default).
    if venc_name == "x264enc" {
        venc.set_property_from_str("tune", "zerolatency");
    }
    pipeline.add(&venc).context("add venc")?;

    // Audio chain: queue → encoder
    let aq = make(pipeline, "queue", &format!("aq-{tag}"))?;
    let aenc_name = profile.audio_encoder_element()?;
    let aenc = gst::ElementFactory::make(aenc_name)
        .name(format!("aenc-{tag}"))
        .build()
        .with_context(|| format!("create {aenc_name}"))?;
    pipeline.add(&aenc).context("add aenc")?;

    // Muxer + filesink
    let mux_name = profile.muxer_element();
    let muxer = gst::ElementFactory::make(mux_name)
        .name(format!("mux-{tag}"))
        .build()
        .with_context(|| format!("create {mux_name}"))?;
    let filesink = gst::ElementFactory::make("filesink")
        .name(format!("sink-{tag}"))
        .property("location", location)
        .build()
        .context("create filesink")?;

    pipeline.add(&muxer).context("add muxer")?;
    pipeline.add(&filesink).context("add filesink")?;

    // Link video branch
    vq.link(&venc).context("link vq → venc")?;
    venc.static_pad("src").context("venc src")?
        .link(
            &muxer.request_pad_simple("video_%u").context("mux video pad")?,
        )
        .context("link venc → mux video")?;

    // Link audio branch
    aq.link(&aenc).context("link aq → aenc")?;
    aenc.static_pad("src").context("aenc src")?
        .link(
            &muxer.request_pad_simple("audio_%u").context("mux audio pad")?,
        )
        .context("link aenc → mux audio")?;

    // Muxer → filesink
    muxer.link(&filesink).context("link mux → filesink")?;

    // Tap tees
    vtee.request_pad_simple("src_%u")
        .context("vtee src pad")?
        .link(&vq.static_pad("sink").context("vq sink")?)
        .context("link vtee → vq")?;

    atee.request_pad_simple("src_%u")
        .context("atee src pad")?
        .link(&aq.static_pad("sink").context("aq sink")?)
        .context("link atee → aq")?;

    Ok(())
}

/// Attach the thumbnail branch to the video tee.
///
/// Pipeline: vtee → queue → videorate → capsfilter(1fps) → videoscale
///           → capsfilter(320×180) → videoconvert → jpegenc → appsink
fn add_thumbnail_branch(
    pipeline: &gst::Pipeline,
    vtee: &gst::Element,
    store: ThumbnailStore,
) -> Result<()> {
    let tq = make(pipeline, "queue", "tq")?;
    let videorate = make(pipeline, "videorate", "thumb-rate")?;

    let rate_caps = gst::ElementFactory::make("capsfilter")
        .name("thumb-rate-caps")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("framerate", gst::Fraction::new(THUMB_FPS_NUM, THUMB_FPS_DEN))
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
                .field("width", THUMB_WIDTH)
                .field("height", THUMB_HEIGHT)
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
    pipeline
        .add(&appsink)
        .context("add thumbnail appsink")?;

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_preroll(|_sink| Ok(gst::FlowSuccess::Ok))
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                store.update(map.to_vec());
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // Link thumbnail chain
    for (src, dst) in [
        (&tq, &videorate),
        (&videorate, &rate_caps),
        (&rate_caps, &videoscale),
        (&videoscale, &scale_caps),
        (&scale_caps, &vconv),
        (&vconv, &jpegenc),
    ] {
        src.link(dst).with_context(|| {
            format!("link {} → {}", src.name(), dst.name())
        })?;
    }
    jpegenc
        .link(&appsink)
        .context("link jpegenc → appsink")?;

    // Tap video tee
    vtee.request_pad_simple("src_%u")
        .context("vtee thumb pad")?
        .link(&tq.static_pad("sink").context("tq sink")?)
        .context("link vtee → tq")?;

    Ok(())
}

/// Attach the audio level metering branch to the audio tee.
///
/// Pipeline: atee → queue → audioconvert → level → fakesink
/// The `level` element posts "level" messages on the bus at LEVEL_INTERVAL_NS.
fn add_level_branch(pipeline: &gst::Pipeline, atee: &gst::Element) -> Result<()> {
    let lq = make(pipeline, "queue", "lq")?;
    let aconv = make(pipeline, "audioconvert", "level-conv")?;

    let level = gst::ElementFactory::make("level")
        .name("level")
        .property("interval", LEVEL_INTERVAL_NS)
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

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a named element and add it to the pipeline.
fn make(pipeline: &gst::Pipeline, factory: &str, name: &str) -> Result<gst::Element> {
    let el = gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("create {factory}"))?;
    pipeline.add(&el).with_context(|| format!("add {name}"))?;
    Ok(el)
}
