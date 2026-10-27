//! Output legs: where a channel's program is sent. Each runs in its own
//! pipeline fed by the channel's [`MonitorPipeline`], so a failing output
//! fails alone, for as long as the monitor runs:
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

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_video::{self as gst_video, prelude::*};
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::rtsp::{self, RtspOutput};
use super::whep::WhepOutput;
use super::{make_el, set_property, start_or_reason, ConsumerPipeline};
use crate::api::types::{DisplayIdle, OutputConfig, SourceCapabilitiesDto, VideoFormat};
use crate::display::{DisplayWindow, Displays};
use crate::sources::decklink::DecklinkOutput;
use crate::sources::device::LocalDevices;
use crate::sources::stream;

/// One type of output, the counterpart of an
/// [`InputSource`](crate::sources::InputSource). Most run a
/// [`PipelineOutput`]; an RTSP mount builds one per viewer group instead.
pub trait OutputSink {
    /// What the output is called on the channel, like `NDI Studio A`.
    fn label(&self) -> String;

    /// Start sending `monitor`'s program. Stops when the result is dropped.
    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>>;
}

pub trait RunningOutput: Send + Sync {
    /// Why it stopped sending, if it did.
    fn error(&self) -> Option<String>;

    fn receivers(&self) -> Option<u32> {
        None
    }

    /// More about how it's sending (a display's screen mode).
    fn detail(&self) -> Option<String> {
        None
    }

    /// Whether the channel has something loaded (a display output can hide
    /// while it doesn't). Called often; only changes matter.
    fn set_on_air(&self, _on_air: bool) {}

    /// Frames dropped at a leaky input queue, for outputs that have one.
    fn dropped_frames(&self) -> Option<u64> {
        None
    }
}

/// An output's own pipeline, taking the program at two appsrcs.
pub struct OutputPipeline {
    pub consumer: ConsumerPipeline,
    /// Connected receivers, for outputs that can count them.
    pub receivers: Option<Arc<AtomicU32>>,
}

/// Count the receivers `sink` signals with `added` and `removed`.
pub(super) fn count_receivers(sink: &gst::Element, added: &str, removed: &str) -> Arc<AtomicU32> {
    let n = Arc::new(AtomicU32::new(0));
    let up = n.clone();
    sink.connect(added, false, move |_| {
        up.fetch_add(1, Ordering::Relaxed);
        None
    });
    let down = n.clone();
    sink.connect(removed, false, move |_| {
        let _ = down.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_sub(1))
        });
        None
    });
    n
}

impl OutputPipeline {
    /// A pipeline called `name` with its appsrcs added.
    pub fn new(name: &str) -> Result<Self> {
        let consumer = ConsumerPipeline::new(name);
        consumer
            .pipeline
            .add_many([
                consumer.video_src.upcast_ref::<gst::Element>(),
                consumer.audio_src.upcast_ref(),
            ])
            .context("add output appsrcs")?;
        Ok(Self {
            consumer,
            receivers: None,
        })
    }
}

/// What a channel's program is, as its outputs need to know it up front.
#[derive(Debug, Clone, Copy)]
pub struct Program {
    pub format: VideoFormat,
    pub audio_channels: u32,
}

impl From<&SourceCapabilitiesDto> for Program {
    fn from(caps: &SourceCapabilitiesDto) -> Self {
        Self {
            format: VideoFormat::new(
                caps.max_width,
                caps.max_height,
                (caps.fps_num, caps.fps_den),
            ),
            audio_channels: caps.audio_channels,
        }
    }
}

/// What the node has for outputs to use.
pub struct OutputContext<'a> {
    pub displays: &'a Displays,
    pub devices: &'a Arc<LocalDevices>,
}

/// `config` with the defaults taken from the channel's `name` filled in: two
/// outputs with the same identity send the same thing, so a rename restarts
/// only the outputs named after the channel.
pub fn identity(config: &OutputConfig, name: &str) -> OutputConfig {
    match config {
        OutputConfig::Ndi { ndi_name } => OutputConfig::Ndi {
            ndi_name: Some(ndi_name_or(ndi_name.as_deref(), name)),
        },
        OutputConfig::Rtsp { path, bitrate_kbps } => OutputConfig::Rtsp {
            path: Some(rtsp::mount_path(path.as_deref(), name)),
            bitrate_kbps: *bitrate_kbps,
        },
        OutputConfig::Whep { path, bitrate_kbps } => OutputConfig::Whep {
            path: Some(rtsp::mount_path(path.as_deref(), name)),
            bitrate_kbps: *bitrate_kbps,
        },
        other => other.clone(),
    }
}

fn ndi_name_or(ndi_name: Option<&str>, name: &str) -> String {
    ndi_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(name)
        .to_string()
}

/// The sink `config` describes. `name` is the channel's, the default for
/// what the output is called.
pub fn sink(
    config: &OutputConfig,
    name: &str,
    program: Program,
    context: &OutputContext,
) -> Box<dyn OutputSink> {
    match config {
        OutputConfig::Ndi { ndi_name } => Box::new(NdiOutput {
            ndi_name: ndi_name_or(ndi_name.as_deref(), name),
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
        OutputConfig::Whep { path, bitrate_kbps } => Box::new(WhepOutput {
            path: rtsp::mount_path(path.as_deref(), name),
            bitrate_kbps: *bitrate_kbps,
        }),
        OutputConfig::Decklink { device, mode } => Box::new(DecklinkOutput {
            device: device.clone(),
            mode: mode.clone().filter(|m| !m.is_empty()),
            program,
            devices: context.devices.clone(),
        }),
        OutputConfig::Display {
            display,
            audio_device,
            idle,
        } => Box::new(DisplayOutput {
            display: display.clone(),
            audio_device: audio_device.clone().filter(|d| !d.is_empty()),
            idle: *idle,
            program,
            displays: context.displays.clone(),
            devices: context.devices.clone(),
        }),
    }
}

/// A channel's output, sending.
pub struct OutputLeg {
    label: String,
    running: Box<dyn RunningOutput>,
    #[cfg(test)]
    key: usize,
}

// Not the leg's address: a restarted leg is allocated just after the old one is
// freed, and the allocator can hand it the same address.
#[cfg(test)]
static NEXT_KEY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl OutputLeg {
    /// Start sending `monitor`'s program to `sink`. Errors don't repeat the
    /// sink's label: the status shows them beside it.
    pub fn start(monitor: &Arc<MonitorPipeline>, sink: &dyn OutputSink) -> Result<Self> {
        let label = sink.label();
        let running = sink.start(monitor)?;
        info!(output = %label, "output started");
        Ok(Self {
            label,
            running,
            #[cfg(test)]
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn error(&self) -> Option<String> {
        self.running.error()
    }

    pub fn receivers(&self) -> Option<u32> {
        self.running.receivers()
    }

    pub fn detail(&self) -> Option<String> {
        self.running.detail()
    }

    pub fn set_on_air(&self, on_air: bool) {
        self.running.set_on_air(on_air);
    }

    pub fn dropped_frames(&self) -> Option<u64> {
        self.running.dropped_frames()
    }

    /// Tells running legs apart: a restarted one has a new key.
    #[cfg(test)]
    pub fn key(&self) -> usize {
        self.key
    }
}

impl Drop for OutputLeg {
    fn drop(&mut self) {
        info!(output = %self.label, "output stopped");
    }
}

/// An [`OutputPipeline`] running on the monitor's clock and base time.
pub struct PipelineOutput {
    /// It stops sending after its first error.
    consumer: ConsumerPipeline,
    receivers: Option<Arc<AtomicU32>>,
}

impl PipelineOutput {
    pub fn start(monitor: &MonitorPipeline, label: String, output: OutputPipeline) -> Result<Self> {
        let OutputPipeline {
            mut consumer,
            receivers,
        } = output;
        let error_label = label.clone();
        consumer.watch_errors(move |err| {
            warn!(output = %error_label, error = %err.error(), debug = ?err.debug(), "output failed");
        })?;

        monitor.share_timing(&consumer.pipeline);
        // Off the runtime: the WebRTC elements block on their own.
        start_or_reason(
            &consumer.pipeline,
            &consumer.error,
            "output failed to start",
        )?;
        consumer.connect(monitor)?;
        Ok(Self {
            consumer,
            receivers,
        })
    }
}

impl RunningOutput for PipelineOutput {
    fn error(&self) -> Option<String> {
        self.consumer.error.get()
    }

    fn receivers(&self) -> Option<u32> {
        self.receivers.as_ref().map(|n| n.load(Ordering::Relaxed))
    }

    fn dropped_frames(&self) -> Option<u64> {
        Some(self.consumer.dropped_frames())
    }
}

/// An NDI sender called `ndi_name` (NDI shows it as `HOST (ndi_name)`).
struct NdiOutput {
    ndi_name: String,
}

impl OutputSink for NdiOutput {
    fn label(&self) -> String {
        format!("NDI {}", self.ndi_name)
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        Ok(Box::new(PipelineOutput::start(
            monitor,
            self.label(),
            self.build()?,
        )?))
    }
}

impl NdiOutput {
    fn build(&self) -> Result<OutputPipeline> {
        let output = OutputPipeline::new(&format!("output-ndi-{}", self.ndi_name))?;
        let consumer = &output.consumer;
        let combiner = make_el("ndisinkcombiner", "combiner")?;
        let sink = make_el("ndisink", "ndisink")?;
        sink.set_property("ndi-name", &self.ndi_name);
        consumer
            .pipeline
            .add_many([&combiner, &sink])
            .context("add NDI output")?;
        consumer
            .video_chain(&[], &[])?
            .link_pads(Some("src"), &combiner, Some("video"))
            .context("link NDI video to combiner")?;
        consumer
            .audio_chain(&[], &[])?
            .link_pads(Some("src"), &combiner, Some("audio"))
            .context("link NDI audio to combiner")?;
        combiner.link(&sink).context("link NDI combiner → sink")?;
        Ok(output)
    }
}

/// The video sink display outputs draw into their window with.
pub const DISPLAY_SINK: &str = "glimagesink";

/// The program fullscreen on an attached screen, its audio to an audio
/// output: usually the screen's own HDMI audio, so an HDMI→SDI converter
/// embeds it.
struct DisplayOutput {
    display: String,
    audio_device: Option<String>,
    idle: DisplayIdle,
    program: Program,
    displays: Displays,
    devices: Arc<LocalDevices>,
}

impl OutputSink for DisplayOutput {
    fn label(&self) -> String {
        format!("Display {}", self.display)
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        // Hidden to start with when it hides while idle: the first status
        // tick shows it if something's loaded.
        let visible = self.idle == DisplayIdle::Black;
        let window = self
            .displays
            .open(&self.display, self.program.format, visible)?;
        let output = self.build(window.handle)?;
        let pipeline = PipelineOutput::start(monitor, self.label(), output)?;
        Ok(Box::new(DisplayRunning {
            pipeline,
            window,
            idle: self.idle,
            visible: AtomicBool::new(visible),
        }))
    }
}

impl DisplayOutput {
    fn build(&self, window: usize) -> Result<OutputPipeline> {
        let output = OutputPipeline::new(&format!("output-display-{}", self.display))?;
        // The sink converts on the GPU: no videoconvert, which would cost a
        // CPU pass per frame (a lot at 4K).
        let vsink = make_el(DISPLAY_SINK, "vsink")?;
        set_property(&vsink, "force-aspect-ratio", "true")?;
        let overlay = vsink
            .dynamic_cast_ref::<gst_video::VideoOverlay>()
            .context("the display sink can't draw into a window")?;
        // SAFETY: the window outlives the pipeline (`DisplayRunning` drops
        // the pipeline first).
        unsafe { overlay.set_window_handle(window) };
        let asink = match &self.audio_device {
            Some(key) => self.devices.output(key)?.0,
            // No audio: the program's is consumed and dropped.
            None => make_el("fakesink", "asink")?,
        };
        let consumer = &output.consumer;
        consumer
            .pipeline
            .add_many([&vsink, &asink])
            .context("add display output")?;
        consumer
            .video_src
            .link(&vsink)
            .context("link display video")?;
        consumer.audio_chain(&[], &[&asink])?;
        Ok(output)
    }
}

struct DisplayRunning {
    // Dropped first: the sink stops drawing before the window closes.
    pipeline: PipelineOutput,
    window: DisplayWindow,
    idle: DisplayIdle,
    visible: AtomicBool,
}

impl RunningOutput for DisplayRunning {
    fn error(&self) -> Option<String> {
        if !self.window.connected() {
            return Some(
                "the screen is unplugged; the output comes back when it's plugged in".into(),
            );
        }
        self.pipeline.error()
    }

    fn detail(&self) -> Option<String> {
        Some(self.window.mode.clone())
    }

    fn set_on_air(&self, on_air: bool) {
        let visible = on_air || self.idle == DisplayIdle::Black;
        if self.visible.swap(visible, Ordering::Relaxed) != visible {
            self.window.set_visible(visible);
        }
    }

    fn dropped_frames(&self) -> Option<u64> {
        self.pipeline.dropped_frames()
    }
}

/// Most audio channels an SRT output carries: AAC's limit.
pub const SRT_MAX_AUDIO_CHANNELS: u32 = 8;

/// Elements an SRT output needs (and one of the H.264 encoders).
pub const SRT_ELEMENTS: &[&str] = &["srtsink", "mpegtsmux", "h264parse", "avenc_aac"];

/// H.264 encoders for a live stream, best first.
pub const LIVE_H264: &[&str] = &[
    "vtenc_h264",
    "vah264enc",
    "amfh264enc",
    "mfh264enc",
    "x264enc",
];

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
        .ok_or_else(|| anyhow!("no H.264 encoder (needs {})", LIVE_H264.join(" or ")))?;
    let mut props = vec![("bitrate", bitrate_kbps.to_string())];
    let keyint = program.format.fps().round().max(1.0);
    match factory {
        "vtenc_h264" => props.extend([
            ("realtime", "true".to_string()),
            ("allow-frame-reordering", "false".to_string()),
            ("max-keyframe-interval-duration", "1000000000".to_string()),
        ]),
        "vah264enc" => props.extend([
            ("rate-control", "cbr".to_string()),
            ("b-frames", "0".to_string()),
            ("key-int-max", keyint.to_string()),
        ]),
        // B-frames are off by default in both.
        "amfh264enc" => props.extend([
            ("usage", "low-latency".to_string()),
            ("rate-control", "cbr".to_string()),
            ("gop-size", keyint.to_string()),
        ]),
        // mfh264enc has only the properties the GPU's encoder supports.
        "mfh264enc" => {
            let probe = gst::ElementFactory::make(factory).build()?;
            props.extend(
                [
                    ("rc-mode", "cbr".to_string()),
                    ("low-latency", "true".to_string()),
                    ("gop-size", keyint.to_string()),
                ]
                .into_iter()
                .filter(|(name, _)| probe.find_property(name).is_some()),
            );
        }
        _ => {
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

/// An SRT stream: H.264 and AAC in MPEG-TS. Listens when the URL has no
/// host, otherwise calls, retrying until the other end answers.
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
/// keyframe. `mpegtsmux` writes a PMT with only the streams that have caps,
/// then another when the second arrives, and `srtsink` replays that first
/// PMT to every new receiver: they'd see the program change, and many drop a
/// stream when it does.
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
        Ok(Box::new(PipelineOutput::start(
            monitor,
            self.label(),
            self.build()?,
        )?))
    }
}

impl SrtOutput {
    fn build(&self) -> Result<OutputPipeline> {
        let mut output = OutputPipeline::new(&format!("output-srt-{}", self.url))?;
        let consumer = &output.consumer;
        let venc = self.video_encoder()?;
        let vparse = make_el("h264parse", "vparse")?;
        // Parameter sets with every keyframe, for receivers that join late.
        vparse.set_property("config-interval", -1i32);
        let aenc = make_el("avenc_aac", "aenc")?;
        set_property(
            &aenc,
            "bitrate",
            &aac_bitrate(self.program.audio_channels).to_string(),
        )?;
        let mux = make_el("mpegtsmux", "mux")?;
        // 7 TS packets per buffer: one SRT payload (1316 bytes).
        set_property(&mux, "alignment", "7")?;
        super::hold_srt();
        let sink = make_el("srtsink", "srtsink")?;
        sink.set_property("uri", stream::uri(&self.url)?);
        set_property(&sink, "latency", &self.latency_ms.to_string())?;
        if let Some(passphrase) = &self.passphrase {
            sink.set_property("passphrase", passphrase);
        }
        // A listener with nobody connected drops the stream rather than
        // holding up the program.
        sink.set_property("wait-for-connection", false);

        let receivers = self
            .listens()
            .then(|| count_receivers(&sink, "caller-added", "caller-removed"));

        consumer
            .pipeline
            .add_many([&venc, &vparse, &aenc, &mux, &sink])
            .context("add SRT output")?;
        consumer.video_chain(&[], &[&venc, &vparse, &mux])?;
        consumer.audio_chain(&[], &[&aenc, &mux])?;
        mux.link(&sink).context("link SRT mux → sink")?;
        start_together(&vparse, &aenc)?;
        output.receivers = receivers;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::api::types::{ChannelConfig, MonitorSettingsDto, VideoFormat};
    use crate::sources::channel::ChannelSource;

    const PROGRAM: Program = Program {
        format: VideoFormat::new(640, 360, (30, 1)),
        audio_channels: 2,
    };

    /// The sink for `config` on a channel called `c`, on a node without
    /// screens.
    fn test_sink(config: &OutputConfig) -> Box<dyn OutputSink> {
        static DEVICES: std::sync::OnceLock<Arc<LocalDevices>> = std::sync::OnceLock::new();
        let context = OutputContext {
            displays: &Displays::none(),
            devices: DEVICES.get_or_init(|| Arc::new(LocalDevices::start())),
        };
        sink(config, "c", PROGRAM, &context)
    }

    /// A 640x360 channel's program, running.
    fn channel_monitor() -> Arc<MonitorPipeline> {
        let channel = ChannelSource::new(
            "c".into(),
            "c".into(),
            ChannelConfig {
                format: VideoFormat {
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
        crate::plugins::locate_ndi_runtime();
        gstndi::plugin_register_static().unwrap();
        let name = format!("cr-test-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let mon = channel_monitor();
        let config = OutputConfig::Ndi {
            ndi_name: Some(name.clone()),
        };
        let leg = OutputLeg::start(&mon, test_sink(&config).as_ref()).unwrap();

        let sender = find_ndi_source(&format!("({name})"));
        let (recv, video, audio) = receiver(&format!(
            "ndisrc ndi-name=\"{sender}\" ! ndisrcdemux name=d \
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

    /// The full NDI name of the source whose name ends with `suffix`. NDI
    /// puts the machine's name first, as it sees it, so ask discovery.
    fn find_ndi_source(suffix: &str) -> String {
        let monitor = gst::DeviceMonitor::new();
        monitor.add_filter(Some("Source/Audio/Video/Network"), None);
        monitor.start().unwrap();
        let found = (0..20).find_map(|_| {
            std::thread::sleep(Duration::from_millis(500));
            monitor.devices().iter().find_map(|d| {
                d.properties()?
                    .get::<String>("ndi-name")
                    .ok()
                    .filter(|n| n.ends_with(suffix))
            })
        });
        monitor.stop();
        found.unwrap_or_else(|| panic!("no NDI source named \"… {suffix}\""))
    }

    /// An SRT listener output runs with nobody connected, and a receiver
    /// that joins later gets the program (and is counted).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~10 s); run with --ignored"]
    async fn srt_listener_output_is_received() {
        gst::init().unwrap();
        let port = 7751;
        let mon = channel_monitor();
        let leg =
            OutputLeg::start(&mon, test_sink(&srt(format!("srt://:{port}"))).as_ref()).unwrap();
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
            test_sink(&srt(format!("srt://127.0.0.1:{port}"))).as_ref(),
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
        let leg = OutputLeg::start(&mon, test_sink(&config).as_ref()).unwrap();
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

    /// A mount whose channel has gone says why its viewers get nothing.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~3 s); uses port 8554; run with --ignored"]
    async fn rtsp_output_reports_an_unfed_viewer() {
        gst::init().unwrap();
        let mon = channel_monitor();
        let config = OutputConfig::Rtsp {
            path: Some("cr-test-unfed".into()),
            bitrate_kbps: 2000,
        };
        let leg = OutputLeg::start(&mon, test_sink(&config).as_ref()).unwrap();
        mon.stop().unwrap();
        drop(mon);
        assert_eq!(leg.error(), None);

        let url = format!("rtsp://127.0.0.1:{}/cr-test-unfed", rtsp::RTSP_PORT);
        let (recv, video, _) = rtsp_receiver(&url);
        recv.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        recv.set_state(gst::State::Null).unwrap();

        assert_eq!(video.load(Ordering::Relaxed), 0);
        assert_eq!(
            leg.error().as_deref(),
            Some("viewers get nothing: the channel stopped")
        );
    }
}
