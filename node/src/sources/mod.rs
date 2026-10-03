use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};

pub mod manager;
pub mod ndi;
pub mod test;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceType {
    Test,
    Ndi,
}

/// What a source can produce. Used by the pipeline layer to negotiate caps
/// and by the UI to display source info.
#[derive(Debug, Clone)]
pub struct SourceCapabilities {
    pub video_formats: Vec<String>,
    pub max_width: u32,
    pub max_height: u32,
    /// (numerator, denominator)
    pub max_framerate: (u32, u32),
    pub audio_channels: u32,
    pub audio_sample_rates: Vec<u32>,
}

/// SMPTE-style timecode.
#[derive(Debug, Clone)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
    /// (numerator, denominator)
    pub framerate: (u32, u32),
}

impl std::fmt::Display for Timecode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, sep, self.frames
        )
    }
}

/// Every input source implements this trait.
///
/// The `gst::Element` returned by `gst_src_element` is always a `gst::Bin`
/// with two named src ghost pads: `"video"` and `"audio"`. The bin is created
/// at construction; the monitor pipeline that adds it drives its state.
pub trait InputSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    fn capabilities(&self) -> SourceCapabilities;

    /// Identifies the configuration the bin was built from. A rescan keeps a
    /// source (and its running monitor) only if the id and fingerprint match;
    /// otherwise the source is rebuilt and its monitor restarted.
    fn fingerprint(&self) -> String;

    /// Returns the source's GStreamer bin (video + audio ghost pads).
    fn gst_src_element(&self) -> gst::Element;

    fn timecode(&self) -> Option<Timecode>;
    fn is_available(&self) -> bool;
}

/// Expose `element`'s static src pad on `bin` as the ghost pad `name`
/// (`"video"` or `"audio"`, per the [`InputSource`] contract).
fn add_ghost_pad(bin: &gst::Bin, element: &gst::Element, name: &str) -> Result<()> {
    let target = element.static_pad("src").with_context(|| format!("{} src pad", element.name()))?;
    let ghost = gst::GhostPad::builder_with_target(&target)
        .map_err(|e| anyhow!("{name} ghost pad: {e}"))?
        .name(name)
        .build();
    bin.add_pad(&ghost).with_context(|| format!("add {name} ghost pad"))?;
    Ok(())
}
