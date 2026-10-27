//! Blackmagic Decklink cards: SDI capture ([`DecklinkSource`]) and playout
//! ([`DecklinkOutput`]) via gst-plugins-bad's decklink elements.
//!
//! A card appears as sub-devices (one per connector, or pair when full
//! duplex), split per the card's profile, a per-card node setting applied to
//! every element opened on it. The card keeps sending frames (flagged as gaps)
//! with no signal, so a source is never restarted like `fallbacksrc` ones: it's
//! a direct source in [`live::build_bin`] and no-signal frames are dropped.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_video as gst_video;
use tracing::warn;

use super::device::LocalDevices;
use super::live::{self, LinkTracker, LiveAudio, LiveInput};
use super::{ghost_pad, InputSource};
use crate::api::types::{
    ConfiguredSourceDto, DecklinkCardDto, DecklinkConnection, DecklinkDeviceDto, DecklinkModeDto,
    DecklinkProfile, DecklinkSourceConfig, DecklinkStatusDto, DecklinkUseDto, LinkState,
    OutputConfig, SourceCapabilitiesDto, SourceConfig, SourceType,
};
use crate::pipeline::monitor::MonitorPipeline;
use crate::pipeline::output::{OutputPipeline, OutputSink, PipelineOutput, Program, RunningOutput};
use crate::pipeline::{capsfilter, make_el, numbered_channels, set_mix_matrix, set_property};

pub const SOURCE_ELEMENTS: &[&str] = &["decklinkvideosrc", "decklinkaudiosrc"];
pub const OUTPUT_ELEMENTS: &[&str] = &["decklinkvideosink", "decklinkaudiosink"];

/// Embedded audio channel counts the cards carry.
const AUDIO_CHANNELS: &[u32] = &[2, 8, 16];

/// How long the live bin waits for a card's frames past the latency it reports
/// (about a frame). In 2160p its audio comes in later than that, and the
/// mixer dropping the late part of every frame's audio makes it buzz.
const LATENCY: Duration = Duration::from_millis(200);

/// Why this node can't use Decklink cards, if it can't.
pub fn unavailable(has: impl Fn(&str) -> bool) -> Option<String> {
    if let Some(e) = SOURCE_ELEMENTS
        .iter()
        .chain(OUTPUT_ELEMENTS)
        .find(|e| !has(e))
    {
        return Some(format!("GStreamer's decklink plugin is missing ({e})"));
    }
    if !driver_installed() {
        return Some("Blackmagic Desktop Video isn't installed".into());
    }
    None
}

/// Whether Blackmagic Desktop Video's API is installed, which the decklink
/// plugin loads at run time: without it the elements exist but find no cards.
#[cfg(target_os = "macos")]
pub fn driver_installed() -> bool {
    std::path::Path::new("/Library/Frameworks/DeckLinkAPI.framework").exists()
}

/// The plugin `dlopen`s the library by this name, so ask the loader rather
/// than guess at library folders. The plugin tries once per process, so a
/// driver installed later needs a restart anyway: the answer is kept, and
/// the handle left open for the plugin, which loads the same library.
#[cfg(target_os = "linux")]
pub fn driver_installed() -> bool {
    use std::sync::OnceLock;
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| {
        // SAFETY: a constant NUL-terminated name; the handle is only tested.
        let handle = unsafe { libc::dlopen(c"libDeckLinkAPI.so".as_ptr(), libc::RTLD_LAZY) };
        !handle.is_null()
    })
}

/// Desktop Video registers its COM server, `DeckLinkAPI64.dll`, from the
/// system folder; the plugin finds cards through that class.
#[cfg(target_os = "windows")]
pub fn driver_installed() -> bool {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::Path::new(&root)
        .join("System32")
        .join("DeckLinkAPI64.dll")
        .exists()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn driver_installed() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub nick: &'static str,
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub interlaced: bool,
}

const fn mode(
    nick: &'static str,
    label: &'static str,
    width: u32,
    height: u32,
    fps: (u32, u32),
    interlaced: bool,
) -> Mode {
    Mode {
        nick,
        label,
        width,
        height,
        fps_num: fps.0,
        fps_den: fps.1,
        interlaced,
    }
}

const P2398: (u32, u32) = (24000, 1001);
const P2997: (u32, u32) = (30000, 1001);
const P5994: (u32, u32) = (60000, 1001);

/// The broadcast modes offered, in list order. Interlaced modes' rates are
/// frame rates (1080i59.94 is 29.97 frames a second).
pub const MODES: &[Mode] = &[
    mode("2160p5994", "2160p 59.94", 3840, 2160, P5994, false),
    mode("2160p60", "2160p 60", 3840, 2160, (60, 1), false),
    mode("2160p50", "2160p 50", 3840, 2160, (50, 1), false),
    mode("2160p30", "2160p 30", 3840, 2160, (30, 1), false),
    mode("2160p2997", "2160p 29.97", 3840, 2160, P2997, false),
    mode("2160p25", "2160p 25", 3840, 2160, (25, 1), false),
    mode("2160p24", "2160p 24", 3840, 2160, (24, 1), false),
    mode("2160p2398", "2160p 23.98", 3840, 2160, P2398, false),
    mode("1080p5994", "1080p 59.94", 1920, 1080, P5994, false),
    mode("1080p60", "1080p 60", 1920, 1080, (60, 1), false),
    mode("1080p50", "1080p 50", 1920, 1080, (50, 1), false),
    mode("1080p30", "1080p 30", 1920, 1080, (30, 1), false),
    mode("1080p2997", "1080p 29.97", 1920, 1080, P2997, false),
    mode("1080p25", "1080p 25", 1920, 1080, (25, 1), false),
    mode("1080p24", "1080p 24", 1920, 1080, (24, 1), false),
    mode("1080p2398", "1080p 23.98", 1920, 1080, P2398, false),
    mode("1080i5994", "1080i 59.94", 1920, 1080, P2997, true),
    mode("1080i60", "1080i 60", 1920, 1080, (30, 1), true),
    mode("1080i50", "1080i 50", 1920, 1080, (25, 1), true),
    mode("720p5994", "720p 59.94", 1280, 720, P5994, false),
    mode("720p60", "720p 60", 1280, 720, (60, 1), false),
    mode("720p50", "720p 50", 1280, 720, (50, 1), false),
    mode("ntsc", "NTSC 525i 59.94", 720, 486, P2997, true),
    mode("pal", "PAL 625i 50", 720, 576, (25, 1), true),
];

pub fn find_mode(nick: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.nick == nick)
}

impl From<&Mode> for DecklinkModeDto {
    fn from(m: &Mode) -> Self {
        Self {
            mode: m.nick.into(),
            label: m.label.into(),
            width: m.width,
            height: m.height,
            fps_num: m.fps_num,
            fps_den: m.fps_den,
            interlaced: m.interlaced,
        }
    }
}

/// The mode an output plays `program` in: `pinned` (which must match its size
/// and rate) or the matching progressive mode. Interlaced output isn't supported yet.
pub fn output_mode(
    size: (u32, u32),
    fps: (u32, u32),
    pinned: Option<&str>,
) -> Result<&'static Mode, String> {
    let same_rate = |m: &Mode| m.fps_num as u64 * fps.1 as u64 == fps.0 as u64 * m.fps_den as u64;
    let fits = |m: &Mode| (m.width, m.height) == size && same_rate(m);
    let rate = fps.0 as f64 / fps.1.max(1) as f64;
    let program = format!("{}x{} at {rate:.2} fps", size.0, size.1);
    match pinned.filter(|p| !p.is_empty()) {
        Some(nick) => {
            let m = find_mode(nick).ok_or_else(|| format!("unknown SDI mode {nick}"))?;
            if m.interlaced {
                return Err(format!(
                    "{} is interlaced; SDI outputs are progressive for now",
                    m.label
                ));
            }
            if !fits(m) {
                return Err(format!("{} doesn't match the channel's {program}", m.label));
            }
            Ok(m)
        }
        None => MODES
            .iter()
            .find(|m| !m.interlaced && fits(m))
            .ok_or_else(|| format!("SDI has no mode for the channel's {program}")),
    }
}

/// The embedded channel count that carries `channels`: 2, 8 or 16.
pub fn embedded_channels(channels: u32) -> u32 {
    AUDIO_CHANNELS
        .iter()
        .copied()
        .find(|&n| n >= channels)
        .unwrap_or(16)
}

/// Profiles set on this node, by card key; shared so a change reaches what's built next.
#[derive(Default)]
pub struct Profiles(Mutex<HashMap<String, DecklinkProfile>>);

impl Profiles {
    pub fn get(&self, card: &str) -> DecklinkProfile {
        self.0
            .lock()
            .unwrap()
            .get(card)
            .copied()
            .unwrap_or_default()
    }

    pub fn set(&self, card: &str, profile: DecklinkProfile) {
        let mut map = self.0.lock().unwrap();
        if profile == DecklinkProfile::Default {
            map.remove(card);
        } else {
            map.insert(card.to_string(), profile);
        }
    }

    pub fn replace(&self, all: HashMap<String, DecklinkProfile>) {
        *self.0.lock().unwrap() = all;
    }

    pub fn all(&self) -> HashMap<String, DecklinkProfile> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone)]
struct SubDevice {
    key: String,
    name: String,
    card: String,
    card_name: String,
    persistent_id: Option<i64>,
    device_number: i32,
    capture: bool,
    playout: bool,
    modes: Vec<&'static Mode>,
}

impl SubDevice {
    fn element(&self, factory: &str, name: &str, profile: DecklinkProfile) -> Result<gst::Element> {
        let el = make_el(factory, name)?;
        match self.persistent_id {
            Some(id) => el.set_property("persistent-id", id),
            None => el.set_property("device-number", self.device_number),
        }
        if profile != DecklinkProfile::Default && el.find_property("profile").is_some() {
            set_property(&el, "profile", profile_nick(profile))?;
        }
        Ok(el)
    }
}

fn profile_nick(profile: DecklinkProfile) -> &'static str {
    match profile {
        DecklinkProfile::Default => "default",
        DecklinkProfile::OneSubDeviceFull => "one-sub-device-full",
        DecklinkProfile::OneSubDeviceHalf => "one-sub-device-half",
        DecklinkProfile::TwoSubDevicesFull => "two-sub-devices-full",
        DecklinkProfile::TwoSubDevicesHalf => "two-sub-devices-half",
        DecklinkProfile::FourSubDevicesHalf => "four-sub-devices-half",
    }
}

/// Whether a profile's sub-devices capture or play out, not both.
fn half_duplex(profile: DecklinkProfile) -> bool {
    matches!(
        profile,
        DecklinkProfile::OneSubDeviceHalf
            | DecklinkProfile::TwoSubDevicesHalf
            | DecklinkProfile::FourSubDevicesHalf
    )
}

pub fn is_decklink(device: &gst::Device) -> bool {
    device.type_().name().to_lowercase().contains("decklink")
}

/// The sub-devices among `devices`; capture and playout entries for one sub-device merge.
fn sub_devices(devices: &[gst::Device]) -> Vec<SubDevice> {
    let mut by_key: BTreeMap<String, SubDevice> = BTreeMap::new();
    for device in devices.iter().filter(|d| is_decklink(d)) {
        let class = device.device_class();
        let (capture, playout) = (class.contains("Video/Source"), class.contains("Video/Sink"));
        if !capture && !playout {
            continue; // the audio halves
        }
        let props = device.properties();
        let int = |name: &str| {
            props.as_ref().and_then(|p| {
                p.get::<i64>(name)
                    .ok()
                    .or_else(|| p.get::<i32>(name).ok().map(i64::from))
                    .or_else(|| p.get::<u32>(name).ok().map(i64::from))
            })
        };
        let persistent_id = int("persistent-id");
        let device_number = int("device-number").unwrap_or(0) as i32;
        let name = device.display_name().to_string();
        let card_name = card_name(&name);
        let serial = ["serial-number", "serial", "hw-serial-number"]
            .iter()
            .find_map(|k| props.as_ref()?.get::<String>(*k).ok())
            .filter(|s| !s.is_empty());
        let key = match persistent_id {
            Some(id) => format!("decklink:{id:x}"),
            None => format!("decklink:n{device_number}"),
        };
        let modes = device_modes(device.caps());
        let entry = by_key.entry(key.clone()).or_insert_with(|| SubDevice {
            key,
            name,
            card: serial.unwrap_or_else(|| card_name.clone()),
            card_name,
            persistent_id,
            device_number,
            capture: false,
            playout: false,
            modes: Vec::new(),
        });
        entry.capture |= capture;
        entry.playout |= playout;
        for m in modes {
            if !entry.modes.contains(&m) {
                entry.modes.push(m);
            }
        }
    }
    let mut list: Vec<SubDevice> = by_key.into_values().collect();
    for d in &mut list {
        d.modes.sort_by_key(|m| MODES.iter().position(|x| x == *m));
    }
    list.sort_by_key(|d| d.device_number);
    list
}

/// `DeckLink 8K Pro (2)` is sub-device 2 of a `DeckLink 8K Pro`.
fn card_name(name: &str) -> String {
    match name.rsplit_once(" (") {
        Some((card, n)) if n.trim_end_matches(')').chars().all(|c| c.is_ascii_digit()) => {
            card.to_string()
        }
        _ => name.to_string(),
    }
}

/// The listed modes the driver's caps offer (all of them if it gives none).
fn device_modes(caps: Option<gst::Caps>) -> Vec<&'static Mode> {
    let Some(caps) = caps.filter(|c| !c.is_empty() && !c.is_any()) else {
        return MODES.iter().collect();
    };
    MODES
        .iter()
        .filter(|m| {
            caps.iter().any(|s| {
                let int = |n: &str| s.get::<i32>(n).ok();
                let rate = s.get::<gst::Fraction>("framerate").ok();
                let interlaced = s
                    .get::<&str>("interlace-mode")
                    .is_ok_and(|i| i != "progressive");
                int("width") == Some(m.width as i32)
                    && int("height") == Some(m.height as i32)
                    && rate.is_some_and(|r| {
                        r.numer() as i64 * m.fps_den as i64 == m.fps_num as i64 * r.denom() as i64
                    })
                    && interlaced == m.interlaced
            })
        })
        .collect()
}

impl LocalDevices {
    fn decklink(&self, key: &str) -> Option<SubDevice> {
        sub_devices(&self.all()).into_iter().find(|d| d.key == key)
    }
}

/// Each sub-device's users: sources capturing from it and channel outputs playing to it.
fn uses(configs: &[ConfiguredSourceDto]) -> Vec<(String, DecklinkUseDto)> {
    let mut uses = Vec::new();
    for c in configs {
        let using = |playout| DecklinkUseDto {
            source_id: c.id.clone(),
            name: c.name.clone(),
            playout,
        };
        match &c.config {
            SourceConfig::Decklink(cfg) => uses.push((cfg.device.clone(), using(false))),
            SourceConfig::Channel(cfg) => {
                for o in &cfg.outputs {
                    if let OutputConfig::Decklink { device, .. } = o {
                        uses.push((device.clone(), using(true)));
                    }
                }
            }
            _ => {}
        }
    }
    uses
}

/// The node's cards, for `GET /decklink`.
pub fn status(devices: &LocalDevices, configs: &[ConfiguredSourceDto]) -> DecklinkStatusDto {
    let unavailable = unavailable(crate::plugins::has);
    let uses = uses(configs);
    let mut cards: Vec<DecklinkCardDto> = Vec::new();
    for d in sub_devices(&devices.all()) {
        let dto = DecklinkDeviceDto {
            key: d.key.clone(),
            name: d.name.clone(),
            capture: d.capture,
            playout: d.playout,
            modes: d.modes.iter().map(|m| DecklinkModeDto::from(*m)).collect(),
            used_by: uses
                .iter()
                .filter(|(key, _)| *key == d.key)
                .map(|(_, u)| u.clone())
                .collect(),
        };
        match cards.iter_mut().find(|c| c.key == d.card) {
            Some(card) => card.devices.push(dto),
            None => cards.push(DecklinkCardDto {
                key: d.card.clone(),
                name: d.card_name.clone(),
                profile: devices.decklink_profiles.get(&d.card),
                devices: vec![dto],
            }),
        }
    }
    DecklinkStatusDto { unavailable, cards }
}

/// Check `device` can `playout` (or capture) for `source_id`: one capture and
/// one playout per sub-device, only one on a half-duplex profile. Returns its name.
fn check_use(
    devices: &LocalDevices,
    device: &str,
    playout: bool,
    source_id: Option<&str>,
    configs: &[ConfiguredSourceDto],
    extra: &[(String, DecklinkUseDto)],
) -> Result<String, String> {
    if let Some(reason) = unavailable(crate::plugins::has) {
        return Err(reason);
    }
    let d = devices
        .decklink(device)
        .ok_or("pick a connected Decklink sub-device")?;
    let (can, doing) = if playout {
        (d.playout, "play out")
    } else {
        (d.capture, "capture")
    };
    if !can {
        return Err(format!("{} can't {doing}", d.name));
    }
    let others: Vec<(String, DecklinkUseDto)> = uses(configs)
        .into_iter()
        .filter(|(_, u)| Some(u.source_id.as_str()) != source_id)
        .chain(extra.iter().cloned())
        .filter(|(key, _)| key == device)
        .collect();
    if let Some((_, u)) = others.iter().find(|(_, u)| u.playout == playout) {
        return Err(format!("{} is already used by {}", d.name, u.name));
    }
    let profile = devices.decklink_profiles.get(&d.card);
    if let Some((_, u)) = others.first().filter(|_| half_duplex(profile)) {
        return Err(format!(
            "{} is used by {}, and the card's profile is half duplex",
            d.name, u.name
        ));
    }
    Ok(d.name)
}

/// Check a source config before it's saved and fill in the sub-device's name.
/// `id` is the source being replaced.
pub fn validate(
    cfg: &mut DecklinkSourceConfig,
    devices: &LocalDevices,
    id: Option<&str>,
    configs: &[ConfiguredSourceDto],
) -> Result<(), String> {
    cfg.device_name = check_use(devices, &cfg.device, false, id, configs, &[])?;
    if let Some(nick) = cfg.mode.as_deref().filter(|m| !m.is_empty()) {
        find_mode(nick).ok_or_else(|| format!("unknown SDI mode {nick}"))?;
    }
    if !AUDIO_CHANNELS.contains(&cfg.channels) {
        return Err("SDI audio has 2, 8 or 16 channels".into());
    }
    live::validate_format(cfg.format.as_ref())
}

/// Check a channel's Decklink outputs against the node's sub-devices.
pub fn validate_outputs(
    outputs: &[OutputConfig],
    devices: &LocalDevices,
    id: Option<&str>,
    name: &str,
    configs: &[ConfiguredSourceDto],
) -> Result<(), String> {
    let mut mine = Vec::new();
    for o in outputs {
        if let OutputConfig::Decklink { device, .. } = o {
            check_use(devices, device, true, id, configs, &mine)?;
            mine.push((
                device.clone(),
                DecklinkUseDto {
                    source_id: id.unwrap_or_default().to_string(),
                    name: name.to_string(),
                    playout: true,
                },
            ));
        }
    }
    Ok(())
}

pub struct DecklinkSource {
    id: String,
    name: String,
    config: DecklinkSourceConfig,
    devices: Arc<LocalDevices>,
    tracker: Arc<LinkTracker>,
    timecode: Arc<Mutex<Option<String>>>,
}

impl DecklinkSource {
    pub fn new(
        id: String,
        name: String,
        config: DecklinkSourceConfig,
        devices: Arc<LocalDevices>,
    ) -> Self {
        Self {
            id,
            name,
            config,
            devices,
            tracker: Arc::default(),
            timecode: Arc::default(),
        }
    }

    fn profile(&self) -> DecklinkProfile {
        self.devices
            .decklink(&self.config.device)
            .map(|d| self.devices.decklink_profiles.get(&d.card))
            .unwrap_or_default()
    }

    /// The sub-device's video and audio on a bin's pads, which the caller adds
    /// once the live bin watches for them.
    fn capture(&self, d: &SubDevice) -> Result<(gst::Bin, [gst::Pad; 2])> {
        let cfg = &self.config;
        let id = &self.id;
        let profile = self.devices.decklink_profiles.get(&d.card);
        let vsrc = d.element("decklinkvideosrc", &format!("decklink-v-{id}"), profile)?;
        match cfg.mode.as_deref().filter(|m| !m.is_empty()) {
            Some(nick) => set_property(&vsrc, "mode", nick)?,
            None => {
                set_property(&vsrc, "mode", "auto")?;
                set_property(&vsrc, "video-format", "auto")?;
            }
        }
        let connection = match cfg.connection {
            DecklinkConnection::Auto => "auto",
            DecklinkConnection::Sdi => "sdi",
            DecklinkConnection::Hdmi => "hdmi",
            DecklinkConnection::OpticalSdi => "optical-sdi",
        };
        set_property(&vsrc, "connection", connection)?;
        set_property(&vsrc, "timecode-format", "rp188any")?;
        let asrc = d.element(
            "decklinkaudiosrc",
            &format!("decklink-a-{id}"),
            DecklinkProfile::Default,
        )?;
        set_property(&asrc, "channels", &cfg.channels.to_string())?;

        let bin = gst::Bin::with_name(&format!("decklink-{id}"));
        bin.add_many([&vsrc, &asrc])
            .context("add decklink sources")?;
        let vpad = vsrc.static_pad("src").context("decklink video pad")?;
        let apad = asrc.static_pad("src").context("decklink audio pad")?;
        watch_video(&vpad, self.timecode.clone());
        Ok((
            bin,
            [ghost_pad(&vpad, "video")?, ghost_pad(&apad, "audio")?],
        ))
    }
}

/// Drop no-signal frames (flagged as gaps) so they don't count as live and the
/// compositor's black or last frame shows; keep each frame's timecode.
fn watch_video(pad: &gst::Pad, timecode: Arc<Mutex<Option<String>>>) {
    let signal = Arc::new(AtomicBool::new(true));
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let Some(gst::PadProbeData::Buffer(buffer)) = &info.data else {
            return gst::PadProbeReturn::Ok;
        };
        if buffer.flags().contains(gst::BufferFlags::GAP) {
            if signal.swap(false, Ordering::Relaxed) {
                warn!("decklink input lost its signal");
            }
            *timecode.lock().unwrap() = None;
            return gst::PadProbeReturn::Drop;
        }
        signal.store(true, Ordering::Relaxed);
        *timecode.lock().unwrap() = buffer
            .meta::<gst_video::VideoTimeCodeMeta>()
            .map(|m| m.tc().to_string());
        gst::PadProbeReturn::Ok
    });
}

impl InputSource for DecklinkSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    fn source_type(&self) -> SourceType {
        SourceType::Decklink
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        None
    }

    /// Includes the card's profile: a profile change rebuilds it.
    fn fingerprint(&self) -> String {
        format!(
            "{}|{:?}",
            serde_json::to_string(&self.config).unwrap_or_default(),
            self.profile()
        )
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let cfg = &self.config;
        let d = self.devices.decklink(&cfg.device).ok_or_else(|| {
            let name = if cfg.device_name.is_empty() {
                &cfg.device
            } else {
                &cfg.device_name
            };
            anyhow!("{name} isn't connected")
        })?;
        if !d.capture {
            bail!("{} can't capture", d.name);
        }
        let (capture, pads) = self.capture(&d)?;
        let bin = live::build_bin(LiveInput {
            id: &self.id,
            source: capture.clone().upcast(),
            audio: LiveAudio::Source {
                channels: cfg.channels,
            },
            format: cfg.format.as_ref(),
            tracker: &self.tracker,
            direct: true,
            latency: Some(LATENCY),
        })?;
        // Added now that the live bin is watching for them.
        for pad in pads {
            capture.add_pad(&pad).context("add decklink pad")?;
        }
        Ok(bin.upcast())
    }

    fn timecode(&self) -> Option<String> {
        self.timecode.lock().unwrap().clone()
    }

    fn link(&self) -> Option<LinkState> {
        Some(self.tracker.state(false))
    }
}

/// SDI out of a sub-device, audio embedded. Runs on the node clock; the sink
/// schedules on the card's clock and drops or repeats a frame as they drift.
pub struct DecklinkOutput {
    pub device: String,
    pub mode: Option<String>,
    pub program: Program,
    pub devices: Arc<LocalDevices>,
}

impl OutputSink for DecklinkOutput {
    fn label(&self) -> String {
        let name = self
            .devices
            .decklink(&self.device)
            .map(|d| d.name)
            .unwrap_or_else(|| self.device.clone());
        format!("SDI {name}")
    }

    fn start(&self, monitor: &Arc<MonitorPipeline>) -> Result<Box<dyn RunningOutput>> {
        let d = self
            .devices
            .decklink(&self.device)
            .ok_or_else(|| anyhow!("the Decklink sub-device isn't connected"))?;
        if !d.playout {
            bail!("{} can't play out", d.name);
        }
        let mode = output_mode(self.program.size, self.program.fps, self.mode.as_deref())
            .map_err(|e| anyhow!(e))?;
        let (output, vsink) = self.build(&d, mode)?;
        let pipeline = PipelineOutput::start(monitor, self.label(), output)?;
        Ok(Box::new(DecklinkRunning {
            pipeline,
            vsink,
            mode: mode.label,
        }))
    }
}

impl DecklinkOutput {
    fn build(&self, d: &SubDevice, mode: &Mode) -> Result<(OutputPipeline, gst::Element)> {
        let output = OutputPipeline::new(&format!("output-decklink-{}", d.key))?;
        let consumer = &output.consumer;
        let profile = self.devices.decklink_profiles.get(&d.card);
        let vsink = d.element("decklinkvideosink", "vsink", profile)?;
        set_property(&vsink, "mode", mode.nick)?;
        let asink = d.element("decklinkaudiosink", "asink", DecklinkProfile::Default)?;
        let [spread, acaps, numbered] = audio_spread(self.program.audio_channels)?;
        let vcaps = capsfilter(
            "vcaps",
            gst::Caps::builder("video/x-raw")
                .field("format", "UYVY")
                .field("width", mode.width as i32)
                .field("height", mode.height as i32)
                .build(),
        )?;
        consumer
            .pipeline
            .add_many([&vcaps, &vsink, &spread, &acaps, &numbered, &asink])
            .context("add Decklink output")?;
        consumer.video_chain(&[], &[&vcaps, &vsink])?;
        consumer.audio_chain(&[], &[&spread, &acaps, &numbered, &asink])?;
        Ok((output, vsink))
    }
}

/// The program's `channels` first, then silence up to what SDI carries, as
/// numbered channels.
fn audio_spread(channels: u32) -> Result<[gst::Element; 3]> {
    let channels = channels.max(1);
    let embedded = embedded_channels(channels);
    let spread = make_el("audioconvert", "aspread")?;
    set_mix_matrix(&spread, &spread_matrix(channels, embedded));
    // Mask left open for the matrix: see `numbered_channels`.
    let acaps = capsfilter(
        "acaps",
        gst::Caps::builder("audio/x-raw")
            .field("rate", 48000)
            .field("channels", embedded as i32)
            .build(),
    )?;
    Ok([spread, acaps, numbered_channels("anumbered")?])
}

/// `audioconvert` matrix putting `channels` on the first of `embedded`.
fn spread_matrix(channels: u32, embedded: u32) -> Vec<Vec<f32>> {
    (0..embedded)
        .map(|out| {
            (0..channels)
                .map(|i| {
                    // Mono goes to both channels of the first pair.
                    let hit = i == out || (channels == 1 && out == 1);
                    if hit {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect()
}

struct DecklinkRunning {
    pipeline: PipelineOutput,
    vsink: gst::Element,
    mode: &'static str,
}

impl RunningOutput for DecklinkRunning {
    fn error(&self) -> Option<String> {
        self.pipeline.error()
    }

    fn detail(&self) -> Option<String> {
        Some(self.mode.to_string())
    }

    /// Program-queue drops plus card drops (late, or skipped as its clock drifts).
    fn dropped_frames(&self) -> Option<u64> {
        let card = self
            .vsink
            .property::<gst::Structure>("stats")
            .get::<u64>("dropped")
            .unwrap_or(0);
        Some(self.pipeline.dropped_frames().unwrap_or(0) + card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mode_matches_program() {
        let m = output_mode((1920, 1080), (60000, 1001), None).unwrap();
        assert_eq!(m.nick, "1080p5994");
        let m = output_mode((3840, 2160), (25, 1), None).unwrap();
        assert_eq!(m.nick, "2160p25");
        // Same rate written differently.
        let m = output_mode((1280, 720), (120, 2), None).unwrap();
        assert_eq!(m.nick, "720p60");
        assert!(output_mode((1000, 1000), (30, 1), None).is_err());
    }

    #[test]
    fn output_mode_checks_pinned() {
        assert!(output_mode((1920, 1080), (50, 1), Some("1080p50")).is_ok());
        assert!(output_mode((1920, 1080), (50, 1), Some("1080p5994")).is_err());
        assert!(output_mode((1920, 1080), (25, 1), Some("1080i50")).is_err());
        assert!(output_mode((1920, 1080), (50, 1), Some("nope")).is_err());
        // Empty means unset.
        assert!(output_mode((1920, 1080), (50, 1), Some("")).is_ok());
    }

    #[test]
    fn embedded_channels_round_up() {
        assert_eq!(embedded_channels(1), 2);
        assert_eq!(embedded_channels(2), 2);
        assert_eq!(embedded_channels(3), 8);
        assert_eq!(embedded_channels(8), 8);
        assert_eq!(embedded_channels(12), 16);
    }

    /// Any program channel count reaches SDI as numbered channels.
    #[test]
    fn spread_negotiates_numbered_channels() {
        gst::init().unwrap();
        for channels in [1, 2, 3, 6, 8, 12, 16] {
            let pipeline = gst::parse::launch(&format!(
                "audiotestsrc num-buffers=5 ! audio/x-raw,channels={channels} \
                 ! audioconvert ! audioresample name=in"
            ))
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
            let [spread, acaps, numbered] = audio_spread(channels).unwrap();
            let sink = make_el("fakesink", "sink").unwrap();
            pipeline
                .add_many([&spread, &acaps, &numbered, &sink])
                .unwrap();
            let input = pipeline.by_name("in").unwrap();
            gst::Element::link_many([&input, &spread, &acaps, &numbered, &sink]).unwrap();
            pipeline.set_state(gst::State::Playing).unwrap();
            let msg = pipeline.bus().unwrap().timed_pop_filtered(
                gst::ClockTime::from_seconds(5),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
            let caps = sink.static_pad("sink").unwrap().current_caps();
            pipeline.set_state(gst::State::Null).unwrap();
            if let Some(gst::MessageView::Error(e)) = msg.as_ref().map(|m| m.view()) {
                panic!("{channels} channels: {} {:?}", e.error(), e.debug());
            }
            assert!(msg.is_some(), "{channels} channels: no EOS");
            let s = caps.unwrap().structure(0).unwrap().to_owned();
            assert_eq!(
                s.get::<i32>("channels").unwrap() as u32,
                embedded_channels(channels)
            );
            assert_eq!(s.get::<gst::Bitmask>("channel-mask").unwrap().0, 0);
        }
    }

    #[test]
    fn spread_pads_with_silence() {
        let m = spread_matrix(2, 8);
        assert_eq!(m.len(), 8);
        assert_eq!(m[0], vec![1.0, 0.0]);
        assert_eq!(m[1], vec![0.0, 1.0]);
        assert!(m[2..].iter().all(|r| r == &vec![0.0, 0.0]));
        let mono = spread_matrix(1, 2);
        assert_eq!(mono, vec![vec![1.0], vec![1.0]]);
    }

    #[test]
    fn card_names() {
        assert_eq!(card_name("DeckLink 8K Pro (2)"), "DeckLink 8K Pro");
        assert_eq!(
            card_name("DeckLink Mini Recorder"),
            "DeckLink Mini Recorder"
        );
    }

    #[test]
    fn modes_from_caps() {
        gst::init().unwrap();
        let caps = gst::Caps::builder("video/x-raw")
            .field("width", 1920)
            .field("height", 1080)
            .field("framerate", gst::Fraction::new(30000, 1001))
            .field("interlace-mode", "interleaved")
            .build();
        let modes = device_modes(Some(caps));
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].nick, "1080i5994");
        assert_eq!(device_modes(None).len(), MODES.len());
    }

    #[test]
    fn duplex() {
        assert!(half_duplex(DecklinkProfile::FourSubDevicesHalf));
        assert!(!half_duplex(DecklinkProfile::TwoSubDevicesFull));
        assert!(!half_duplex(DecklinkProfile::Default));
    }

    #[test]
    fn mode_nicks_exist() {
        gst::init().unwrap();
        // The table's names are the element's (when the plugin is here).
        let Ok(el) = gst::ElementFactory::make("decklinkvideosrc").build() else {
            return;
        };
        for m in MODES {
            set_property(&el, "mode", m.nick).unwrap();
        }
    }
}
