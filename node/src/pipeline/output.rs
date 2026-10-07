//! Output legs: where a channel's program is sent. Like a recording leg, each
//! runs in its own pipeline fed by the channel's [`MonitorPipeline`]
//! producers, so a failing output (no NDI Runtime, say) fails alone:
//!
//! ```text
//! appsrc(video) → videoconvert ─────────────────┐
//!                                                ndisinkcombiner → ndisink
//! appsrc(audio) → audioconvert → audioresample ─┘
//! ```
//!
//! Outputs run for as long as the channel's monitor does.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tracing::{info, warn};

use super::make_el;
use super::monitor::MonitorPipeline;
use crate::api::types::OutputConfig;

pub struct OutputLeg {
    pipeline: gst::Pipeline,
    /// Connections to the monitor's producers; dropping one stops the feed.
    links: Vec<ConsumptionLink>,
    label: String,
    /// The first error the leg hit; it stops sending after one.
    error: Arc<Mutex<Option<String>>>,
}

impl OutputLeg {
    /// Start sending `monitor`'s program to `config`. `name` is the
    /// channel's, the default for what the output is called.
    pub fn start(monitor: &MonitorPipeline, config: &OutputConfig, name: &str) -> Result<Self> {
        let (pipeline, video_src, audio_src, label) = match config {
            OutputConfig::Ndi { ndi_name } => {
                let ndi_name = ndi_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(name);
                let (p, v, a) = build_ndi(ndi_name)?;
                (p, v, a, format!("NDI {ndi_name}"))
            }
        };
        let error = Arc::new(Mutex::new(None));
        let error_ref = error.clone();
        let error_label = label.clone();
        pipeline
            .bus()
            .context("output pipeline has no bus")?
            .set_sync_handler(move |_, msg| {
                if let gst::MessageView::Error(err) = msg.view() {
                    let mut error = error_ref.lock().unwrap();
                    if error.is_none() {
                        warn!(output = %error_label, error = %err.error(), debug = ?err.debug(), "output failed");
                        *error = Some(err.error().to_string());
                    }
                }
                gst::BusSyncReply::Drop
            });

        // Same clock and base time as the monitor, so the program's
        // timestamps mean the same here.
        if let (Some(clock), Some(base_time)) = monitor.timing() {
            pipeline.use_clock(Some(&clock));
            pipeline.set_base_time(base_time);
            pipeline.set_start_time(gst::ClockTime::NONE);
        }
        if pipeline.set_state(gst::State::Playing).is_err() {
            let reason = error
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "output failed to start".into());
            let _ = pipeline.set_state(gst::State::Null);
            return Err(anyhow!("{label}: {reason}"));
        }
        let links = vec![
            monitor.video.add_consumer(&video_src)?,
            monitor.audio.add_consumer(&audio_src)?,
        ];
        info!(output = %label, "output started");
        Ok(Self {
            pipeline,
            links,
            label,
            error,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for OutputLeg {
    fn drop(&mut self) {
        self.links.clear();
        let _ = self.pipeline.set_state(gst::State::Null);
        info!(output = %self.label, "output stopped");
    }
}

fn consumer(name: &str) -> gst_app::AppSrc {
    let src = gst_app::AppSrc::builder().name(name).build();
    StreamProducer::configure_consumer(&src);
    src
}

/// An NDI sender called `ndi_name` (NDI shows it as `HOST (ndi_name)`).
fn build_ndi(ndi_name: &str) -> Result<(gst::Pipeline, gst_app::AppSrc, gst_app::AppSrc)> {
    let pipeline = gst::Pipeline::with_name(&format!("output-ndi-{ndi_name}"));
    let video_src = consumer("video-src");
    let audio_src = consumer("audio-src");
    let vconv = make_el("videoconvert", "vconv")?;
    let aconv = make_el("audioconvert", "aconv")?;
    let aresample = make_el("audioresample", "aresample")?;
    let combiner = make_el("ndisinkcombiner", "combiner")?;
    let sink = make_el("ndisink", "ndisink")?;
    sink.set_property("ndi-name", ndi_name);
    pipeline
        .add_many([
            video_src.upcast_ref(),
            audio_src.upcast_ref(),
            &vconv,
            &aconv,
            &aresample,
            &combiner,
            &sink,
        ])
        .context("add NDI output")?;
    gst::Element::link_many([video_src.upcast_ref(), &vconv]).context("link NDI video")?;
    vconv
        .link_pads(Some("src"), &combiner, Some("video"))
        .context("link NDI video to combiner")?;
    gst::Element::link_many([audio_src.upcast_ref(), &aconv, &aresample])
        .context("link NDI audio")?;
    aresample
        .link_pads(Some("src"), &combiner, Some("audio"))
        .context("link NDI audio to combiner")?;
    combiner.link(&sink).context("link NDI combiner → sink")?;
    Ok((pipeline, video_src, audio_src))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::api::types::{ChannelConfig, LiveVideoFormat, MonitorSettingsDto};
    use crate::sources::channel::ChannelSource;

    /// A channel's program, received back over NDI. Needs the NDI Runtime.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the NDI Runtime; real-time (~10 s)"]
    async fn ndi_output_is_received() {
        gst::init().unwrap();
        gstndi::plugin_register_static().unwrap();
        let name = format!("cr-test-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let channel = ChannelSource::new(
            "c".into(),
            "c".into(),
            ChannelConfig {
                format: LiveVideoFormat {
                    width: 640,
                    height: 360,
                    fps_num: 30,
                    fps_den: 1,
                },
                audio_channels: 2,
                outputs: vec![],
            },
        );
        let mon = MonitorPipeline::new(
            &channel,
            &MonitorSettingsDto::default(),
            &gst::SystemClock::obtain(),
        )
        .unwrap();
        let leg = OutputLeg::start(
            &mon,
            &OutputConfig::Ndi {
                ndi_name: Some(name.clone()),
            },
            "c",
        )
        .unwrap();

        let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .unwrap()
            .trim()
            .to_uppercase();
        let recv = gst::parse::launch(&format!(
            "ndisrc ndi-name=\"{host} ({name})\" ! ndisrcdemux name=d \
             d.video ! queue ! videoconvert ! appsink name=v sync=false \
             d.audio ! queue ! audioconvert ! appsink name=a sync=false"
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let count = |sink: &str| {
            let n = Arc::new(AtomicU64::new(0));
            let n2 = n.clone();
            recv.by_name(sink)
                .unwrap()
                .static_pad("sink")
                .unwrap()
                .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    n2.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            n
        };
        let (video, audio) = (count("v"), count("a"));
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        let caps = recv
            .by_name("v")
            .unwrap()
            .static_pad("sink")
            .unwrap()
            .current_caps();
        recv.set_state(gst::State::Null).unwrap();

        assert_eq!(leg.error(), None);
        let (v, a) = (video.load(Ordering::Relaxed), audio.load(Ordering::Relaxed));
        println!("received {v} frames, {a} audio buffers, caps {caps:?}");
        assert!(v > 60, "video received: {v}");
        assert!(a > 20, "audio received: {a}");
        let s = caps.unwrap();
        let s = s.structure(0).unwrap();
        assert_eq!(s.get::<i32>("width").unwrap(), 640);
        assert_eq!(s.get::<i32>("height").unwrap(), 360);
        drop(leg);
        mon.stop().unwrap();
    }
}
