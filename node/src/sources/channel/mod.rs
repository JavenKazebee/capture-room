//! Playout channels.
//!
//! A channel's *program* is a source like any other: a bin with `"video"` and
//! `"audio"` pads, run by an ordinary monitor pipeline (so it has thumbnails
//! and meters, and can be recorded). The bin is a compositor over black and a
//! mixer over silence at the channel's fixed format (see
//! [`live::video_output`]), so the program never goes dark or changes format.
//!
//! A loaded clip plays in its own *player* pipeline, paced by the clock, whose
//! appsinks push into appsrcs added to the program bin for that clip (its
//! *lanes*). Removing the lanes leaves black and silence. A bad file fails
//! its player, never the program.
//!
//! Timestamps: a player buffer at running time `rt` is rendered when the clock
//! reads `player base time + rt`. It's restamped to that moment in the
//! program's running time, plus [`LEAD`] so it reaches the compositor ahead
//! of its slot. The same mapping applies to video and audio, so they stay in
//! sync, and since a paused pipeline moves its base time on resume, across
//! pauses too.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};

use super::{live, InputSource};
use crate::api::types::{
    ChannelConfig, LiveVideoFormat, OutputConfig, SourceCapabilitiesDto, SourceType,
};

mod lanes;
mod player;
mod playout;
#[cfg(test)]
mod tests;
mod validate;

pub use lanes::PlayedProgress;
pub use playout::{ClipRequest, PlaylistItem, Playout, PlayoutStatus, ReplayClip};
pub use validate::{listen_ports, validate};

/// How far ahead of its slot a clip's buffer is stamped. Covers the
/// program-side conversion, so frames are never late at the compositor.
const LEAD: gst::ClockTime = gst::ClockTime::from_mseconds(100);
/// How long a clip may take to load (open, decode and preroll).
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a frame step may take to show its frame.
const STEP_TIMEOUT: Duration = Duration::from_millis(300);

// ── ChannelSource ─────────────────────────────────────────────────────────────

pub struct ChannelSource {
    id: String,
    name: String,
    config: ChannelConfig,
    playout: Arc<Playout>,
}

impl ChannelSource {
    pub fn new(id: String, name: String, config: ChannelConfig) -> Self {
        let playout = Arc::new(Playout::new(&id));
        Self {
            id,
            name,
            config,
            playout,
        }
    }
}

impl InputSource for ChannelSource {
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
        SourceType::Channel
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        let f = &self.config.format;
        Some(SourceCapabilitiesDto {
            max_width: f.width,
            max_height: f.height,
            max_framerate: [f.fps_num, f.fps_den],
            audio_channels: self.config.audio_channels,
        })
    }

    /// Outputs are left out: changing them restarts the outputs, not the
    /// program (and the clip playing on it).
    fn fingerprint(&self) -> String {
        format!("{:?}|{}", self.config.format, self.config.audio_channels)
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let id = &self.id;
        let bin = gst::Bin::with_name(&format!("channel-bin-{id}"));
        let (video, _) = live::video_output(&bin, id, program_caps(&self.config.format))?;
        let compositor = video.aggregator().context("compositor")?;
        live::add_background(&bin, id, &compositor)?;
        let audio = live::audio_output(&bin, id, self.config.audio_channels, false)?;
        self.playout.lanes.attach(video, audio);
        Ok(bin.upcast())
    }

    fn timecode(&self) -> Option<String> {
        None
    }

    fn playout(&self) -> Option<&Arc<Playout>> {
        Some(&self.playout)
    }

    fn outputs(&self) -> &[OutputConfig] {
        &self.config.outputs
    }

    /// The transport carries over: what's on air plays on into the new
    /// program, and the playlist stays.
    fn adopt(&mut self, old: &dyn InputSource) {
        if let Some(playout) = old.playout() {
            self.playout = playout.clone();
        }
    }
}

/// The program's video caps: the channel's size and rate, progressive I420.
fn program_caps(format: &LiveVideoFormat) -> gst::Caps {
    let base = gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("colorimetry", "bt709")
        .field("chroma-site", "mpeg2")
        .build();
    live::lock_caps(&base, Some(format))
}
