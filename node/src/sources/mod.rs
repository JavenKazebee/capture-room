use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};

pub mod manager;
pub mod ndi;
pub mod test;

use crate::api::types::{SourceCapabilitiesDto, SourceType};

/// Every input source implements this trait.
///
/// The `gst::Element` returned by `gst_src_element` is always a `gst::Bin`
/// with two named src ghost pads: `"video"` and `"audio"`. The bin is created
/// at construction; the monitor pipeline that adds it drives its state.
pub trait InputSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    /// The source's format, if known before it starts producing.
    fn capabilities(&self) -> Option<SourceCapabilitiesDto>;

    /// Identifies the configuration the bin was built from. A rescan keeps a
    /// source (and its running monitor) only if the id and fingerprint match;
    /// otherwise the source is rebuilt and its monitor restarted.
    fn fingerprint(&self) -> String;

    /// Returns the source's GStreamer bin (video + audio ghost pads).
    fn gst_src_element(&self) -> gst::Element;

    /// Current timecode as `HH:MM:SS:FF`, if the source has one.
    fn timecode(&self) -> Option<String>;
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
