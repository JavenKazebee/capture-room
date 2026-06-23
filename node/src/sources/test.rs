use anyhow::{Context, Result};
use chrono::Timelike;
use gstreamer::{self as gst, prelude::*};
use serde::{Deserialize, Serialize};

use super::{InputSource, SourceCapabilities, SourceType, Timecode};

// ── Video pattern ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VideoTestPattern {
    Smpte,
    Snow,
    Black,
    White,
    Ball,
    #[serde(rename = "smpte75")]
    Smpte75,
    #[serde(rename = "checkers-1")]
    Checkers1,
}

impl VideoTestPattern {
    pub fn as_gst_str(&self) -> &'static str {
        match self {
            Self::Smpte => "smpte",
            Self::Snow => "snow",
            Self::Black => "black",
            Self::White => "white",
            Self::Ball => "ball",
            Self::Smpte75 => "smpte75",
            Self::Checkers1 => "checkers-1",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "snow" => Self::Snow,
            "black" => Self::Black,
            "white" => Self::White,
            "ball" => Self::Ball,
            "smpte75" => Self::Smpte75,
            "checkers-1" => Self::Checkers1,
            _ => Self::Smpte,
        }
    }

    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Smpte => "smpte",
            Self::Snow => "snow",
            Self::Black => "black",
            Self::White => "white",
            Self::Ball => "ball",
            Self::Smpte75 => "smpte75",
            Self::Checkers1 => "checkers-1",
        }
    }
}

// ── Audio signal ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AudioTestSignal {
    Tone,
    Silence,
    PinkNoise,
}

impl AudioTestSignal {
    pub fn as_gst_wave(&self) -> &'static str {
        match self {
            Self::Tone => "sine",
            Self::Silence => "silence",
            Self::PinkNoise => "pink-noise",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "silence" => Self::Silence,
            "pink-noise" => Self::PinkNoise,
            _ => Self::Tone,
        }
    }

    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Tone => "tone",
            Self::Silence => "silence",
            Self::PinkNoise => "pink-noise",
        }
    }
}

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TestSourceConfig {
    pub id: String,
    pub name: String,
    pub pattern: VideoTestPattern,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub audio_signal: AudioTestSignal,
    pub frequency: f64,
    pub channels: u32,
}

impl Default for TestSourceConfig {
    fn default() -> Self {
        Self {
            id: "test-1".into(),
            name: "Test Source 1".into(),
            pattern: VideoTestPattern::Smpte,
            width: 1920,
            height: 1080,
            fps_num: 30,
            fps_den: 1,
            audio_signal: AudioTestSignal::Tone,
            frequency: 440.0,
            channels: 2,
        }
    }
}

// ── TestSource ────────────────────────────────────────────────────────────────

pub struct TestSource {
    config: TestSourceConfig,
    bin: gst::Bin,
    connected: bool,
}

impl TestSource {
    pub fn new(config: TestSourceConfig) -> Result<Self> {
        let bin = build_bin(&config)?;
        Ok(Self { config, bin, connected: false })
    }
}

fn build_bin(cfg: &TestSourceConfig) -> Result<gst::Bin> {
    let id = &cfg.id;
    let bin = gst::Bin::with_name(&format!("testsrc-bin-{id}"));

    // ── Video: videotestsrc → capsfilter → videoconvert ───────────────────────
    let vsrc = gst::ElementFactory::make("videotestsrc")
        .name(format!("vsrc-{id}"))
        .build()
        .context("create videotestsrc")?;
    vsrc.set_property_from_str("pattern", cfg.pattern.as_gst_str());

    let vcaps = gst::ElementFactory::make("capsfilter")
        .name(format!("vcaps-{id}"))
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("width", cfg.width as i32)
                .field("height", cfg.height as i32)
                .field(
                    "framerate",
                    gst::Fraction::new(cfg.fps_num as i32, cfg.fps_den as i32),
                )
                .build(),
        )
        .build()
        .context("create video capsfilter")?;

    let vconv = gst::ElementFactory::make("videoconvert")
        .name(format!("vconv-{id}"))
        .build()
        .context("create videoconvert")?;

    // ── Audio: audiotestsrc → audioconvert → capsfilter ───────────────────────
    let asrc = gst::ElementFactory::make("audiotestsrc")
        .name(format!("asrc-{id}"))
        .build()
        .context("create audiotestsrc")?;
    asrc.set_property_from_str("wave", cfg.audio_signal.as_gst_wave());
    if cfg.audio_signal == AudioTestSignal::Tone && cfg.frequency > 0.0 {
        asrc.set_property("freq", cfg.frequency);
    }

    let aconv = gst::ElementFactory::make("audioconvert")
        .name(format!("aconv-{id}"))
        .build()
        .context("create audioconvert")?;

    let acaps = gst::ElementFactory::make("capsfilter")
        .name(format!("acaps-{id}"))
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("channels", cfg.channels as i32)
                .build(),
        )
        .build()
        .context("create audio capsfilter")?;

    for el in [&vsrc, &vcaps, &vconv, &asrc, &aconv, &acaps] {
        bin.add(el).context("add element to bin")?;
    }

    vsrc.link(&vcaps).context("link vsrc -> vcaps")?;
    vcaps.link(&vconv).context("link vcaps -> vconv")?;
    asrc.link(&aconv).context("link asrc -> aconv")?;
    aconv.link(&acaps).context("link aconv -> acaps")?;

    let video_pad = vconv.static_pad("src").context("videoconvert src pad")?;
    let ghost_video = gst::GhostPad::builder_with_target(&video_pad)
        .map_err(|e| anyhow::anyhow!("video ghost pad: {e}"))?
        .name("video")
        .build();
    bin.add_pad(&ghost_video).context("add video ghost pad")?;

    let audio_pad = acaps.static_pad("src").context("audio capsfilter src pad")?;
    let ghost_audio = gst::GhostPad::builder_with_target(&audio_pad)
        .map_err(|e| anyhow::anyhow!("audio ghost pad: {e}"))?
        .name("audio")
        .build();
    bin.add_pad(&ghost_audio).context("add audio ghost pad")?;

    Ok(bin)
}

impl InputSource for TestSource {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn display_name(&self) -> &str {
        &self.config.name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Test
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            video_formats: vec!["video/x-raw".into()],
            max_width: self.config.width,
            max_height: self.config.height,
            max_framerate: (self.config.fps_num, self.config.fps_den),
            audio_channels: self.config.channels,
            audio_sample_rates: vec![48000],
        }
    }

    fn connect(&mut self) -> Result<()> {
        self.bin
            .set_state(gst::State::Ready)
            .map_err(|e| anyhow::anyhow!("set Ready: {e:?}"))?;
        self.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) {
        let _ = self.bin.set_state(gst::State::Null);
        self.connected = false;
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn gst_src_element(&self) -> gst::Element {
        self.bin.clone().upcast()
    }

    fn timecode(&self) -> Option<Timecode> {
        let now = chrono::Utc::now();
        let fps = self.config.fps_num;
        let frames = ((now.nanosecond() as f64 / 1_000_000_000.0) * fps as f64) as u8;
        Some(Timecode {
            hours: now.hour() as u8,
            minutes: now.minute() as u8,
            seconds: now.second() as u8,
            frames,
            drop_frame: false,
            framerate: (self.config.fps_num, self.config.fps_den),
        })
    }

    fn is_available(&self) -> bool {
        true
    }
}
