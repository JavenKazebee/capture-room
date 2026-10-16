use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};

#[cfg(target_os = "macos")]
mod avf;
pub mod channel;
pub mod decklink;
pub mod device;
pub mod file;
pub mod live;
pub mod manager;
pub mod ndi;
mod player;
pub mod privacy;
pub mod stream;
pub mod test;
pub mod whip;

use crate::api::types::{
    ConfiguredSourceDto, LinkState, OutputConfig, SourceCapabilitiesDto, SourceConfig, SourceType,
};

/// Every input source. A source is a cheap description a rescan can rebuild;
/// [`InputSource::build_bin`] makes a fresh bin each time a monitor starts.
pub trait InputSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    /// Rename a source a rescan keeps: the name is never part of a
    /// fingerprint, so a rename doesn't restart anything.
    fn set_name(&mut self, _name: &str) {}
    fn source_type(&self) -> SourceType;
    /// The source's format, if known before it starts producing.
    fn capabilities(&self) -> Option<SourceCapabilitiesDto>;

    /// A rescan keeps a source and its monitor only if id and fingerprint
    /// match. Leaves out the name.
    fn fingerprint(&self) -> String;

    /// A new `gst::Bin` with src ghost pads `"video"` and `"audio"`; the
    /// monitor pipeline drives its state.
    fn build_bin(&self) -> Result<gst::Element>;

    /// Current timecode as `HH:MM:SS:FF`, if the source has one.
    fn timecode(&self) -> Option<String> {
        None
    }

    /// Whether a live source is delivering frames (`None` for other
    /// sources). Only meaningful while its monitor runs.
    fn link(&self) -> Option<LinkState> {
        None
    }

    fn playout(&self) -> Option<&Arc<channel::Playout>> {
        None
    }

    fn outputs(&self) -> &[OutputConfig] {
        &[]
    }

    /// This source replaces `old` (same id, changed config): take over
    /// whatever should outlive the change.
    fn adopt(&mut self, _old: &dyn InputSource) {}
}

/// The source a stored config describes; `devices` resolves capture and audio devices.
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
        SourceConfig::Channel(cfg) => Box::new(channel::ChannelSource::new(id, name, cfg.clone())),
        SourceConfig::Whip(cfg) => Box::new(whip::WhipSource::new(
            id,
            name,
            cfg.clone(),
            devices.clone(),
        )),
        SourceConfig::Decklink(cfg) => Box::new(decklink::DecklinkSource::new(
            id,
            name,
            cfg.clone(),
            devices.clone(),
        )),
    }
}

/// Add `chain` to `bin`, link it in order, and ghost its last element as `name`.
fn add_chain(bin: &gst::Bin, chain: &[&gst::Element], name: &str) -> Result<()> {
    bin.add_many(chain.iter().copied())
        .with_context(|| format!("add {name} elements to {}", bin.name()))?;
    gst::Element::link_many(chain.iter().copied())
        .with_context(|| format!("link {name} chain in {}", bin.name()))?;
    add_ghost_pad(bin, chain.last().context("empty chain")?, name)
}

/// Expose `element`'s src pad on `bin` as the ghost pad `name`.
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
