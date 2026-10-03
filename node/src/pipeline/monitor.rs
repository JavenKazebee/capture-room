use std::collections::HashMap;
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

/// Opaque handle returned by [`MonitorPipeline::attach_recording_legs`].
/// Must be passed to [`MonitorPipeline::detach_recording`] to stop and clean up.
pub struct RecordingBranch {
    vtee_pad: gst::Pad,
    atee_pad: gst::Pad,
    vq: gst::Element,
    aq: gst::Element,
    elements: Vec<gst::Element>,
    sink_name: String,
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
    /// Keyed by filesink element name (e.g. "sink-r0", "sink-r1").
    /// The bus task fires the sender when the corresponding filesink posts EOS.
    recording_eos: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
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
        let recording_eos: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let pipeline = gst::Pipeline::new();
        // A bin only posts EOS once *every* sink is EOS, and the thumbnail and
        // level sinks never are — so a recording filesink's EOS would be
        // swallowed. With message-forward the bin re-posts each child's EOS
        // wrapped in a "GstBinForwarded" element message (handled below).
        pipeline.set_property("message-forward", true);
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
                        let Some(s) = el.structure() else { continue };
                        if s.name() == "level" {
                            handle_level_message(s, &audio_meter_ref);
                        } else if s.name() == "GstBinForwarded" {
                            // GstBaseSink (including filesink) posts EOS after it
                            // has processed EOS — i.e. once the muxer's final
                            // index has been written to the file.
                            let Ok(inner) = s.get::<gst::Message>("message") else { continue };
                            if let (gst::MessageView::Eos(_), Some(src)) = (inner.view(), inner.src()) {
                                let name = src.name().to_string();
                                if let Some(tx) = recording_eos_ref.lock().unwrap().remove(&name) {
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

    /// Attach one recording branch per `(path, profile)` leg. Element names are
    /// suffixed with `"{tag}-{i}"`, so `tag` must be unique among branches that
    /// can coexist in this pipeline — a stopped session may still be draining
    /// when the next one starts. If any leg fails, the legs already attached
    /// are discarded.
    pub fn attach_recording_legs(
        &self,
        tag: &str,
        legs: &[(impl AsRef<Path>, &RecordingProfile)],
    ) -> Result<Vec<RecordingBranch>> {
        let mut branches = Vec::with_capacity(legs.len());
        for (i, (path, profile)) in legs.iter().enumerate() {
            match self.attach_recording(path.as_ref(), profile, &format!("{tag}-{i}")) {
                Ok(branch) => branches.push(branch),
                Err(e) => {
                    for branch in branches {
                        self.discard_recording(branch);
                    }
                    return Err(e);
                }
            }
        }
        Ok(branches)
    }

    /// vq → [videorate → caps] → [videoscale → caps] → videoconvert → venc ─┐
    ///                                                                     mux → filesink
    /// aq → audioconvert → audioresample → aenc ───────────────────────────┘
    ///
    /// The branch is built, linked and set playing before the tee pads are
    /// linked to it. A freshly requested tee pad carries no data until it is
    /// linked, so no blocking probe is needed — and none can hang waiting for
    /// a stalled source.
    fn attach_recording(
        &self,
        path: &Path,
        profile: &RecordingProfile,
        tag: &str,
    ) -> Result<RecordingBranch> {
        let location = path.to_str().context("output path not valid UTF-8")?;

        // Leaky video queue: if the encoder can't keep up (complex content like
        // the moving ball), vtee's recording pad would otherwise block and stall
        // the entire vtee — and via the muxer's collect-pads, also the atee.
        // With leaky=upstream the oldest frame is dropped instead of blocking,
        // so vtee and atee always keep flowing.
        let vq = gst::ElementFactory::make("queue")
            .name(format!("vq-{tag}"))
            .property("max-size-buffers", 60u32) // ~2 s at 30 fps
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .build()
            .context("create video recording queue")?;
        vq.set_property_from_str("leaky", "upstream");

        let mut video = vec![vq.clone()];
        if let Some((num, den)) = profile.framerate {
            // skip-to-first: otherwise videorate fills from the segment start
            // (pipeline time zero) by duplicating the first frame.
            video.push(
                gst::ElementFactory::make("videorate")
                    .name(format!("vrate-{tag}"))
                    .property("skip-to-first", true)
                    .build()
                    .context("create videorate")?,
            );
            video.push(capsfilter(
                &format!("vrate-caps-{tag}"),
                gst::Caps::builder("video/x-raw")
                    .field("framerate", gst::Fraction::new(num as i32, den as i32))
                    .build(),
            )?);
        }
        if let Some((width, height)) = profile.resolution {
            video.push(make_el("videoscale", &format!("vscale-{tag}"))?);
            // Pin the pixel aspect ratio so videoscale actually resizes instead
            // of satisfying the caps by changing PAR.
            video.push(capsfilter(
                &format!("vscale-caps-{tag}"),
                gst::Caps::builder("video/x-raw")
                    .field("width", width as i32)
                    .field("height", height as i32)
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .build(),
            )?);
        }
        // Format converter before the encoder. Prevents RECONFIGURE events from
        // the encoder propagating upstream to the source (e.g. ndisrc) and
        // handles formats the encoder can't take directly (NDI UYVY → I420).
        video.push(make_el("videoconvert", &format!("vconv-{tag}"))?);
        video.push(build_video_encoder(profile, tag)?);

        // Large audio queue so the muxer can buffer audio while waiting for the
        // first video frames without blocking the atee.
        let aq = gst::ElementFactory::make("queue")
            .name(format!("aq-{tag}"))
            .property("max-size-time", gst::ClockTime::from_seconds(10).nseconds())
            .property("max-size-bytes", 0u32)
            .property("max-size-buffers", 0u32)
            .build()
            .context("create audio recording queue")?;
        let audio = vec![
            aq.clone(),
            make_el("audioconvert", &format!("aconv-{tag}"))?,
            make_el("audioresample", &format!("aresample-{tag}"))?,
            build_audio_encoder(profile, tag)?,
        ];

        let muxer = build_muxer(profile, tag)?;
        let filesink = gst::ElementFactory::make("filesink")
            .name(format!("sink-{tag}"))
            .property("location", location)
            .build()
            .context("create filesink")?;
        let sink_name = filesink.name().to_string();

        let elements: Vec<gst::Element> =
            video.iter().chain(&audio).chain([&muxer, &filesink]).cloned().collect();

        let mut tee_pads: Vec<(gst::Element, gst::Pad)> = Vec::new();
        let linked = (|| -> Result<oneshot::Receiver<()>> {
            self.pipeline.add_many(&elements).context("add recording branch")?;
            gst::Element::link_many(&video).context("link video chain")?;
            gst::Element::link_many(&audio).context("link audio chain")?;
            link_to_muxer(video.last().unwrap(), &muxer, "video", profile)?;
            link_to_muxer(audio.last().unwrap(), &muxer, "audio", profile)?;
            muxer.link(&filesink).context("link mux → filesink")?;

            for el in &elements {
                el.sync_state_with_parent()
                    .map_err(|_| anyhow::anyhow!("sync_state_with_parent failed for {}", el.name()))?;
            }

            // Register before linking so even an immediate EOS is caught.
            let (eos_tx, eos_rx) = oneshot::channel();
            self.recording_eos.lock().unwrap().insert(sink_name.clone(), eos_tx);

            for (tee, queue) in [(&self.vtee, &vq), (&self.atee, &aq)] {
                let pad = tee.request_pad_simple("src_%u").context("request tee src pad")?;
                tee_pads.push((tee.clone(), pad.clone()));
                pad.link(&queue.static_pad("sink").context("queue sink pad")?)
                    .context("link tee → recording queue")?;
            }
            Ok(eos_rx)
        })();

        match linked {
            Ok(eos_rx) => {
                info!(path = ?path, "recording branch attached");
                let [(_, vtee_pad), (_, atee_pad)]: [_; 2] = tee_pads.try_into().unwrap();
                Ok(RecordingBranch { vtee_pad, atee_pad, vq, aq, elements, sink_name, eos_rx })
            }
            Err(e) => {
                self.remove_branch(tee_pads, &elements, &sink_name);
                Err(e)
            }
        }
    }

    /// Tear a branch down immediately, without finalizing its file. Only for
    /// rolling back a start that failed part-way.
    fn discard_recording(&self, branch: RecordingBranch) {
        let tee_pads = vec![(self.vtee.clone(), branch.vtee_pad), (self.atee.clone(), branch.atee_pad)];
        self.remove_branch(tee_pads, &branch.elements, &branch.sink_name);
    }

    fn remove_branch(
        &self,
        tee_pads: Vec<(gst::Element, gst::Pad)>,
        elements: &[gst::Element],
        sink_name: &str,
    ) {
        // Releasing a request pad also unlinks it, which stops data first.
        for (tee, pad) in tee_pads {
            tee.release_request_pad(&pad);
        }
        self.recording_eos.lock().unwrap().remove(sink_name);
        // Downstream first: a muxer blocking its inputs is flushed before the
        // queues' streaming threads (which may be pushing into it) are joined.
        for el in elements.iter().rev() {
            let _ = el.set_state(gst::State::Null);
            let _ = self.pipeline.remove(el);
        }
    }

    /// Detach a recording branch and wait for the file to be fully written.
    ///
    /// The branch is unlinked from both tees, EOS is pushed into it so the
    /// muxer can write its final index, and we wait (up to `timeout`) for the
    /// filesink to post EOS before removing the elements. A timeout at either
    /// step is an error: the file is probably missing its index.
    pub async fn detach_recording(&self, branch: RecordingBranch, timeout: Duration) -> Result<()> {
        let RecordingBranch { vtee_pad, atee_pad, vq, aq, elements, sink_name, eos_rx } = branch;

        let v_unlinked = unlink_when_idle(&vtee_pad, &vq);
        let a_unlinked = unlink_when_idle(&atee_pad, &aq);
        let unlinked = async {
            v_unlinked.await.ok();
            a_unlinked.await.ok();
        };
        if tokio::time::timeout(timeout, unlinked).await.is_err() {
            // The tee's push into this branch is blocked — e.g. the muxer stalled
            // after a failed negotiation and the audio queue filled up. Flushing
            // the branch unblocks the push, but the file can't be finalized.
            warn!(sink = %sink_name, "recording branch stalled, forcing it down");
            self.remove_branch(
                vec![(self.vtee.clone(), vtee_pad), (self.atee.clone(), atee_pad)],
                &elements,
                &sink_name,
            );
            anyhow::bail!("recording stalled and was force-stopped; the file is likely incomplete");
        }
        self.vtee.release_request_pad(&vtee_pad);
        self.atee.release_request_pad(&atee_pad);

        // The bus task signals eos_rx when the filesink posts its EOS message,
        // which GstBaseSink does after processing EOS (i.e. after fclose).
        let result = match tokio::time::timeout(timeout, eos_rx).await {
            Ok(Ok(())) => {
                info!(sink = %sink_name, "recording EOS: file closed cleanly");
                Ok(())
            }
            _ => {
                warn!(sink = %sink_name, "recording EOS timed out after {timeout:?}, forcing NULL");
                Err(anyhow::anyhow!(
                    "recording did not finish writing within {timeout:?}; the file may be incomplete"
                ))
            }
        };

        self.remove_branch(Vec::new(), &elements, &sink_name);
        info!(sink = %sink_name, "recording branch removed");
        result
    }
}

/// Unlink `pad` from `queue` and push EOS into the orphaned branch so it
/// drains through encoder → muxer → filesink. An IDLE probe runs as soon as
/// the tee isn't pushing on this pad — immediately if no data is flowing — so
/// a stalled source can't hang a stop. Resolves once the unlink has happened.
fn unlink_when_idle(pad: &gst::Pad, queue: &gst::Element) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let queue_sink = queue.static_pad("sink");
    pad.add_probe(gst::PadProbeType::IDLE, move |pad, _| {
        if let Some(sink) = &queue_sink {
            let _ = pad.unlink(sink);
            sink.send_event(gst::event::Eos::new());
        }
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        gst::PadProbeReturn::Remove
    });
    rx
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
        .with_context(|| format!("create {factory} (is its GStreamer plugin installed?)"))
}

fn capsfilter(name: &str, caps: gst::Caps) -> Result<gst::Element> {
    gst::ElementFactory::make("capsfilter")
        .name(name)
        .property("caps", caps)
        .build()
        .with_context(|| format!("create {name}"))
}

/// Link an encoder to a new muxer input, with an error that names the
/// combination when the container can't carry the codec.
fn link_to_muxer(
    encoder: &gst::Element,
    muxer: &gst::Element,
    kind: &str,
    profile: &RecordingProfile,
) -> Result<()> {
    let incompatible = || {
        anyhow::anyhow!(
            "{} output can't be muxed into .{} ({kind})",
            encoder.factory().map(|f| f.name().to_string()).unwrap_or_default(),
            profile.file_extension(),
        )
    };
    let sink = muxer.request_pad_simple(&format!("{kind}_%u")).ok_or_else(incompatible)?;
    encoder
        .static_pad("src")
        .context("encoder src pad")?
        .link(&sink)
        .map_err(|_| incompatible())?;
    Ok(())
}

fn build_video_encoder(profile: &RecordingProfile, tag: &str) -> Result<gst::Element> {
    let name = profile.video_encoder_element();
    let venc = make_el(name, &format!("venc-{tag}"))?;
    if let Some(kbps) = profile.bitrate_kbps {
        match name {
            "x264enc" | "x265enc" => set_property(&venc, "bitrate", &kbps.to_string())?,
            "vp9enc" => set_property(&venc, "target-bitrate", &(kbps * 1000).to_string())?,
            _ => {}
        }
    }
    if let Some(prores) = profile.prores_profile() {
        set_property(&venc, "profile", prores)?;
    }
    if name == "x264enc" {
        set_property(&venc, "tune", "zerolatency")?;
    }
    Ok(venc)
}

/// `set_property_from_str` panics on an unknown property; encoder builds
/// differ between platforms, so a missing one is an error instead.
fn set_property(el: &gst::Element, name: &str, value: &str) -> Result<()> {
    if el.find_property(name).is_none() {
        anyhow::bail!("{} has no `{name}` property", el.factory().map(|f| f.name()).unwrap_or_default());
    }
    el.set_property_from_str(name, value);
    Ok(())
}

fn build_audio_encoder(profile: &RecordingProfile, tag: &str) -> Result<gst::Element> {
    make_el(profile.audio_encoder_element(), &format!("aenc-{tag}"))
}

fn build_muxer(profile: &RecordingProfile, tag: &str) -> Result<gst::Element> {
    make_el(profile.muxer_element(), &format!("mux-{tag}"))
}
