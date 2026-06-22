pub mod profile;

use std::path::Path;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, prelude::*};
use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::sources::InputSource;
use profile::RecordingProfile;

/// A single active recording pipeline (source → encode → mux → file).
///
/// Drop or call `stop()` to cleanly flush and close the output file.
pub struct Pipeline {
    inner: gst::Pipeline,
    /// Resolves when the pipeline reaches EOS or hits a fatal error.
    eos_rx: oneshot::Receiver<PipelineEnd>,
    /// Background task watching the GStreamer bus.
    _bus_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub enum PipelineEnd {
    Eos,
    Error(String),
}

impl Pipeline {
    /// Build and return a pipeline ready to record.
    ///
    /// The pipeline is in `READY` state after this call — call `start()` to
    /// begin writing frames.
    pub fn new(
        source: &dyn InputSource,
        output_path: &Path,
        profile: &RecordingProfile,
    ) -> Result<Self> {
        let pipeline = build_pipeline(source, output_path, profile)?;

        let bus = pipeline.bus().context("pipeline has no bus")?;
        let (eos_tx, eos_rx) = oneshot::channel::<PipelineEnd>();

        let pipeline_ref = pipeline.clone();
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
                        let msg = format!(
                            "{}: {}",
                            err.src().map(|s| s.name().to_string()).unwrap_or_default(),
                            err.error()
                        );
                        error!(msg, "pipeline error");
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(PipelineEnd::Error(msg));
                        }
                        break;
                    }
                    gst::MessageView::Warning(warn) => {
                        warn!(
                            msg = %warn.error(),
                            "pipeline warning"
                        );
                    }
                    gst::MessageView::StateChanged(sc) => {
                        if msg
                            .src()
                            .map(|s| s == pipeline.upcast_ref::<gst::Object>())
                            .unwrap_or(false)
                        {
                            info!(
                                old = ?sc.old(),
                                new = ?sc.current(),
                                "pipeline state"
                            );
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
        })
    }

    /// Set the pipeline to PLAYING and begin recording.
    pub fn start(&self) -> Result<()> {
        self.inner
            .set_state(gst::State::Playing)
            .map_err(|e| anyhow::anyhow!("set PLAYING: {e:?}"))?;
        Ok(())
    }

    /// Send EOS downstream, wait for the pipeline to flush and close the file,
    /// then set the pipeline to NULL.
    ///
    /// Times out after `timeout_secs` seconds if EOS never arrives.
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

// ── Pipeline construction ─────────────────────────────────────────────────────

fn build_pipeline(
    source: &dyn InputSource,
    output_path: &Path,
    profile: &RecordingProfile,
) -> Result<gst::Pipeline> {
    let pipeline = gst::Pipeline::new();

    let src_bin = source.gst_src_element();
    pipeline.add(&src_bin).context("add source bin")?;

    // ── Video branch ─────────────────────────────────────────────────────────
    let vqueue = gst::ElementFactory::make("queue")
        .name("vqueue")
        .build()
        .context("create video queue")?;

    let venc_name = profile.video_encoder_element()?;
    let venc_builder = gst::ElementFactory::make(venc_name).name("venc");
    let venc = if let Some(kbps) = profile.bitrate_kbps {
        match venc_name {
            "x264enc" => venc_builder.property("bitrate", kbps),
            "x265enc" => venc_builder.property("bitrate", kbps),
            "vp9enc" => venc_builder.property("target-bitrate", kbps as i32 * 1000),
            _ => venc_builder,
        }
    } else {
        venc_builder
    };
    let venc = venc.build().with_context(|| format!("create {venc_name}"))?;

    if let Some(idx) = profile.prores_profile_index() {
        venc.set_property("profile", idx);
    }

    // ── Audio branch ─────────────────────────────────────────────────────────
    let aqueue = gst::ElementFactory::make("queue")
        .name("aqueue")
        .build()
        .context("create audio queue")?;

    let aenc_name = profile.audio_encoder_element()?;
    let aenc = gst::ElementFactory::make(aenc_name)
        .name("aenc")
        .build()
        .with_context(|| format!("create {aenc_name}"))?;

    // ── Muxer + filesink ─────────────────────────────────────────────────────
    let mux_name = profile.muxer_element();
    let muxer = gst::ElementFactory::make(mux_name)
        .name("muxer")
        .build()
        .with_context(|| format!("create {mux_name}"))?;

    let location = output_path
        .to_str()
        .context("output path is not valid UTF-8")?;
    let filesink = gst::ElementFactory::make("filesink")
        .name("filesink")
        .property("location", location)
        .build()
        .context("create filesink")?;

    // Add all elements to the pipeline
    for el in [&vqueue, &venc, &aqueue, &aenc, &muxer, &filesink] {
        pipeline.add(el).context("add element")?;
    }

    // ── Link video branch ─────────────────────────────────────────────────────
    vqueue.link(&venc).context("link vqueue -> venc")?;

    let mux_video_sink = muxer
        .request_pad_simple("video_%u")
        .context("request muxer video pad")?;
    venc.static_pad("src")
        .context("venc src pad")?
        .link(&mux_video_sink)
        .context("link venc -> muxer video")?;

    // ── Link audio branch ─────────────────────────────────────────────────────
    aqueue.link(&aenc).context("link aqueue -> aenc")?;

    let mux_audio_sink = muxer
        .request_pad_simple("audio_%u")
        .context("request muxer audio pad")?;
    aenc.static_pad("src")
        .context("aenc src pad")?
        .link(&mux_audio_sink)
        .context("link aenc -> muxer audio")?;

    // ── muxer → filesink ──────────────────────────────────────────────────────
    muxer.link(&filesink).context("link muxer -> filesink")?;

    // ── Connect source ghost pads to queues ───────────────────────────────────
    src_bin
        .static_pad("video")
        .context("source video pad")?
        .link(&vqueue.static_pad("sink").context("vqueue sink")?)
        .context("link source video -> vqueue")?;

    src_bin
        .static_pad("audio")
        .context("source audio pad")?
        .link(&aqueue.static_pad("sink").context("aqueue sink")?)
        .context("link source audio -> aqueue")?;

    Ok(pipeline)
}
