use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};

pub mod device;
pub mod file;
pub mod live;
pub mod manager;
pub mod ndi;
pub mod stream;
pub mod test;
pub mod whip;

use crate::api::types::{
    ConfiguredSourceDto, LinkState, SourceCapabilitiesDto, SourceConfig, SourceType,
};

/// Every input source implements this trait.
///
/// A source is a cheap description of where media comes from: a rescan builds
/// one for everything it finds and keeps the old one if nothing changed. The
/// GStreamer side is built by [`InputSource::build_bin`] each time a monitor
/// starts, so every monitor owns a fresh bin.
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

    /// Build a new GStreamer bin for this source: a `gst::Bin` with two
    /// named src ghost pads, `"video"` and `"audio"`. The monitor pipeline
    /// that adds it drives its state.
    fn build_bin(&self) -> Result<gst::Element>;

    /// Current timecode as `HH:MM:SS:FF`, if the source has one.
    fn timecode(&self) -> Option<String>;

    /// Whether a live source is delivering frames (`None` for other
    /// sources). Only meaningful while its monitor runs.
    fn link(&self) -> Option<LinkState> {
        None
    }
}

/// The source a stored config describes. `devices` resolves capture and
/// audio devices.
pub fn configured(
    dto: &ConfiguredSourceDto,
    devices: &Arc<device::LocalDevices>,
) -> Box<dyn InputSource> {
    let (id, name) = (dto.id.clone(), dto.name.clone());
    match &dto.config {
        SourceConfig::Test(cfg) => Box::new(test::TestSource::new(id, name, cfg.clone())),
        SourceConfig::File(cfg) => Box::new(file::FileSource::new(id, name, cfg.clone())),
        SourceConfig::Stream(cfg) => Box::new(stream::StreamSource::new(
            id,
            name,
            cfg.clone(),
            devices.clone(),
        )),
        SourceConfig::Device(cfg) => Box::new(device::DeviceSource::new(
            id,
            name,
            cfg.clone(),
            devices.clone(),
        )),
        SourceConfig::Whip(cfg) => Box::new(whip::WhipSource::new(
            id,
            name,
            cfg.clone(),
            devices.clone(),
        )),
    }
}

/// Expose `element`'s static src pad on `bin` as the ghost pad `name`
/// (`"video"` or `"audio"`, per the [`InputSource`] contract).
fn add_ghost_pad(bin: &gst::Bin, element: &gst::Element, name: &str) -> Result<()> {
    let target = element
        .static_pad("src")
        .with_context(|| format!("{} src pad", element.name()))?;
    let ghost = gst::GhostPad::builder_with_target(&target)
        .map_err(|e| anyhow!("{name} ghost pad: {e}"))?
        .name(name)
        .build();
    bin.add_pad(&ghost)
        .with_context(|| format!("add {name} ghost pad"))?;
    Ok(())
}
