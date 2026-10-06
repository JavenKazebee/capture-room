use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use tracing::warn;

use super::live::{self, LinkTracker, LiveAudio, LiveInput};
use super::InputSource;
use crate::api::types::{
    AudioPlan, DeviceDto, DeviceKind, DeviceMode, DeviceSourceConfig, LinkState, LiveVideoFormat,
    SourceCapabilitiesDto, SourceType,
};
use crate::pipeline::{capsfilter, make_el};

/// Device classes the monitor watches. Screens are `Source/Monitor` on
/// Windows; macOS lists them as video sources.
const CLASSES: &[&str] = &["Video/Source", "Audio/Source", "Source/Monitor"];

/// Modes listed per video device, at most.
const MAX_MODES: usize = 32;

// ── Device monitor ────────────────────────────────────────────────────────────

/// The node's capture devices, from a `GstDeviceMonitor` that runs for the
/// app's lifetime (like [`super::ndi::NdiMonitor`]). Each platform's device
/// providers make the right element (`v4l2src` / `pipewiresrc` on Linux,
/// `avfvideosrc` on macOS, `mfvideosrc` / `wasapi2src` on Windows), so
/// nothing here is platform-specific except how a device's stable key is
/// found.
pub struct LocalDevices {
    monitor: gst::DeviceMonitor,
}

impl LocalDevices {
    /// Blocking: starting a monitor probes the providers.
    pub fn start() -> Self {
        let monitor = gst::DeviceMonitor::new();
        // PipeWire's provider hides the v4l2 and PulseAudio ones it
        // duplicates; show them, and skip PipeWire's (see `usable`).
        monitor.set_show_all_devices(true);
        for class in CLASSES {
            monitor.add_filter(Some(class), None);
        }
        if let Err(e) = monitor.start() {
            warn!("device monitor failed to start: {e}");
        }
        Self { monitor }
    }

    pub fn list(&self) -> Vec<DeviceDto> {
        let mut seen = BTreeSet::new();
        let devices = self.monitor.devices();
        let has_pulse = devices.iter().any(is_pulse);
        let mut list: Vec<(bool, DeviceDto)> = devices
            .into_iter()
            .filter(usable)
            .filter(|d| !(has_pulse && is_alsa(d)))
            .map(|d| (is_monitor(&d), describe(&d)))
            // A device two providers both list keeps its first entry.
            .filter(|(_, d)| seen.insert(d.key.clone()))
            .collect();
        // Real inputs before monitors of outputs (what the machine plays).
        list.sort_by_key(|(monitor, _)| *monitor);
        list.into_iter().map(|(_, d)| d).collect()
    }

    fn find(&self, key: &str) -> Option<gst::Device> {
        self.monitor
            .devices()
            .into_iter()
            .find(|d| device_key(d) == key)
    }

    /// An element capturing from the device, and its description.
    fn element(&self, key: &str, name: &str) -> Result<(gst::Element, DeviceDto)> {
        let device = self
            .find(key)
            .ok_or_else(|| anyhow!("{} isn't connected", display(key, name)))?;
        Ok((open(&device)?, describe(&device)))
    }

    /// Resolve an audio plan to what [`live::build_bin`] takes.
    pub fn audio(&self, plan: &AudioPlan) -> Result<LiveAudio> {
        Ok(match plan {
            AudioPlan::Source { channels } => LiveAudio::Source {
                channels: *channels,
            },
            AudioPlan::Silence { channels } => LiveAudio::Silence {
                channels: *channels,
            },
            AudioPlan::Device {
                device_key,
                channels,
            } => {
                let (element, dto) = self.element(device_key, "")?;
                LiveAudio::Element {
                    element,
                    inputs: dto.channels.unwrap_or(2),
                    channels: channels.clone(),
                }
            }
        })
    }
}

impl Drop for LocalDevices {
    fn drop(&mut self) {
        self.monitor.stop();
    }
}

/// An element capturing from `device`, made to find it again after it's
/// replugged: PipeWire targets the node's serial, which changes, so it
/// targets the (stable) node name instead.
fn open(device: &gst::Device) -> Result<gst::Element> {
    let element = device
        .create_element(None)
        .with_context(|| format!("open {}", device.display_name()))?;
    let node_name = device
        .properties()
        .and_then(|p| p.get::<String>("node.name").ok());
    if let Some(name) = node_name {
        if element.find_property("target-object").is_some() {
            element.set_property("target-object", name);
        }
    }
    Ok(element)
}

/// Whether to offer `device`. PipeWire's devices are skipped: its
/// `pipewiresrc` fails to find its target on some systems (seen with
/// PipeWire 1.6 / GStreamer 1.28), and the same cameras and audio inputs
/// are listed by the v4l2 and PulseAudio providers, whose elements are the
/// long-established ones (PipeWire serves PulseAudio clients itself).
fn usable(device: &gst::Device) -> bool {
    !device.type_().name().starts_with("GstPipeWire")
}

fn display(key: &str, name: &str) -> String {
    if name.is_empty() {
        key.to_string()
    } else {
        format!("{name} ({key})")
    }
}

/// A key for `device` that survives restarts and replugging where the
/// platform gives one: PipeWire's node name, a v4l2 path, AVFoundation's
/// unique id, a Windows device path. The display name is the last resort.
fn device_key(device: &gst::Device) -> String {
    const STABLE: &[&str] = &[
        "node.name",
        "api.v4l2.path",
        "device.path",
        "unique-id",
        "avf.unique_id",
        "device.id",
        "device.strid",
    ];
    // The device objects' own ids first: a PulseAudio source's name (the
    // same as PipeWire's node name) and a v4l2 device's path.
    let own = ["internal-name", "device-path"].iter().find_map(|name| {
        device
            .find_property(name)
            .and_then(|_| device.property::<Option<String>>(name))
            .filter(|v| !v.is_empty())
    });
    let props = device.properties();
    let stable = own.or_else(|| {
        props.as_ref().and_then(|p| {
            STABLE
                .iter()
                .find_map(|k| p.get::<String>(*k).ok().filter(|v| !v.is_empty()))
        })
    });
    let kind = match kind(device) {
        DeviceKind::Video => "video",
        DeviceKind::Audio => "audio",
        DeviceKind::Screen => "screen",
    };
    format!(
        "{kind}:{}",
        stable.unwrap_or_else(|| device.display_name().to_string())
    )
}

fn is_pulse(device: &gst::Device) -> bool {
    device.type_().name().starts_with("GstPulse")
}

/// ALSA's raw devices duplicate PulseAudio's, and opening one directly
/// would fight the sound server for the card.
fn is_alsa(device: &gst::Device) -> bool {
    device.type_().name().starts_with("GstAlsa")
}

/// A monitor of an audio output: the sound the machine plays.
fn is_monitor(device: &gst::Device) -> bool {
    device
        .properties()
        .and_then(|p| p.get::<String>("device.class").ok())
        .is_some_and(|c| c == "monitor")
}

fn kind(device: &gst::Device) -> DeviceKind {
    let class = device.device_class();
    if class.contains("Monitor") || device.display_name().to_lowercase().contains("screen") {
        DeviceKind::Screen
    } else if class.contains("Video") {
        DeviceKind::Video
    } else {
        DeviceKind::Audio
    }
}

fn describe(device: &gst::Device) -> DeviceDto {
    let props = device.properties();
    let api = props
        .as_ref()
        .and_then(|p| p.get::<String>("device.api").ok())
        .or_else(|| {
            props
                .as_ref()
                .filter(|p| p.has_field("node.name"))
                .map(|_| "pipewire".to_string())
        })
        .unwrap_or_default();
    let caps = device.caps();
    let kind = kind(device);
    let mut modes: Vec<DeviceMode> = Vec::new();
    let mut channels = None;
    for s in caps.iter().flat_map(|c| c.iter()) {
        if s.name().starts_with("audio/") {
            // A fixed count; a range (PulseAudio's 1–32) says nothing.
            channels = channels.max(s.get::<i32>("channels").ok().map(|c| c as u32));
            continue;
        }
        let (Some(width), Some(height)) = (max_int(s, "width"), max_int(s, "height")) else {
            continue;
        };
        let format = match s.get::<&str>("format") {
            Ok(f) => f.to_string(),
            Err(_) => s.name().trim_start_matches("image/").to_string(),
        };
        let rates = rates(s);
        let rates = if rates.is_empty() {
            vec![(0, 1)]
        } else {
            rates
        };
        for (fps_num, fps_den) in rates {
            match modes.iter_mut().find(|m| {
                (m.width, m.height, m.fps_num, m.fps_den) == (width, height, fps_num, fps_den)
            }) {
                Some(m) if !m.formats.contains(&format) => m.formats.push(format.clone()),
                Some(_) => {}
                None => modes.push(DeviceMode {
                    width,
                    height,
                    fps_num,
                    fps_den,
                    formats: vec![format.clone()],
                }),
            }
        }
    }
    // Largest and fastest first.
    modes.sort_by(|a, b| {
        let fps = |m: &DeviceMode| m.fps_num as f64 / m.fps_den.max(1) as f64;
        (b.width * b.height)
            .cmp(&(a.width * a.height))
            .then(fps(b).total_cmp(&fps(a)))
    });
    modes.truncate(MAX_MODES);
    // The device's own count, where the caps only give a range.
    let declared = props.as_ref().and_then(|p| {
        p.get::<String>("audio.channels")
            .ok()
            .and_then(|c| c.parse().ok())
            .or_else(|| p.get::<i32>("audio.channels").ok().map(|c| c as u32))
    });
    let channels = match kind {
        DeviceKind::Audio => declared.or(channels).or(Some(2)),
        _ => None,
    };
    DeviceDto {
        key: device_key(device),
        name: device.display_name().to_string(),
        kind,
        api,
        channels,
        modes,
    }
}

/// An int field's value, or the top of its range or list.
fn max_int(s: &gst::StructureRef, name: &str) -> Option<u32> {
    let v = s.value(name).ok()?;
    if let Ok(i) = v.get::<i32>() {
        return u32::try_from(i).ok();
    }
    if let Ok(r) = v.get::<gst::IntRange<i32>>() {
        return u32::try_from(r.max()).ok();
    }
    if let Ok(l) = v.get::<gst::List>() {
        return l
            .iter()
            .filter_map(|v| v.get::<i32>().ok())
            .max()
            .map(|i| i as u32);
    }
    None
}

/// The frame rates a caps structure offers: each of a list, or the top of
/// a range.
fn rates(s: &gst::StructureRef) -> Vec<(u32, u32)> {
    let Ok(v) = s.value("framerate") else {
        return Vec::new();
    };
    let frac = |f: gst::Fraction| (f.numer().max(0) as u32, f.denom().max(1) as u32);
    if let Ok(f) = v.get::<gst::Fraction>() {
        return vec![frac(f)];
    }
    if let Ok(r) = v.get::<gst::FractionRange>() {
        return vec![frac(r.max())];
    }
    if let Ok(l) = v.get::<gst::List>() {
        return l
            .iter()
            .filter_map(|v| v.get::<gst::Fraction>().ok())
            .map(frac)
            .collect();
    }
    Vec::new()
}

// ── Capture element ───────────────────────────────────────────────────────────

/// Wrap a video device's element so it always outputs raw video from a
/// static pad (what `fallbacksrc` needs): raw if the device offers it in the
/// requested format, else MJPEG decoded (webcams often offer high
/// resolutions only as MJPEG).
fn capture_bin(
    id: &str,
    element: gst::Element,
    device_caps: Option<gst::Caps>,
    format: Option<&LiveVideoFormat>,
) -> Result<gst::Element> {
    let want = |media: &str| {
        let mut b = gst::Caps::builder(media);
        if let Some(f) = format {
            b = b
                .field("width", f.width as i32)
                .field("height", f.height as i32)
                .field(
                    "framerate",
                    gst::Fraction::new(f.fps_num as i32, f.fps_den as i32),
                );
        }
        b.build()
    };
    let offers = |caps: &gst::Caps| device_caps.as_ref().is_none_or(|d| d.can_intersect(caps));
    let raw = want("video/x-raw");
    let jpeg = want("image/jpeg");
    let bin = gst::Bin::with_name(&format!("device-capture-{id}"));
    let mut chain = vec![element];
    if offers(&raw) || !offers(&jpeg) {
        chain.push(capsfilter(&format!("device-caps-{id}"), raw)?);
    } else {
        chain.push(capsfilter(&format!("device-caps-{id}"), jpeg)?);
        chain.push(make_el("jpegparse", &format!("device-jpegparse-{id}"))?);
        chain.push(make_el("jpegdec", &format!("device-jpegdec-{id}"))?);
    }
    bin.add_many(&chain).context("add device capture")?;
    gst::Element::link_many(&chain).context("link device capture")?;
    let last = chain.last().expect("chain has elements");
    let pad = last.static_pad("src").context("device capture src pad")?;
    let ghost = gst::GhostPad::builder_with_target(&pad)
        .map_err(|e| anyhow!("device ghost pad: {e}"))?
        .name("src")
        .build();
    bin.add_pad(&ghost).context("add device ghost pad")?;
    Ok(bin.upcast())
}

// ── DeviceSource ──────────────────────────────────────────────────────────────

pub struct DeviceSource {
    id: String,
    name: String,
    config: DeviceSourceConfig,
    devices: Arc<LocalDevices>,
    tracker: Arc<LinkTracker>,
}

impl DeviceSource {
    pub fn new(
        id: String,
        name: String,
        config: DeviceSourceConfig,
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

impl InputSource for DeviceSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Device
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        None
    }

    /// Not whether the devices are connected: a device unplugged while
    /// recording plays black until it's back (a rebuild would stop the
    /// recording). One missing when the source is built fails it, and the
    /// failed-monitor rescan retries.
    fn fingerprint(&self) -> String {
        serde_json::to_string(&self.config).unwrap_or_default()
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let cfg = &self.config;
        let device = self.devices.find(&cfg.video_device).ok_or_else(|| {
            anyhow!(
                "{} isn't connected",
                display(&cfg.video_device, &cfg.video_device_name)
            )
        })?;
        let element = open(&device)?;
        let source = capture_bin(&self.id, element, device.caps(), cfg.format.as_ref())?;
        let bin = live::build_bin(LiveInput {
            id: &self.id,
            source,
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
        Some(self.tracker.state(false))
    }
}

/// Check a config before it's saved, and fill in the device's name.
pub fn validate(cfg: &mut DeviceSourceConfig, devices: &LocalDevices) -> Result<(), String> {
    let device = devices
        .list()
        .into_iter()
        .find(|d| d.key == cfg.video_device)
        .ok_or("pick a connected video device")?;
    if device.kind == DeviceKind::Audio {
        return Err(format!("{} is an audio device", device.name));
    }
    cfg.video_device_name = device.name;
    if let Some(f) = &cfg.format {
        if f.width == 0 || f.height == 0 || f.fps_num == 0 || f.fps_den == 0 {
            return Err("the format needs a size and frame rate".into());
        }
    }
    validate_audio(&cfg.audio, devices)
}

/// Check an audio plan against the node's devices.
pub fn validate_audio(plan: &AudioPlan, devices: &LocalDevices) -> Result<(), String> {
    match plan {
        AudioPlan::Source { channels } | AudioPlan::Silence { channels } => {
            if !(1..=64).contains(channels) {
                return Err("audio needs 1–64 channels".into());
            }
        }
        AudioPlan::Device {
            device_key,
            channels,
        } => {
            let device = devices
                .list()
                .into_iter()
                .find(|d| &d.key == device_key)
                .ok_or("pick a connected audio device")?;
            let inputs = device.channels.unwrap_or(2);
            if channels.is_empty() {
                return Err("pick at least one audio channel".into());
            }
            if let Some(c) = channels.iter().find(|&&c| c == 0 || c > inputs) {
                return Err(format!("{} has no channel {c}", device.name));
            }
        }
    }
    Ok(())
}
