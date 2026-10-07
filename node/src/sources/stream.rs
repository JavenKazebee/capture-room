use std::sync::Arc;

use anyhow::{bail, Result};
use gstreamer::{self as gst, prelude::*};

use super::device::LocalDevices;
use super::live::{self, LinkTracker, LiveInput};
use super::InputSource;
use crate::api::types::{
    AudioPlan, LinkState, RtspTransport, SourceCapabilitiesDto, SourceType, StreamSourceConfig,
};
use crate::pipeline::make_el;

/// URL schemes a stream source accepts, and the element each one needs.
pub const PROTOCOLS: &[(&str, &str)] = &[
    ("rtsp", "rtspsrc"),
    ("rtsps", "rtspsrc"),
    ("srt", "srtsrc"),
    ("rtmp", "rtmp2src"),
    ("http", "souphttpsrc"),
    ("https", "souphttpsrc"),
    ("udp", "udpsrc"),
];

// ── StreamSource ──────────────────────────────────────────────────────────────

/// A network stream, decoded by `uridecodebin3` and kept alive by
/// `fallbacksrc` (see [`live`]).
pub struct StreamSource {
    id: String,
    name: String,
    config: StreamSourceConfig,
    devices: Arc<LocalDevices>,
    tracker: Arc<LinkTracker>,
}

impl StreamSource {
    pub fn new(
        id: String,
        name: String,
        config: StreamSourceConfig,
        devices: Arc<LocalDevices>,
    ) -> Self {
        Self {
            id,
            name,
            config,
            devices,
            tracker: Arc::default(),
        }
    }
}

impl InputSource for StreamSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Stream
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        // Known only once the stream connects.
        None
    }

    fn fingerprint(&self) -> String {
        serde_json::to_string(&self.config).unwrap_or_default()
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let cfg = &self.config;
        let decode = make_el("uridecodebin3", &format!("stream-decode-{}", self.id))?;
        decode.set_property("uri", uri(&cfg.url)?);
        let (latency, transport) = (cfg.latency_ms, cfg.rtsp_transport);
        decode.connect("source-setup", false, move |args| {
            if let Ok(src) = args[1].get::<gst::Element>() {
                configure(&src, latency, transport);
            }
            None
        });
        let bin = live::build_bin(LiveInput {
            id: &self.id,
            source: decode,
            audio: self.devices.audio(&cfg.audio)?,
            format: cfg.format.as_ref(),
            tracker: &self.tracker,
            direct: false,
        })?;
        Ok(bin.upcast())
    }

    fn timecode(&self) -> Option<String> {
        None
    }

    fn link(&self) -> Option<LinkState> {
        Some(self.tracker.state(listen_port(&self.config).is_some()))
    }
}

/// Apply the source's settings to the protocol element `uridecodebin3`
/// made for it.
fn configure(src: &gst::Element, latency_ms: u32, transport: RtspTransport) {
    let factory = src
        .factory()
        .map(|f| f.name().to_string())
        .unwrap_or_default();
    if src.find_property("latency").is_some() {
        // rtspsrc's is unsigned, srtsrc's signed: parse from a string.
        src.set_property_from_str("latency", &latency_ms.to_string());
    }
    if factory == "rtspsrc" {
        match transport {
            RtspTransport::Auto => {}
            RtspTransport::Tcp => src.set_property_from_str("protocols", "tcp"),
            RtspTransport::Udp => src.set_property_from_str("protocols", "udp+udp-mcast"),
        }
    }
}

// ── URLs ──────────────────────────────────────────────────────────────────────

/// The URL's scheme, lowercased.
pub fn scheme(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    (!scheme.is_empty() && !rest.is_empty()).then(|| scheme.to_ascii_lowercase())
}

/// `host:port` part of a URL (no userinfo).
fn authority(url: &str) -> &str {
    let rest = url.split_once("://").map_or("", |(_, r)| r);
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let auth = &rest[..end];
    auth.rsplit_once('@').map_or(auth, |(_, a)| a)
}

fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    url.split_once('?')?.1.split('&').find_map(|kv| {
        kv.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
    })
}

/// Host and port; the host is empty for `srt://:9000` and `udp://@:5000`.
fn host_port(url: &str) -> (&str, Option<u16>) {
    let auth = authority(url).trim_start_matches('@');
    match auth.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port.parse().ok()),
        _ => (auth, None),
    }
}

/// The local port a listening stream binds: an SRT listener, or UDP.
pub fn listen_port(cfg: &StreamSourceConfig) -> Option<u16> {
    let url = cfg.url.trim();
    let (host, port) = host_port(url);
    match scheme(url)?.as_str() {
        "udp" => port,
        "srt" if host.is_empty() || query_param(url, "mode") == Some("listener") => port,
        _ => None,
    }
}

/// The URI to hand GStreamer: an SRT URL with no host listens, and UDP's
/// VLC-style `udp://@:5000` / `udp://@239.1.1.1:5000` become plain
/// `host:port` (no host binds every interface).
fn uri(url: &str) -> Result<String> {
    let url = url.trim();
    let Some(scheme) = scheme(url) else {
        bail!("not a URL: {url}");
    };
    let (host, port) = host_port(url);
    if scheme == "udp" {
        let host = if host.is_empty() { "0.0.0.0" } else { host };
        let port = port.ok_or_else(|| anyhow::anyhow!("{url} has no port"))?;
        let query = url
            .split_once('?')
            .map_or(String::new(), |(_, q)| format!("?{q}"));
        return Ok(format!("udp://{host}:{port}{query}"));
    }
    if scheme == "srt" && host.is_empty() && query_param(url, "mode").is_none() {
        let sep = if url.contains('?') { '&' } else { '?' };
        return Ok(format!("{url}{sep}mode=listener"));
    }
    Ok(url.to_string())
}

/// Check a config before it's saved. `available` says whether an element is
/// installed on this node.
pub fn validate(cfg: &StreamSourceConfig, available: impl Fn(&str) -> bool) -> Result<(), String> {
    let url = cfg.url.trim();
    let scheme = scheme(url).ok_or("enter a URL, like rtsp://camera.local/stream")?;
    let (_, element) = PROTOCOLS
        .iter()
        .find(|(s, _)| *s == scheme)
        .ok_or_else(|| format!("{scheme}:// isn't supported"))?;
    if !available(element) {
        return Err(format!(
            "{scheme}:// needs {element}, which isn't installed on this node"
        ));
    }
    let (host, port) = host_port(url);
    if host.is_empty() && listen_port(cfg).is_none() {
        return Err("the URL needs a host".into());
    }
    if listen_port(cfg).is_none() && matches!(scheme.as_str(), "udp" | "srt") && port.is_none() {
        return Err("the URL needs a port".into());
    }
    if let Some(f) = &cfg.format {
        if f.width == 0 || f.height == 0 || f.fps_num == 0 || f.fps_den == 0 {
            return Err("the format needs a size and frame rate".into());
        }
    }
    match &cfg.audio {
        AudioPlan::Source { channels } | AudioPlan::Silence { channels }
            if !(1..=64).contains(channels) =>
        {
            Err("audio needs 1–64 channels".into())
        }
        // Checked against the node's devices by the caller.
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> StreamSourceConfig {
        StreamSourceConfig {
            url: url.into(),
            latency_ms: 200,
            rtsp_transport: RtspTransport::Auto,
            audio: AudioPlan::default(),
            format: None,
        }
    }

    #[test]
    fn listeners() {
        assert_eq!(listen_port(&cfg("srt://:9000")), Some(9000));
        assert_eq!(
            listen_port(&cfg("srt://0.0.0.0:9000?mode=listener")),
            Some(9000)
        );
        assert_eq!(listen_port(&cfg("srt://10.0.0.5:9000")), None);
        assert_eq!(listen_port(&cfg("udp://@:5000")), Some(5000));
        assert_eq!(listen_port(&cfg("udp://239.1.1.1:5000")), Some(5000));
        assert_eq!(listen_port(&cfg("rtsp://user:pw@cam:554/s")), None);
    }

    #[test]
    fn uris() {
        assert_eq!(uri("srt://:9000").unwrap(), "srt://:9000?mode=listener");
        assert_eq!(
            uri("srt://:9000?latency=100").unwrap(),
            "srt://:9000?latency=100&mode=listener"
        );
        assert_eq!(uri(" rtsp://cam/s ").unwrap(), "rtsp://cam/s");
        assert_eq!(uri("udp://@:5000").unwrap(), "udp://0.0.0.0:5000");
        assert_eq!(
            uri("udp://@239.1.1.1:5000").unwrap(),
            "udp://239.1.1.1:5000"
        );
        assert!(uri("cam").is_err());
    }

    #[test]
    fn validation() {
        let all = |_: &str| true;
        assert!(validate(&cfg("rtsp://cam.local/stream"), all).is_ok());
        assert!(validate(&cfg("srt://:9000"), all).is_ok());
        assert!(validate(&cfg("srt://host"), all).is_err());
        assert!(validate(&cfg("ftp://x/y"), all).is_err());
        assert!(validate(&cfg("rtsp://cam/s"), |_| false).is_err());
    }
}

/// The whole monitor pipeline, as the app runs it, across a sender dropout:
/// the producers recordings read from must never stall, or a recording's
/// input queue drops the burst that follows.
#[cfg(test)]
mod monitor_tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::api::types::MonitorSettingsDto;
    use crate::pipeline::monitor::MonitorPipeline;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~30 s); run with --ignored"]
    async fn producers_never_stall_across_a_dropout() {
        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        let port = 7741;
        let src = StreamSource::new(
            "t".into(),
            "t".into(),
            StreamSourceConfig {
                url: format!("srt://:{port}"),
                latency_ms: 200,
                rtsp_transport: RtspTransport::Auto,
                audio: AudioPlan::default(),
                format: None,
            },
            Arc::new(LocalDevices::start()),
        );
        let mon = MonitorPipeline::new(
            &src,
            &MonitorSettingsDto::default(),
            &gst::SystemClock::obtain(),
        )
        .unwrap();
        let stalls = Arc::new(Mutex::new(Vec::<String>::new()));
        for (name, producer) in [("video", &mon.video), ("audio", &mon.audio)] {
            let last = Mutex::new(None::<Instant>);
            let stalls = stalls.clone();
            producer.appsink().static_pad("sink").unwrap().add_probe(
                gst::PadProbeType::BUFFER,
                move |_, _| {
                    let now = Instant::now();
                    let mut last = last.lock().unwrap();
                    if let Some(prev) = last.filter(|p| now - *p > Duration::from_millis(150)) {
                        stalls
                            .lock()
                            .unwrap()
                            .push(format!("{name}: {:?}", now - prev));
                    }
                    *last = Some(now);
                    gst::PadProbeReturn::Ok
                },
            );
        }
        let send = || {
            let p = gst::parse::launch(&format!(
                "videotestsrc is-live=1 pattern=ball ! video/x-raw,width=1280,height=720,framerate=30/1,format=I420 \
                 ! x264enc tune=zerolatency key-int-max=30 ! mux. \
                 audiotestsrc is-live=1 ! audioconvert ! avenc_aac ! aacparse ! mux. \
                 mpegtsmux name=mux ! srtsink uri=srt://127.0.0.1:{port}?mode=caller"
            ))
            .unwrap();
            p.set_state(gst::State::Playing).unwrap();
            p
        };
        let sleep = |s| tokio::time::sleep(Duration::from_secs(s));

        sleep(3).await;
        let s = send();
        sleep(5).await;
        assert_eq!(src.link(), Some(LinkState::Live));
        s.set_state(gst::State::Null).unwrap();
        drop(s);
        // Long enough for fallbacksrc to restart the source a few times.
        sleep(15).await;
        let s = send();
        sleep(6).await;
        assert_eq!(src.link(), Some(LinkState::Live));
        s.set_state(gst::State::Null).unwrap();
        mon.stop().unwrap();
        assert_eq!(*stalls.lock().unwrap(), Vec::<String>::new());
    }

    /// A stream's audio replaced by an audio device's, with its channels
    /// picked (and swapped). Needs an audio input on the machine.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an audio input; real-time (~8 s)"]
    async fn audio_from_a_device() {
        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        let devices = Arc::new(LocalDevices::start());
        let mic = devices
            .list()
            .into_iter()
            .find(|d| d.kind == crate::api::types::DeviceKind::Audio)
            .expect("an audio input");
        let port = 7742;
        let src = StreamSource::new(
            "m".into(),
            "m".into(),
            StreamSourceConfig {
                url: format!("srt://:{port}"),
                latency_ms: 200,
                rtsp_transport: RtspTransport::Auto,
                audio: AudioPlan::Device {
                    device_key: mic.key.clone(),
                    channels: vec![2, 1],
                },
                format: None,
            },
            devices,
        );
        let mon = MonitorPipeline::new(
            &src,
            &MonitorSettingsDto::default(),
            &gst::SystemClock::obtain(),
        )
        .unwrap();
        let audio_buffers = Arc::new(Mutex::new(0u32));
        let n = audio_buffers.clone();
        mon.audio.appsink().static_pad("sink").unwrap().add_probe(
            gst::PadProbeType::BUFFER,
            move |_, _| {
                *n.lock().unwrap() += 1;
                gst::PadProbeReturn::Ok
            },
        );
        let sender = gst::parse::launch(&format!(
            "videotestsrc is-live=1 ! video/x-raw,width=640,height=360,framerate=30/1,format=I420 \
             ! x264enc tune=zerolatency key-int-max=30 ! mpegtsmux ! srtsink uri=srt://127.0.0.1:{port}?mode=caller"
        ))
        .unwrap();
        sender.set_state(gst::State::Playing).unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;
        sender.set_state(gst::State::Null).unwrap();
        assert_eq!(src.link(), Some(LinkState::Live));
        assert_eq!(mon.error(), None);
        let caps = mon
            .audio
            .appsink()
            .static_pad("sink")
            .unwrap()
            .current_caps()
            .unwrap();
        assert_eq!(
            caps.structure(0).unwrap().get::<i32>("channels").unwrap(),
            2
        );
        // 10 ms buffers: about 100 a second once running.
        assert!(*audio_buffers.lock().unwrap() > 300, "audio flows");
        mon.stop().unwrap();
    }
}
