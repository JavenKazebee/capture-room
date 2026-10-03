use anyhow::{Context, Result};
use chrono::Timelike;
use gstreamer::{self as gst, prelude::*};

use super::{InputSource, SourceCapabilities, SourceType, Timecode};
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
    let vsrc = gst::ElementFactory::make("videotestsrc")
        .name(format!("vsrc-{id}"))
        .property("is-live", true)
        .build()
        .context("create videotestsrc")?;
    vsrc.set_property_from_str("pattern", pattern_gst_name(cfg.pattern));

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
        .property("is-live", true)
        .build()
        .context("create audiotestsrc")?;
    asrc.set_property_from_str("wave", signal_gst_wave(cfg.audio_signal));
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

    fn is_available(&self) -> bool {
        true
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
