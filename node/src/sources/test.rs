use anyhow::Result;
use chrono::Timelike;
use gstreamer::{self as gst, prelude::*};

use super::{add_chain, InputSource};
use crate::api::types::{
    AudioTestSignal, SourceCapabilitiesDto, SourceType, TestSourceConfig, VideoTestPattern,
};
use crate::pipeline::{capsfilter, make_el, set_property};

pub struct TestSource {
    id: String,
    name: String,
    config: TestSourceConfig,
}

impl TestSource {
    pub fn new(id: String, name: String, config: TestSourceConfig) -> Self {
        Self { id, name, config }
    }
}

fn build_bin(id: &str, cfg: &TestSourceConfig) -> Result<gst::Bin> {
    let bin = gst::Bin::with_name(&format!("testsrc-bin-{id}"));

    let vsrc = make_el("videotestsrc", &format!("vsrc-{id}"))?;
    vsrc.set_property("is-live", true);
    set_property(&vsrc, "pattern", pattern_gst_name(cfg.pattern))?;
    let vcaps = capsfilter(
        &format!("vcaps-{id}"),
        // videotestsrc draws limited range whatever it negotiates; unlabelled,
        // a full-range consumer (a JPEG encoder) mislabels it.
        gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("colorimetry", "bt709")
            .field("width", cfg.width as i32)
            .field("height", cfg.height as i32)
            .field(
                "framerate",
                gst::Fraction::new(cfg.fps_num as i32, cfg.fps_den as i32),
            )
            .build(),
    )?;
    let vconv = make_el("videoconvert", &format!("vconv-{id}"))?;
    add_chain(&bin, &[&vsrc, &vcaps, &vconv], "video")?;

    let asrc = make_el("audiotestsrc", &format!("asrc-{id}"))?;
    asrc.set_property("is-live", true);
    set_property(&asrc, "wave", signal_gst_wave(cfg.audio_signal))?;
    if cfg.audio_signal == AudioTestSignal::Tone && cfg.frequency > 0.0 {
        asrc.set_property("freq", cfg.frequency);
    }
    let acaps = capsfilter(
        &format!("acaps-{id}"),
        gst::Caps::builder("audio/x-raw")
            .field("channels", cfg.channels as i32)
            .build(),
    )?;
    let aconv = make_el("audioconvert", &format!("aconv-{id}"))?;
    add_chain(&bin, &[&asrc, &aconv, &acaps], "audio")?;

    Ok(bin)
}

impl InputSource for TestSource {
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
        SourceType::Test
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        Some(SourceCapabilitiesDto {
            max_width: self.config.width,
            max_height: self.config.height,
            fps_num: self.config.fps_num,
            fps_den: self.config.fps_den,
            audio_channels: self.config.channels,
        })
    }

    fn fingerprint(&self) -> String {
        format!("{:?}", self.config)
    }

    fn build_bin(&self) -> Result<gst::Element> {
        Ok(build_bin(&self.id, &self.config)?.upcast())
    }

    fn timecode(&self) -> Option<String> {
        let now = chrono::Utc::now();
        let fps = self.config.fps_num as f64 / self.config.fps_den.max(1) as f64;
        // nanosecond() exceeds 1e9 during a leap second; keep frames in range.
        let frac = (now.nanosecond() as f64 / 1_000_000_000.0).min(0.999_999);
        let frames = (frac * fps) as u32;
        Some(format!(
            "{:02}:{:02}:{:02}:{frames:02}",
            now.hour(),
            now.minute(),
            now.second()
        ))
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
