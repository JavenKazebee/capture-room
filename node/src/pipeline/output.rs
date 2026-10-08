//! Output legs: where a channel's program is sent. Like a recording leg, each
//! runs in its own pipeline fed by the channel's [`MonitorPipeline`]
//! producers, so a failing output (no NDI Runtime, say) fails alone. Each
//! output type is an [`OutputSink`], the counterpart of an input source:
//!
//! ```text
//! NDI: appsrc(video) → videoconvert ─────────────────┐
//!                                                     ndisinkcombiner → ndisink
//!      appsrc(audio) → audioconvert → audioresample ─┘
//!
//! SRT: appsrc(video) → videoconvert → H.264 → h264parse ─┐
//!                                                         mpegtsmux → srtsink
//!      appsrc(audio) → audioconvert → audioresample → AAC ┘
//! ```
//!
//! Outputs run for as long as the channel's monitor does.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tracing::{info, warn};

use super::make_el;
use super::monitor::MonitorPipeline;
use super::recording::set_property;
use super::rtsp::{self, RtspOutput};
use crate::api::types::{OutputConfig, SourceCapabilitiesDto};
use crate::sources::stream;

/// One type of output: how a channel's program leaves the node. The
/// counterpart of an [`InputSource`](crate::sources::InputSource). Most run
/// a pipeline of their own for as long as the channel's monitor does (see
/// [`PipelineOutput`]); an RTSP mount builds one per viewer group instead.
pub trait OutputSink {
    /// What the output is called on the channel, like `NDI Studio A`.
    fn label(&self) -> String;

    /// Start sending `monitor`'s program. Stops when the result is dropped.
    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>>;
}

/// An output that's sending.
pub trait RunningOutput: Send + Sync {
    /// Why it stopped sending, if it did.
    fn error(&self) -> Option<String>;

    /// How many receivers are connected, for outputs that can count them.
    fn receivers(&self) -> Option<u32> {
        None
    }
}

/// An output's own pipeline, taking the program at two appsrcs.
pub struct OutputPipeline {
    pipeline: gst::Pipeline,
    video_src: gst_app::AppSrc,
    audio_src: gst_app::AppSrc,
    /// Connected receivers, for outputs that can count them.
    receivers: Option<Arc<AtomicU32>>,
}

/// What a channel's program is, as its outputs need to know it up front.
#[derive(Debug, Clone, Copy)]
pub struct Program {
    /// (numerator, denominator)
    pub fps: (u32, u32),
    pub audio_channels: u32,
}

impl From<&SourceCapabilitiesDto> for Program {
    fn from(caps: &SourceCapabilitiesDto) -> Self {
        Self {
            fps: (caps.max_framerate[0], caps.max_framerate[1]),
            audio_channels: caps.audio_channels,
        }
    }
}

/// The sink `config` describes. `name` is the channel's, the default for
/// what the output is called.
pub fn sink(config: &OutputConfig, name: &str, program: Program) -> Box<dyn OutputSink> {
    match config {
        OutputConfig::Ndi { ndi_name } => Box::new(NdiOutput {
            ndi_name: ndi_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(name)
                .to_string(),
        }),
        OutputConfig::Srt {
            url,
            latency_ms,
            bitrate_kbps,
            passphrase,
        } => Box::new(SrtOutput {
            url: url.trim().to_string(),
            latency_ms: *latency_ms,
            bitrate_kbps: *bitrate_kbps,
            passphrase: passphrase.clone().filter(|p| !p.is_empty()),
            program,
        }),
        OutputConfig::Rtsp { path, bitrate_kbps } => Box::new(RtspOutput {
            path: rtsp::mount_path(path.as_deref(), name),
            bitrate_kbps: *bitrate_kbps,
            program,
        }),
    }
}

/// A channel's output, sending.
pub struct OutputLeg {
    label: String,
    running: Box<dyn RunningOutput>,
}

impl OutputLeg {
    /// Start sending `monitor`'s program to `sink`. Errors don't repeat the
    /// sink's label: the status shows them beside it.
    pub fn start(monitor: &Arc<MonitorPipeline>, sink: &dyn OutputSink) -> Result<Self> {
        let label = sink.label();
        let running = sink.start(monitor)?;
        info!(output = %label, "output started");
        Ok(Self { label, running })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn error(&self) -> Option<String> {
        self.running.error()
    }

    /// How many receivers are connected, if the output counts them.
    pub fn receivers(&self) -> Option<u32> {
        self.running.receivers()
    }
}

impl Drop for OutputLeg {
    fn drop(&mut self) {
        info!(output = %self.label, "output stopped");
    }
}

/// An [`OutputPipeline`] running, fed by the monitor's producers like a
/// recording leg, on the monitor's clock and base time.
pub struct PipelineOutput {
    pipeline: gst::Pipeline,
    /// Connections to the monitor's producers; dropping one stops the feed.
    links: Vec<ConsumptionLink>,
    /// The first error the pipeline hit; it stops sending after one.
    error: Arc<Mutex<Option<String>>>,
    receivers: Option<Arc<AtomicU32>>,
}

impl PipelineOutput {
    fn start(
        monitor: &MonitorPipeline,
        label: String,
        output: OutputPipeline,
    ) -> Result<Box<dyn RunningOutput>> {
        let OutputPipeline {
            pipeline,
            video_src,
            audio_src,
            receivers,
        } = output;
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
            return Err(anyhow!(reason));
        }
        let links = vec![
            monitor.video.add_consumer(&video_src)?,
            monitor.audio.add_consumer(&audio_src)?,
        ];
        Ok(Box::new(Self {
            pipeline,
            links,
            error,
            receivers,
        }))
    }
}

impl RunningOutput for PipelineOutput {
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    fn receivers(&self) -> Option<u32> {
        self.receivers.as_ref().map(|n| n.load(Ordering::Relaxed))
    }
}

impl Drop for PipelineOutput {
    fn drop(&mut self) {
        self.links.clear();
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn consumer(name: &str) -> gst_app::AppSrc {
    let src = gst_app::AppSrc::builder().name(name).build();
    StreamProducer::configure_consumer(&src);
    src
}

// ── NDI ───────────────────────────────────────────────────────────────────────

/// An NDI sender called `ndi_name` (NDI shows it as `HOST (ndi_name)`).
struct NdiOutput {
    ndi_name: String,
}

impl OutputSink for NdiOutput {
    fn label(&self) -> String {
        format!("NDI {}", self.ndi_name)
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        PipelineOutput::start(monitor, self.label(), self.build()?)
    }
}

impl NdiOutput {
    fn build(&self) -> Result<OutputPipeline> {
        let pipeline = gst::Pipeline::with_name(&format!("output-ndi-{}", self.ndi_name));
        let video_src = consumer("video-src");
        let audio_src = consumer("audio-src");
        let vconv = make_el("videoconvert", "vconv")?;
        let aconv = make_el("audioconvert", "aconv")?;
        let aresample = make_el("audioresample", "aresample")?;
        let combiner = make_el("ndisinkcombiner", "combiner")?;
        let sink = make_el("ndisink", "ndisink")?;
        sink.set_property("ndi-name", &self.ndi_name);
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
        Ok(OutputPipeline {
            pipeline,
            video_src,
            audio_src,
            receivers: None,
        })
    }
}

// ── SRT ───────────────────────────────────────────────────────────────────────

/// Most audio channels an SRT output carries: AAC's limit.
pub const SRT_MAX_AUDIO_CHANNELS: u32 = 8;

/// Elements an SRT output needs (and one of the H.264 encoders).
pub const SRT_ELEMENTS: &[&str] = &["srtsink", "mpegtsmux", "h264parse", "avenc_aac"];

/// H.264 encoders for a live stream, best first; the first this node has is
/// used.
pub const LIVE_H264: &[&str] = &["vtenc_h264", "x264enc"];

/// The first live H.264 encoder this node has, and its properties for
/// streaming: low latency, no B-frames, a keyframe every second (so a
/// receiver joining mid-stream waits at most that long).
pub(crate) fn live_h264(
    bitrate_kbps: u32,
    program: Program,
) -> Result<(&'static str, Vec<(&'static str, String)>)> {
    let factory = LIVE_H264
        .iter()
        .copied()
        .find(|f| crate::plugins::has(f))
        .ok_or_else(|| anyhow!("no H.264 encoder (needs x264enc or vtenc_h264)"))?;
    let mut props = vec![("bitrate", bitrate_kbps.to_string())];
    match factory {
        "vtenc_h264" => props.extend([
            ("realtime", "true".to_string()),
            ("allow-frame-reordering", "false".to_string()),
            ("max-keyframe-interval-duration", "1000000000".to_string()),
        ]),
        _ => {
            let (n, d) = program.fps;
            let keyint = (f64::from(n) / f64::from(d.max(1))).round().max(1.0);
            props.extend([
                ("tune", "zerolatency".to_string()),
                ("speed-preset", "veryfast".to_string()),
                ("key-int-max", keyint.to_string()),
            ]);
        }
    }
    Ok((factory, props))
}

/// AAC bitrate for `channels`, in bits per second.
pub(crate) fn aac_bitrate(channels: u32) -> u32 {
    64_000 * channels.max(2)
}

/// An SRT stream: H.264 and AAC in MPEG-TS, with fixed live settings (a
/// keyframe every second, so a receiver joining mid-stream waits at most
/// that long). Listening (no host in the URL) or calling; a caller retries
/// on its own until the other end answers.
struct SrtOutput {
    url: String,
    latency_ms: u32,
    bitrate_kbps: u32,
    passphrase: Option<String>,
    program: Program,
}

impl SrtOutput {
    fn listens(&self) -> bool {
        stream::url_listen_port(&self.url).is_some()
    }

    /// The first live H.264 encoder this node has, set for streaming.
    fn video_encoder(&self) -> Result<gst::Element> {
        let (factory, props) = live_h264(self.bitrate_kbps, self.program)?;
        let venc = make_el(factory, "venc")?;
        for (name, value) in props {
            set_property(&venc, name, &value)?;
        }
        Ok(venc)
    }
}

/// Drop the encoders' output until both have caps, then start video at a
/// keyframe. `mpegtsmux` writes its first program table (PMT) with the
/// streams that have caps, then another when the second gets them; which
/// is first depends on the encoders' start-up. `srtsink` replays that first
/// table to every receiver that connects, so each would see the program
/// change, and many drop a stream when it does.
fn start_together(video: &gst::Element, audio: &gst::Element) -> Result<()> {
    let caps = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);
    for (i, el) in [video, audio].into_iter().enumerate() {
        let caps = caps.clone();
        let is_video = i == 0;
        el.static_pad("src").context("encoder src pad")?.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::EVENT_DOWNSTREAM,
            move |_, info| {
                if let Some(e) = info.event() {
                    if e.type_() == gst::EventType::Caps {
                        caps[i].store(true, Ordering::Relaxed);
                    }
                    return gst::PadProbeReturn::Ok;
                }
                let both = caps.iter().all(|c| c.load(Ordering::Relaxed));
                let keyframe = info
                    .buffer()
                    .is_some_and(|b| !b.flags().contains(gst::BufferFlags::DELTA_UNIT));
                if both && (keyframe || !is_video) {
                    gst::PadProbeReturn::Remove
                } else {
                    gst::PadProbeReturn::Drop
                }
            },
        );
    }
    Ok(())
}

impl OutputSink for SrtOutput {
    fn label(&self) -> String {
        let (host, port) = stream::host_port(&self.url);
        let port = port.map_or(String::new(), |p| format!(":{p}"));
        if self.listens() {
            format!("SRT listening on {port}")
        } else {
            format!("SRT to {host}{port}")
        }
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        PipelineOutput::start(monitor, self.label(), self.build()?)
    }
}

impl SrtOutput {
    fn build(&self) -> Result<OutputPipeline> {
        let pipeline = gst::Pipeline::with_name(&format!("output-srt-{}", self.url));
        let video_src = consumer("video-src");
        let audio_src = consumer("audio-src");
        let vconv = make_el("videoconvert", "vconv")?;
        let venc = self.video_encoder()?;
        let vparse = make_el("h264parse", "vparse")?;
        // Parameter sets with every keyframe, for receivers that join late.
        vparse.set_property("config-interval", -1i32);
        let aconv = make_el("audioconvert", "aconv")?;
        let aresample = make_el("audioresample", "aresample")?;
        let aenc = make_el("avenc_aac", "aenc")?;
        set_property(
            &aenc,
            "bitrate",
            &aac_bitrate(self.program.audio_channels).to_string(),
        )?;
        let mux = make_el("mpegtsmux", "mux")?;
        // 7 TS packets per buffer: one SRT payload (1316 bytes).
        set_property(&mux, "alignment", "7")?;
        let sink = make_el("srtsink", "srtsink")?;
        sink.set_property("uri", stream::uri(&self.url)?);
        set_property(&sink, "latency", &self.latency_ms.to_string())?;
        if let Some(passphrase) = &self.passphrase {
            sink.set_property("passphrase", passphrase);
        }
        // A listener with nobody connected drops the stream rather than
        // holding up the program.
        sink.set_property("wait-for-connection", false);

        let receivers = self.listens().then(|| {
            let n = Arc::new(AtomicU32::new(0));
            let added = n.clone();
            sink.connect("caller-added", false, move |_| {
                added.fetch_add(1, Ordering::Relaxed);
                None
            });
            let removed = n.clone();
            sink.connect("caller-removed", false, move |_| {
                let _ = removed.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    Some(n.saturating_sub(1))
                });
                None
            });
            n
        });

        pipeline
            .add_many([
                video_src.upcast_ref(),
                audio_src.upcast_ref(),
                &vconv,
                &venc,
                &vparse,
                &aconv,
                &aresample,
                &aenc,
                &mux,
                &sink,
            ])
            .context("add SRT output")?;
        gst::Element::link_many([video_src.upcast_ref(), &vconv, &venc, &vparse, &mux])
            .context("link SRT video")?;
        gst::Element::link_many([audio_src.upcast_ref(), &aconv, &aresample, &aenc, &mux])
            .context("link SRT audio")?;
        mux.link(&sink).context("link SRT mux → sink")?;
        start_together(&vparse, &aenc)?;
        Ok(OutputPipeline {
            pipeline,
            video_src,
            audio_src,
            receivers,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::api::types::{ChannelConfig, LiveVideoFormat, MonitorSettingsDto};
    use crate::sources::channel::ChannelSource;

    const PROGRAM: Program = Program {
        fps: (30, 1),
        audio_channels: 2,
    };

    /// A 640x360 channel's program, running.
    fn channel_monitor() -> Arc<MonitorPipeline> {
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
        Arc::new(
            MonitorPipeline::new(
                &channel,
                &MonitorSettingsDto::default(),
                &gst::SystemClock::obtain(),
            )
            .unwrap(),
        )
    }

    /// A receiving pipeline with appsinks `v` (video) and `a` (audio), and
    /// counters of the buffers each gets.
    fn receiver(launch: &str) -> (gst::Pipeline, Arc<AtomicU64>, Arc<AtomicU64>) {
        let recv = gst::parse::launch(launch)
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
        (recv, video, audio)
    }

    /// The size of the video `recv`'s `v` sink got.
    fn received_size(recv: &gst::Pipeline) -> (i32, i32) {
        let caps = recv
            .by_name("v")
            .unwrap()
            .static_pad("sink")
            .unwrap()
            .current_caps()
            .expect("no video caps");
        let s = caps.structure(0).unwrap();
        (s.get("width").unwrap(), s.get("height").unwrap())
    }

    /// An SRT receiver decoding to `v` and `a`. Decoded pads are linked by
    /// name: `gst-launch`-style `d. ! …` links can't tell them apart.
    fn srt_receiver(uri: &str) -> (gst::Pipeline, Arc<AtomicU64>, Arc<AtomicU64>) {
        let (recv, video, audio) = receiver(&format!(
            "srtsrc uri=\"{uri}\" ! tsdemux name=d \
             queue name=vq ! h264parse ! avdec_h264 ! videoconvert ! appsink name=v sync=false \
             queue name=aq ! aacparse ! avdec_aac ! audioconvert ! appsink name=a sync=false"
        ));
        let weak = recv.downgrade();
        recv.by_name("d").unwrap().connect_pad_added(move |_, pad| {
            let Some(recv) = weak.upgrade() else { return };
            // tsdemux names its pads by stream type, before they have caps.
            let is_video = pad.name().starts_with("video");
            let queue = recv.by_name(if is_video { "vq" } else { "aq" }).unwrap();
            // A second pad for a stream would fail here: the program
            // changed (see `start_together`).
            pad.link(&queue.static_pad("sink").unwrap()).unwrap();
        });
        (recv, video, audio)
    }

    fn srt(url: String) -> OutputConfig {
        OutputConfig::Srt {
            url,
            latency_ms: 200,
            bitrate_kbps: 2000,
            passphrase: None,
        }
    }

    /// A channel's program, received back over NDI. Needs the NDI Runtime.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the NDI Runtime; real-time (~10 s)"]
    async fn ndi_output_is_received() {
        gst::init().unwrap();
        gstndi::plugin_register_static().unwrap();
        let name = format!("cr-test-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let mon = channel_monitor();
        let config = OutputConfig::Ndi {
            ndi_name: Some(name.clone()),
        };
        let leg = OutputLeg::start(&mon, sink(&config, "c", PROGRAM).as_ref()).unwrap();

        let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .unwrap()
            .trim()
            .to_uppercase();
        let (recv, video, audio) = receiver(&format!(
            "ndisrc ndi-name=\"{host} ({name})\" ! ndisrcdemux name=d \
             d.video ! queue ! videoconvert ! appsink name=v sync=false \
             d.audio ! queue ! audioconvert ! appsink name=a sync=false"
        ));
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        let size = received_size(&recv);
        recv.set_state(gst::State::Null).unwrap();

        assert_eq!(leg.error(), None);
        let (v, a) = (video.load(Ordering::Relaxed), audio.load(Ordering::Relaxed));
        println!("received {v} frames, {a} audio buffers");
        assert!(v > 60, "video received: {v}");
        assert!(a > 20, "audio received: {a}");
        assert_eq!(size, (640, 360));
        drop(leg);
        mon.stop().unwrap();
    }

    /// An SRT listener output runs with nobody connected, and a receiver
    /// that joins later gets the program (and is counted).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~10 s); run with --ignored"]
    async fn srt_listener_output_is_received() {
        gst::init().unwrap();
        let port = 7751;
        let mon = channel_monitor();
        let leg = OutputLeg::start(
            &mon,
            sink(&srt(format!("srt://:{port}")), "c", PROGRAM).as_ref(),
        )
        .unwrap();
        assert_eq!(leg.label(), format!("SRT listening on :{port}"));
        // Nobody listening yet: the program shouldn't back up or fail.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(leg.error(), None);
        assert_eq!(leg.receivers(), Some(0));

        let (recv, video, audio) = srt_receiver(&format!("srt://127.0.0.1:{port}?mode=caller"));
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;
        let receivers = leg.receivers();
        let size = received_size(&recv);
        recv.set_state(gst::State::Null).unwrap();

        assert_eq!(leg.error(), None);
        let (v, a) = (video.load(Ordering::Relaxed), audio.load(Ordering::Relaxed));
        println!("received {v} frames, {a} audio buffers");
        assert!(v > 100, "video received: {v}");
        assert!(a > 20, "audio received: {a}");
        assert_eq!(size, (640, 360));
        assert_eq!(receivers, Some(1));
        drop(leg);
        mon.stop().unwrap();
    }

    /// An SRT caller output keeps trying until a listener comes up, then
    /// sends to it.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~10 s); run with --ignored"]
    async fn srt_caller_output_waits_for_listener() {
        gst::init().unwrap();
        let port = 7752;
        let mon = channel_monitor();
        let leg = OutputLeg::start(
            &mon,
            sink(&srt(format!("srt://127.0.0.1:{port}")), "c", PROGRAM).as_ref(),
        )
        .unwrap();
        assert_eq!(leg.label(), format!("SRT to 127.0.0.1:{port}"));
        assert_eq!(leg.receivers(), None);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(leg.error(), None);

        let (recv, video, audio) = srt_receiver(&format!("srt://:{port}?mode=listener"));
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;
        recv.set_state(gst::State::Null).unwrap();

        assert_eq!(leg.error(), None);
        let (v, a) = (video.load(Ordering::Relaxed), audio.load(Ordering::Relaxed));
        println!("received {v} frames, {a} audio buffers");
        assert!(v > 100, "video received: {v}");
        assert!(a > 20, "audio received: {a}");
        drop(leg);
        mon.stop().unwrap();
    }

    /// An RTSP receiver decoding to `v` and `a`, over TCP.
    fn rtsp_receiver(url: &str) -> (gst::Pipeline, Arc<AtomicU64>, Arc<AtomicU64>) {
        let (recv, video, audio) = receiver(&format!(
            "rtspsrc name=src location={url} protocols=tcp latency=200 \
             rtph264depay name=vd ! h264parse ! avdec_h264 ! videoconvert ! appsink name=v sync=false \
             rtpmp4gdepay name=ad ! aacparse ! avdec_aac ! audioconvert ! appsink name=a sync=false"
        ));
        let weak = recv.downgrade();
        recv.by_name("src")
            .unwrap()
            .connect_pad_added(move |_, pad| {
                let Some(recv) = weak.upgrade() else { return };
                let caps = pad.current_caps().unwrap();
                let media = caps.structure(0).unwrap().get::<String>("media").unwrap();
                let depay = recv
                    .by_name(if media == "video" { "vd" } else { "ad" })
                    .unwrap();
                pad.link(&depay.static_pad("sink").unwrap()).unwrap();
            });
        (recv, video, audio)
    }

    /// An RTSP mount serves the program to viewers (counted), and goes away
    /// with its output.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~12 s); uses port 8554; run with --ignored"]
    async fn rtsp_output_is_received() {
        gst::init().unwrap();
        let mon = channel_monitor();
        let config = OutputConfig::Rtsp {
            path: Some("cr-test".into()),
            bitrate_kbps: 2000,
        };
        let leg = OutputLeg::start(&mon, sink(&config, "c", PROGRAM).as_ref()).unwrap();
        assert_eq!(leg.label(), "RTSP /cr-test");
        assert_eq!(leg.receivers(), Some(0));

        let url = format!("rtsp://127.0.0.1:{}/cr-test", rtsp::RTSP_PORT);
        let (recv, video, audio) = rtsp_receiver(&url);
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;
        let receivers = leg.receivers();
        let size = received_size(&recv);
        recv.set_state(gst::State::Null).unwrap();

        assert_eq!(leg.error(), None);
        let (v, a) = (video.load(Ordering::Relaxed), audio.load(Ordering::Relaxed));
        println!("received {v} frames, {a} audio buffers");
        assert!(v > 100, "video received: {v}");
        assert!(a > 20, "audio received: {a}");
        assert_eq!(size, (640, 360));
        assert_eq!(receivers, Some(1));

        // Unmounted: a new viewer is refused.
        drop(leg);
        let (recv, video, _) = rtsp_receiver(&url);
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        recv.set_state(gst::State::Null).unwrap();
        assert_eq!(video.load(Ordering::Relaxed), 0);
        mon.stop().unwrap();
    }
}
