//! WHEP output: a channel's program over WebRTC, to browsers and other WHEP
//! players.
//!
//! Each output runs a `whepserversink` (gst-plugins-rs `webrtcsink` with the
//! WHEP server signaller), whose HTTP server listens on localhost only, on
//! a port of its own. The node's API proxies `/whep/<path>` to it (see
//! `api::whep`), so viewers use one address and port for every channel, and
//! browser pages on other origins can connect. Media goes over ICE (UDP)
//! between the node and each viewer, encoded for each one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Context, Result};
use gstreamer::{self as gst, glib, prelude::*};

use super::make_el;
use super::monitor::MonitorPipeline;
use super::output::{OutputPipeline, OutputSink, PipelineOutput, RunningOutput};
use super::ConsumerPipeline;

/// Elements a WHEP output needs (the libnice plugin for ICE, an Opus
/// encoder for audio).
pub const WHEP_ELEMENTS: &[&str] = &["whepserversink", "webrtcbin", "nicesink", "opusenc"];

/// Mount path → the local port its signaller serves.
static MOUNTS: LazyLock<Mutex<HashMap<String, u16>>> = LazyLock::new(Mutex::default);

/// The local port serving `path` (a mount path, like `/main`), if an output
/// has it.
pub fn port(path: &str) -> Option<u16> {
    MOUNTS.lock().unwrap().get(path).copied()
}

/// Keep `webrtcsink` (which takes the first encoder it finds for a codec)
/// from picking encoders it can't drive: VideoToolbox fails its
/// negotiation, and its bitrate can't be adapted to each viewer. Lowered to
/// no rank, they're left out of its list (and any other autoplugging); the
/// node's recordings and other outputs create them by name, as before. Call
/// before registering the WebRTC plugin: its elements list the encoders
/// once, then.
pub fn hide_unsupported_encoders() {
    for name in ["vtenc_h264", "vtenc_h264_hw", "vtenc_h265", "vtenc_h265_hw"] {
        if let Some(factory) = gst::ElementFactory::find(name) {
            factory.set_rank(gst::Rank::NONE);
        }
    }
}

/// A free port on localhost, for a signaller to listen on.
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).context("find a port for WHEP")?;
    Ok(listener.local_addr()?.port())
}

/// A WHEP endpoint at `/whep<path>` on the node's API port.
pub struct WhepOutput {
    pub path: String,
    pub bitrate_kbps: u32,
}

impl OutputSink for WhepOutput {
    fn label(&self) -> String {
        format!("WHEP {}", self.path)
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        if port(&self.path).is_some() {
            anyhow::bail!("another channel serves {} over WHEP", self.path);
        }
        let port = free_port()?;
        let output = PipelineOutput::start(monitor, self.label(), self.build(port)?)?;
        MOUNTS.lock().unwrap().insert(self.path.clone(), port);
        Ok(Box::new(WhepMount {
            output,
            path: self.path.clone(),
        }))
    }
}

impl WhepOutput {
    fn build(&self, port: u16) -> Result<OutputPipeline> {
        let mut output = OutputPipeline::new(&format!("output-whep-{}", self.path))?;
        let ConsumerPipeline {
            pipeline,
            video_src,
            audio_src,
            ..
        } = &output.consumer;
        let vconv = make_el("videoconvert", "vconv")?;
        let aconv = make_el("audioconvert", "aconv")?;
        let aresample = make_el("audioresample", "aresample")?;
        // Opus in browsers is stereo: a wider program is mixed down.
        let stereo = super::capsfilter(
            "stereo",
            gst::Caps::builder("audio/x-raw")
                .field("channels", 2)
                .build(),
        )?;
        let sink = make_el("whepserversink", "whep")?;
        // H.264 first (hardware encoders, and every browser has it), VP8
        // for players without it.
        let video_caps: gst::Caps = "video/x-h264; video/x-vp8"
            .parse()
            .context("WHEP video caps")?;
        sink.set_property("video-caps", video_caps);
        let bitrate = self.bitrate_kbps.saturating_mul(1000);
        sink.set_property("max-bitrate", bitrate);
        sink.set_property("start-bitrate", bitrate.min(2_000_000));
        // A LAN output: no STUN server to ask.
        sink.set_property("stun-server", None::<String>);
        let signaller = sink.property::<glib::Object>("signaller");
        signaller.set_property("host-addr", format!("http://127.0.0.1:{port}"));
        signaller.set_property("stun-server", None::<String>);

        let receivers = Arc::new(AtomicU32::new(0));
        let added = receivers.clone();
        sink.connect("consumer-added", false, move |_| {
            added.fetch_add(1, Ordering::Relaxed);
            None
        });
        let removed = receivers.clone();
        sink.connect("consumer-removed", false, move |_| {
            let _ = removed.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            });
            None
        });

        pipeline
            .add_many([&vconv, &aconv, &aresample, &stereo, &sink])
            .context("add WHEP output")?;
        let video_pad = sink
            .request_pad_simple("video_%u")
            .context("WHEP video pad")?;
        let audio_pad = sink
            .request_pad_simple("audio_%u")
            .context("WHEP audio pad")?;
        gst::Element::link_many([video_src.upcast_ref(), &vconv]).context("link WHEP video")?;
        vconv
            .static_pad("src")
            .context("videoconvert src pad")?
            .link(&video_pad)
            .context("link WHEP video → sink")?;
        gst::Element::link_many([audio_src.upcast_ref(), &aconv, &aresample, &stereo])
            .context("link WHEP audio")?;
        stereo
            .static_pad("src")
            .context("stereo src pad")?
            .link(&audio_pad)
            .context("link WHEP audio → sink")?;
        output.receivers = Some(receivers);
        Ok(output)
    }
}

/// A running WHEP output, reachable through the API while it runs.
struct WhepMount {
    output: PipelineOutput,
    path: String,
}

impl RunningOutput for WhepMount {
    fn error(&self) -> Option<String> {
        self.output.error()
    }

    fn receivers(&self) -> Option<u32> {
        self.output.receivers()
    }

    fn dropped_frames(&self) -> Option<u64> {
        self.output.dropped_frames()
    }
}

impl Drop for WhepMount {
    fn drop(&mut self) {
        MOUNTS.lock().unwrap().remove(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::api::types::{ChannelConfig, LiveVideoFormat, MonitorSettingsDto};
    use crate::pipeline::output::OutputLeg;
    use crate::sources::channel::ChannelSource;

    /// Change state off the runtime: the WebRTC elements block on their own.
    fn off_runtime(p: &gst::Pipeline, state: gst::State) {
        crate::pipeline::set_state(p, state).unwrap();
    }

    /// A viewer (`whepclientsrc`) watches a 5.1 channel (mixed down to
    /// stereo) through the API's `/whep` route; the output counts it, and
    /// is gone once stopped.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~10 s); run with --ignored"]
    async fn viewer_receives_through_the_api() {
        gst::init().unwrap();
        hide_unsupported_encoders();
        gstrswebrtc::plugin_register_static().unwrap();
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
                audio_channels: 6,
                outputs: vec![],
            },
        );
        let mon = Arc::new(
            MonitorPipeline::new(
                &channel,
                &MonitorSettingsDto::default(),
                &gst::SystemClock::obtain(),
            )
            .unwrap(),
        );
        let leg = OutputLeg::start(
            &mon,
            &WhepOutput {
                path: "/whep-test".into(),
                bitrate_kbps: 2000,
            },
        )
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, crate::api::whep::router::<()>()).await });

        // Answers once the signaller is up (it starts with the stream).
        let endpoint = format!("http://127.0.0.1:{api}/whep/whep-test");
        let http = reqwest::Client::new();
        let mut ready = false;
        for _ in 0..50 {
            let resp = http
                .request(reqwest::Method::OPTIONS, &endpoint)
                .send()
                .await;
            if resp.is_ok_and(|r| r.status().is_success()) {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ready, "the endpoint answers");

        let viewer = gst::Pipeline::new();
        let src = make_el("whepclientsrc", "src").unwrap();
        src.property::<glib::Object>("signaller")
            .set_property("whep-endpoint", &endpoint);
        src.set_property("stun-server", None::<String>);
        viewer.add(&src).unwrap();
        let frames = Arc::new(AtomicU32::new(0));
        let audio = Arc::new(AtomicU32::new(0));
        let (f, a) = (frames.clone(), audio.clone());
        src.connect_pad_added(move |src, pad| {
            let sink = gst::ElementFactory::make("fakesink")
                .property("sync", false)
                .build()
                .unwrap();
            let count = if pad.name().starts_with("video") {
                f.clone()
            } else {
                a.clone()
            };
            sink.static_pad("sink")
                .unwrap()
                .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                    count.fetch_add(1, Ordering::Relaxed);
                    gst::PadProbeReturn::Ok
                });
            let bin = src.parent().unwrap().downcast::<gst::Bin>().unwrap();
            bin.add(&sink).unwrap();
            sink.sync_state_with_parent().unwrap();
            pad.link(&sink.static_pad("sink").unwrap()).unwrap();
        });
        off_runtime(&viewer, gst::State::Playing);

        for _ in 0..100 {
            if frames.load(Ordering::Relaxed) > 30 && audio.load(Ordering::Relaxed) > 10 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let receivers = leg.receivers();
        off_runtime(&viewer, gst::State::Null);
        assert!(
            frames.load(Ordering::Relaxed) > 30,
            "video frames: {}",
            frames.load(Ordering::Relaxed)
        );
        assert!(audio.load(Ordering::Relaxed) > 10, "audio buffers");
        assert_eq!(receivers, Some(1));
        assert_eq!(leg.error(), None);

        drop(leg);
        assert_eq!(port("/whep-test"), None);
        mon.stop().unwrap();
    }
}
