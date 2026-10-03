use anyhow::{Context, Result};
use chrono::Timelike;
use gstreamer::{self as gst, prelude::*};

use super::{add_ghost_pad, InputSource, SourceCapabilities, SourceType, Timecode};
use crate::pipeline::{capsfilter, make_el};
use crate::api::types::{AudioTestSignal, TestSourceConfigDto, VideoTestPattern};

// ── TestSource ────────────────────────────────────────────────────────────────

pub struct TestSource {
    config: TestSourceConfigDto,
    bin: gst::Bin,
}

impl TestSource {
    pub fn new(config: TestSourceConfigDto) -> Result<Self> {
        let bin = build_bin(&config)?;
        Ok(Self { config, bin })
    }
}

fn build_bin(cfg: &TestSourceConfigDto) -> Result<gst::Bin> {
    let id = &cfg.id;
    let bin = gst::Bin::with_name(&format!("testsrc-bin-{id}"));

    // ── Video: videotestsrc → capsfilter → videoconvert ───────────────────────
    let vsrc = make_el("videotestsrc", &format!("vsrc-{id}"))?;
    vsrc.set_property("is-live", true);
    vsrc.set_property_from_str("pattern", pattern_gst_name(cfg.pattern));
    let vcaps = capsfilter(
        &format!("vcaps-{id}"),
        gst::Caps::builder("video/x-raw")
            .field("width", cfg.width as i32)
            .field("height", cfg.height as i32)
            .field("framerate", gst::Fraction::new(cfg.fps_num as i32, cfg.fps_den as i32))
            .build(),
    )?;
    let video = [vsrc, vcaps, make_el("videoconvert", &format!("vconv-{id}"))?];

    // ── Audio: audiotestsrc → audioconvert → capsfilter ───────────────────────
    let asrc = make_el("audiotestsrc", &format!("asrc-{id}"))?;
    asrc.set_property("is-live", true);
    asrc.set_property_from_str("wave", signal_gst_wave(cfg.audio_signal));
    if cfg.audio_signal == AudioTestSignal::Tone && cfg.frequency > 0.0 {
        asrc.set_property("freq", cfg.frequency);
    }
    let acaps = capsfilter(
        &format!("acaps-{id}"),
        gst::Caps::builder("audio/x-raw").field("channels", cfg.channels as i32).build(),
    )?;
    let audio = [asrc, make_el("audioconvert", &format!("aconv-{id}"))?, acaps];

    bin.add_many(video.iter().chain(&audio)).context("add elements to test bin")?;
    gst::Element::link_many(&video).context("link test video chain")?;
    gst::Element::link_many(&audio).context("link test audio chain")?;
    add_ghost_pad(&bin, &video[2], "video")?;
    add_ghost_pad(&bin, &audio[2], "audio")?;

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
            max_width: self.config.width,
            max_height: self.config.height,
            max_framerate: (self.config.fps_num, self.config.fps_den),
            audio_channels: self.config.channels,
        }
    }

    fn fingerprint(&self) -> String {
        format!("{:?}", self.config)
    }

    fn gst_src_element(&self) -> gst::Element {
        self.bin.clone().upcast()
    }

    fn timecode(&self) -> Option<Timecode> {
        let now = chrono::Utc::now();
        let fps = self.config.fps_num as f64 / self.config.fps_den.max(1) as f64;
        // nanosecond() exceeds 1e9 during a leap second; keep frames in range.
        let frac = (now.nanosecond() as f64 / 1_000_000_000.0).min(0.999_999);
        let frames = (frac * fps) as u8;
        Some(Timecode {
            hours: now.hour() as u8,
            minutes: now.minute() as u8,
            seconds: now.second() as u8,
            frames,
            drop_frame: false,
            framerate: (self.config.fps_num, self.config.fps_den),
        })
    }
}

fn pattern_gst_name(pattern: VideoTestPattern) -> &'static str {
    match pattern {
        VideoTestPattern::Smpte => "smpte",
        VideoTestPattern::Snow => "snow",
        VideoTestPattern::Black => "black",
        VideoTestPattern::White => "white",
        VideoTestPattern::Ball => "ball",
        VideoTestPattern::Smpte75 => "smpte75",
        VideoTestPattern::Checkers1 => "checkers-1",
    }
}

fn signal_gst_wave(signal: AudioTestSignal) -> &'static str {
    match signal {
        AudioTestSignal::Tone => "sine",
        AudioTestSignal::Silence => "silence",
        AudioTestSignal::PinkNoise => "pink-noise",
    }
}
