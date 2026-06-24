use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::audio::AudioMeter;
use crate::sources::InputSource;
use crate::thumbnail::ThumbnailStore;

use super::{handle_level_message, make};
use super::profile::RecordingProfile;

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

// ── Recording branch ──────────────────────────────────────────────────────────

/// Opaque handle returned by [`MonitorPipeline::attach_recording`].
/// Must be passed to [`MonitorPipeline::detach_recording`] to stop and clean up.
pub struct RecordingBranch {
    vtee_pad: gst::Pad,
    atee_pad: gst::Pad,
    vq: gst::Element,
    aq: gst::Element,
    all_elements: Vec<gst::Element>,
    /// Fires when the filesink posts its EOS bus message (file fully written).
    eos_rx: oneshot::Receiver<()>,
}

// ── MonitorPipeline ───────────────────────────────────────────────────────────

pub struct MonitorPipeline {
    pipeline: gst::Pipeline,
    src_bin: gst::Element,
    vtee: gst::Element,
    atee: gst::Element,
    pub thumbnail: ThumbnailStore,
    pub audio_meter: AudioMeter,
    /// The bus task writes here when the recording filesink posts EOS.
    recording_eos: Arc<Mutex<Option<oneshot::Sender<()>>>>,
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
        let recording_eos: Arc<Mutex<Option<oneshot::Sender<()>>>> =
            Arc::new(Mutex::new(None));

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

        // ── Bus task ──────────────────────────────────────────────────────────
        let bus = pipeline.bus().context("pipeline has no bus")?;
        let audio_meter_ref = audio_meter.clone();
        let recording_eos_ref = Arc::clone(&recording_eos);
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
                        if let Some(s) = el.structure() {
                            if s.name() == "level" {
                                handle_level_message(s, &audio_meter_ref);
                            }
                        }
                    }
                    // GstBaseSink (including filesink) posts an EOS message on the
                    // bus after processing EOS — i.e. after the file is closed.
                    gst::MessageView::Eos(_) => {
                        if let Some(src) = msg.src() {
                            // The recording filesink is always named "sink-r".
                            if src.name() == "sink-r" {
                                if let Some(tx) =
                                    recording_eos_ref.lock().unwrap().take()
                                {
                                    let _ = tx.send(());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        let monitor = Self {
            pipeline,
            src_bin,
            vtee,
            atee,
            thumbnail,
            audio_meter,
            recording_eos,
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

    // ── Dynamic recording branch attach / detach ──────────────────────────────

    /// Dynamically attach a recording branch to the running monitor pipeline.
    ///
    /// Uses blocking pad probes on both tee src pads so the link happens
    /// atomically with respect to data flow — no frames are lost or duplicated.
    pub async fn attach_recording(
        &self,
        path: &Path,
        profile: &RecordingProfile,
    ) -> Result<RecordingBranch> {
        let location = path.to_str().context("output path not valid UTF-8")?;
        let tag = "r";

        // ── Build branch elements ─────────────────────────────────────────────
        // Leaky video queue: if x264enc can't keep up (complex content like moving
        // ball), vtee's recording pad would otherwise block and stall the entire
        // vtee — and via the muxer's collect-pads, also the atee.  With
        // leaky=upstream the oldest frame is silently dropped instead of blocking,
        // so vtee and atee always keep flowing.
        let vq = gst::ElementFactory::make("queue")
            .name(format!("vq-{tag}"))
            .property("max-size-buffers", 60u32) // ~2 s at 30 fps
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .build()
            .context("create video recording queue")?;
        vq.set_property_from_str("leaky", "upstream");
        // Format converter before the video encoder. Prevents RECONFIGURE events
        // from x264enc propagating upstream to the source (e.g. ndisrc) and
        // handles sources that provide a format the encoder can't accept directly
        // (e.g. NDI UYVY → x264enc I420).
        let vconv = make_el("videoconvert", &format!("vconv-{tag}"))?;
        let venc = build_video_encoder(profile, tag)?;
        // Large audio queue so the muxer can buffer audio while waiting for the
        // first video frames without blocking the atee.
        let aq = gst::ElementFactory::make("queue")
            .name(format!("aq-{tag}"))
            .property("max-size-time", gst::ClockTime::from_seconds(10).nseconds())
            .property("max-size-bytes", 0u32)
            .property("max-size-buffers", 0u32)
            .build()
            .context("create audio recording queue")?;
        // Format/rate converters before the audio encoder for the same reason.
        let aconv = make_el("audioconvert", &format!("aconv-{tag}"))?;
        let aresample = make_el("audioresample", &format!("aresample-{tag}"))?;
        let aenc = build_audio_encoder(profile, tag)?;
        let muxer = build_muxer(profile, tag)?;
        let filesink = gst::ElementFactory::make("filesink")
            .name(format!("sink-{tag}"))
            .property("location", location)
            .build()
            .context("create filesink")?;

        let all_elements = vec![
            vq.clone(),
            vconv.clone(),
            venc.clone(),
            aq.clone(),
            aconv.clone(),
            aresample.clone(),
            aenc.clone(),
            muxer.clone(),
            filesink.clone(),
        ];

        // ── Add to pipeline ───────────────────────────────────────────────────
        for el in &all_elements {
            self.pipeline
                .add(el)
                .with_context(|| format!("add {} to pipeline", el.name()))?;
        }

        // ── Link within branch (not to tees yet) ──────────────────────────────
        vq.link(&vconv).context("link vq → vconv")?;
        vconv.link(&venc).context("link vconv → venc")?;
        venc.static_pad("src")
            .context("venc src pad")?
            .link(&muxer.request_pad_simple("video_%u").context("mux video pad")?)
            .context("link venc → mux video")?;

        aq.link(&aconv).context("link aq → aconv")?;
        aconv.link(&aresample).context("link aconv → aresample")?;
        aresample.link(&aenc).context("link aresample → aenc")?;
        aenc.static_pad("src")
            .context("aenc src pad")?
            .link(&muxer.request_pad_simple("audio_%u").context("mux audio pad")?)
            .context("link aenc → mux audio")?;

        muxer.link(&filesink).context("link mux → filesink")?;

        // ── Sync branch to pipeline state ─────────────────────────────────────
        // Elements are ready to receive data but not yet connected to the tees.
        for el in &all_elements {
            el.sync_state_with_parent()
                .map_err(|_| anyhow::anyhow!("sync_state_with_parent failed for {}", el.name()))?;
        }

        // ── Register EOS receiver before attaching (avoids race with fast EOS) ─
        let (eos_tx, eos_rx) = oneshot::channel::<()>();
        *self.recording_eos.lock().unwrap() = Some(eos_tx);

        // ── Request tee src pads ──────────────────────────────────────────────
        let vtee_pad = self
            .vtee
            .request_pad_simple("src_%u")
            .context("request vtee src pad")?;
        let atee_pad = self
            .atee
            .request_pad_simple("src_%u")
            .context("request atee src pad")?;

        // ── Atomically link vtee → vq via blocking probe ──────────────────────
        let (v_tx, v_rx) = oneshot::channel::<()>();
        let v_tx = Arc::new(Mutex::new(Some(v_tx)));
        let vq_sink = vq.static_pad("sink").context("vq sink pad")?;
        vtee_pad.add_probe(gst::PadProbeType::BLOCK_DOWNSTREAM, move |pad, _| {
            let _ = pad.link(&vq_sink);
            if let Some(tx) = v_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            gst::PadProbeReturn::Remove
        });

        // ── Atomically link atee → aq via blocking probe ──────────────────────
        let (a_tx, a_rx) = oneshot::channel::<()>();
        let a_tx = Arc::new(Mutex::new(Some(a_tx)));
        let aq_sink = aq.static_pad("sink").context("aq sink pad")?;
        atee_pad.add_probe(gst::PadProbeType::BLOCK_DOWNSTREAM, move |pad, _| {
            let _ = pad.link(&aq_sink);
            if let Some(tx) = a_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            gst::PadProbeReturn::Remove
        });

        // Wait for both probes to fire before declaring the branch live.
        v_rx.await.ok();
        a_rx.await.ok();

        info!(path = ?path, "recording branch attached");

        Ok(RecordingBranch {
            vtee_pad,
            atee_pad,
            vq,
            aq,
            all_elements,
            eos_rx,
        })
    }

    /// Detach a recording branch and wait for the file to be fully written.
    ///
    /// Blocking pad probes unlink the branch from both tees, EOS is pushed into
    /// the orphaned branch so the muxer can write its final index, and we wait
    /// for the filesink to post its EOS bus message before removing elements.
    pub async fn detach_recording(
        &self,
        branch: RecordingBranch,
        timeout_secs: u64,
    ) -> Result<()> {
        let RecordingBranch {
            vtee_pad,
            atee_pad,
            vq,
            aq,
            all_elements,
            eos_rx,
        } = branch;

        // ── Unlink vtee pad, push EOS into video branch ───────────────────────
        let (v_tx, v_rx) = oneshot::channel::<()>();
        let v_tx = Arc::new(Mutex::new(Some(v_tx)));
        let vq_for_probe = vq.clone();
        let vtee = self.vtee.clone();
        vtee_pad.add_probe(gst::PadProbeType::BLOCK_DOWNSTREAM, move |pad, _| {
            if let Some(sink) = vq_for_probe.static_pad("sink") {
                let _ = pad.unlink(&sink);
                // Push EOS directly into the unlinked queue so it drains through
                // encoder → muxer → filesink.
                sink.send_event(gst::event::Eos::new());
            }
            vtee.release_request_pad(pad);
            if let Some(tx) = v_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            gst::PadProbeReturn::Remove
        });

        // ── Unlink atee pad, push EOS into audio branch ───────────────────────
        let (a_tx, a_rx) = oneshot::channel::<()>();
        let a_tx = Arc::new(Mutex::new(Some(a_tx)));
        let aq_for_probe = aq.clone();
        let atee = self.atee.clone();
        atee_pad.add_probe(gst::PadProbeType::BLOCK_DOWNSTREAM, move |pad, _| {
            if let Some(sink) = aq_for_probe.static_pad("sink") {
                let _ = pad.unlink(&sink);
                sink.send_event(gst::event::Eos::new());
            }
            atee.release_request_pad(pad);
            if let Some(tx) = a_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            gst::PadProbeReturn::Remove
        });

        // Wait for both pads to be unlinked.
        v_rx.await.ok();
        a_rx.await.ok();

        // ── Wait for the file to be written ───────────────────────────────────
        // The bus task signals eos_rx when the filesink posts its EOS message,
        // which GstBaseSink does after processing EOS (i.e. after fclose).
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
        tokio::select! {
            _ = eos_rx => {
                info!("recording EOS: file closed cleanly");
            }
            _ = tokio::time::sleep_until(deadline) => {
                warn!("recording EOS timed out after {timeout_secs}s, forcing NULL");
            }
        }

        // ── Remove branch elements from pipeline ──────────────────────────────
        for el in &all_elements {
            let _ = el.set_state(gst::State::Null);
            let _ = self.pipeline.remove(el);
        }

        info!("recording branch removed");
        Ok(())
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

// ── Recording branch element builders ─────────────────────────────────────────

/// Build an element without adding it to any pipeline.
fn make_el(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("create {factory}"))
}

fn build_video_encoder(profile: &RecordingProfile, tag: &str) -> Result<gst::Element> {
    let name = profile.video_encoder_element()?;
    let builder = gst::ElementFactory::make(name).name(format!("venc-{tag}"));
    let venc = if let Some(kbps) = profile.bitrate_kbps {
        match name {
            "x264enc" | "x265enc" => builder.property("bitrate", kbps),
            "vp9enc" => builder.property("target-bitrate", kbps as i32 * 1000),
            _ => builder,
        }
    } else {
        builder
    }
    .build()
    .with_context(|| format!("create {name}"))?;

    if let Some(idx) = profile.prores_profile_index() {
        venc.set_property("profile", idx);
    }
    if name == "x264enc" {
        venc.set_property_from_str("tune", "zerolatency");
    }
    Ok(venc)
}

fn build_audio_encoder(profile: &RecordingProfile, tag: &str) -> Result<gst::Element> {
    let name = profile.audio_encoder_element()?;
    gst::ElementFactory::make(name)
        .name(format!("aenc-{tag}"))
        .build()
        .with_context(|| format!("create {name}"))
}

fn build_muxer(profile: &RecordingProfile, tag: &str) -> Result<gst::Element> {
    let name = profile.muxer_element();
    gst::ElementFactory::make(name)
        .name(format!("mux-{tag}"))
        .build()
        .with_context(|| format!("create {name}"))
}
