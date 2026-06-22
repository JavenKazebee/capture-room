use anyhow::{Context, Result};
use chrono::Timelike;
use gstreamer::{self as gst, prelude::*};

use super::{InputSource, SourceCapabilities, SourceType, Timecode};

pub struct TestSource {
    id: String,
    display_name: String,
    width: u32,
    height: u32,
    framerate_num: u32,
    framerate_den: u32,
    audio_channels: u32,
    bin: gst::Bin,
    connected: bool,
}

impl TestSource {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        width: u32,
        height: u32,
        framerate_num: u32,
        framerate_den: u32,
        audio_channels: u32,
    ) -> Result<Self> {
        let id = id.into();
        let bin = build_bin(&id, width, height, framerate_num, framerate_den, audio_channels)?;
        Ok(Self {
            id,
            display_name: display_name.into(),
            width,
            height,
            framerate_num,
            framerate_den,
            audio_channels,
            bin,
            connected: false,
        })
    }

    /// Convenience constructor with 1080p30 stereo defaults.
    pub fn default_config(id: impl Into<String>, display_name: impl Into<String>) -> Result<Self> {
        Self::new(id, display_name, 1920, 1080, 30, 1, 2)
    }
}

fn build_bin(
    id: &str,
    width: u32,
    height: u32,
    framerate_num: u32,
    framerate_den: u32,
    audio_channels: u32,
) -> Result<gst::Bin> {
    let bin = gst::Bin::with_name(&format!("testsrc-bin-{id}"));

    // ── Video branch: videotestsrc ! capsfilter ! videoconvert ──────────────
    let vsrc = gst::ElementFactory::make("videotestsrc")
        .name(format!("vsrc-{id}"))
        .build()
        .context("create videotestsrc")?;

    let vcaps = gst::ElementFactory::make("capsfilter")
        .name(format!("vcaps-{id}"))
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("width", width as i32)
                .field("height", height as i32)
                .field(
                    "framerate",
                    gst::Fraction::new(framerate_num as i32, framerate_den as i32),
                )
                .build(),
        )
        .build()
        .context("create video capsfilter")?;

    let vconv = gst::ElementFactory::make("videoconvert")
        .name(format!("vconv-{id}"))
        .build()
        .context("create videoconvert")?;

    // ── Audio branch: audiotestsrc ! audioconvert ! capsfilter ──────────────
    let asrc = gst::ElementFactory::make("audiotestsrc")
        .name(format!("asrc-{id}"))
        .build()
        .context("create audiotestsrc")?;

    let aconv = gst::ElementFactory::make("audioconvert")
        .name(format!("aconv-{id}"))
        .build()
        .context("create audioconvert")?;

    let acaps = gst::ElementFactory::make("capsfilter")
        .name(format!("acaps-{id}"))
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("channels", audio_channels as i32)
                .build(),
        )
        .build()
        .context("create audio capsfilter")?;

    // Add all elements to the bin
    for el in [&vsrc, &vcaps, &vconv, &asrc, &aconv, &acaps] {
        bin.add(el).context("add element to bin")?;
    }

    // Link within each branch
    vsrc.link(&vcaps).context("link vsrc -> vcaps")?;
    vcaps.link(&vconv).context("link vcaps -> vconv")?;
    asrc.link(&aconv).context("link asrc -> aconv")?;
    aconv.link(&acaps).context("link aconv -> acaps")?;

    // Expose ghost pads named "video" and "audio"
    let video_pad = vconv.static_pad("src").context("videoconvert src pad")?;
    let ghost_video = gst::GhostPad::builder_with_target(&video_pad)
        .map_err(|e| anyhow::anyhow!("video ghost pad builder: {e}"))?
        .name("video")
        .build();
    bin.add_pad(&ghost_video)
        .context("add video ghost pad")?;

    let audio_pad = acaps.static_pad("src").context("audio capsfilter src pad")?;
    let ghost_audio = gst::GhostPad::builder_with_target(&audio_pad)
        .map_err(|e| anyhow::anyhow!("audio ghost pad builder: {e}"))?
        .name("audio")
        .build();
    bin.add_pad(&ghost_audio)
        .context("add audio ghost pad")?;

    Ok(bin)
}

impl InputSource for TestSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Test
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            video_formats: vec!["video/x-raw".into()],
            max_width: self.width,
            max_height: self.height,
            max_framerate: (self.framerate_num, self.framerate_den),
            audio_channels: self.audio_channels,
            audio_sample_rates: vec![48000],
        }
    }

    fn connect(&mut self) -> Result<()> {
        self.bin
            .set_state(gst::State::Ready)
            .map_err(|e| anyhow::anyhow!("set Ready state: {e:?}"))?;
        self.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) {
        let _ = self.bin.set_state(gst::State::Null);
        self.connected = false;
    }

    fn gst_src_element(&self) -> gst::Element {
        self.bin.clone().upcast()
    }

    fn timecode(&self) -> Option<Timecode> {
        let now = chrono::Utc::now();
        let fps = self.framerate_num;
        let frames = ((now.nanosecond() as f64 / 1_000_000_000.0) * fps as f64) as u8;
        Some(Timecode {
            hours: now.hour() as u8,
            minutes: now.minute() as u8,
            seconds: now.second() as u8,
            frames,
            drop_frame: false,
            framerate: (self.framerate_num, self.framerate_den),
        })
    }

    fn is_available(&self) -> bool {
        true
    }
}
