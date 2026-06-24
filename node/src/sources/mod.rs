use anyhow::Result;
use gstreamer as gst;

pub mod manager;
pub mod ndi;
pub mod registry;
pub mod test;

/// Whether the source should be connected (monitor pipeline started) automatically
/// on discovery, or only on an explicit user request.
///
/// Use `Auto` for sources that are cheap to open and harmless to hold open
/// (TestSource, NDI). Use `Manual` for sources that reserve exclusive hardware
/// (Decklink) or have significant connection cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    Auto,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceType {
    Test,
    Ndi,
    Decklink,
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
/// with two named src ghost pads: `"video"` and `"audio"`. The bin is
/// created at construction; `connect` advances it to `Ready` state.
pub trait InputSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    fn capabilities(&self) -> SourceCapabilities;

    /// Advance the internal GStreamer bin to `Ready` state.
    /// Must be called before adding the element to a pipeline.
    fn connect(&mut self) -> Result<()>;

    /// Return the bin to `Null` state and release resources.
    fn disconnect(&mut self);

    /// Returns the source's GStreamer bin (video + audio ghost pads).
    /// The bin is valid as soon as the source is constructed; no need
    /// to call `connect` first just to obtain the element reference.
    fn gst_src_element(&self) -> gst::Element;

    fn timecode(&self) -> Option<Timecode>;
    fn is_available(&self) -> bool;

    fn is_connected(&self) -> bool {
        false
    }

    fn connection_mode(&self) -> ConnectionMode {
        ConnectionMode::Auto
    }
}
