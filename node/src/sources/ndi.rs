use std::time::Duration;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};
use tracing::{info, warn};

use super::{add_ghost_pad, InputSource};
use crate::api::types::{SourceCapabilitiesDto, SourceType};
use crate::pipeline::make_el;

// ── NdiSource ──────────────────────────────────────────────────────────────────

pub struct NdiSource {
    id: String,
    ndi_name: String,
    url_address: String,
    name: String,
    bin: gst::Bin,
}

impl NdiSource {
    pub fn new(
        id: String,
        ndi_name: String,
        url_address: String,
        display_name: String,
    ) -> Result<Self> {
        let bin = build_bin(&id, &ndi_name, &url_address)?;
        Ok(Self { id, ndi_name, url_address, name: display_name, bin })
    }
}

fn build_bin(id: &str, ndi_name: &str, url_address: &str) -> Result<gst::Bin> {
    let bin = gst::Bin::with_name(&format!("ndisrc-bin-{id}"));

    let src = make_el("ndisrc", &format!("ndisrc-{id}"))?;
    src.set_property("ndi-name", ndi_name);
    src.set_property("url-address", url_address);
    src.set_property("receiver-ndi-name", "capture-room");
    let demux = make_el("ndisrcdemux", &format!("ndisrcdemux-{id}"))?;

    // Intermediate converters. Their static src pads anchor the ghost pads at
    // construction time; the demux links its dynamic video/audio pads to their
    // sinks once the stream starts flowing.
    let vconv = make_el("videoconvert", &format!("ndi-vconv-{id}"))?;
    let aconv = make_el("audioconvert", &format!("ndi-aconv-{id}"))?;

    bin.add_many([&src, &demux, &vconv, &aconv]).context("add elements to NDI bin")?;
    src.link(&demux).context("link ndisrc → ndisrcdemux")?;

    let vconv_weak = vconv.downgrade();
    let aconv_weak = aconv.downgrade();
    demux.connect_pad_added(move |_demux, pad| {
        let name = pad.name();
        let conv = if name.starts_with("video") {
            &vconv_weak
        } else if name.starts_with("audio") {
            &aconv_weak
        } else {
            return;
        };
        let Some(sink) = conv.upgrade().and_then(|c| c.static_pad("sink")) else { return };
        if !sink.is_linked() {
            if let Err(e) = pad.link(&sink) {
                warn!("NDI {name} pad link failed: {e:?}");
            }
        }
    });

    add_ghost_pad(&bin, &vconv, "video")?;
    add_ghost_pad(&bin, &aconv, "audio")?;

    Ok(bin)
}

// ── Persistent device monitor ─────────────────────────────────────────────────

/// Wraps a GStreamer DeviceMonitor that stays running for the lifetime of the
/// app. Querying `current_sources()` is instant — no wait required after the
/// initial startup delay.
///
/// The NDI device provider is a GStreamer singleton. Starting and stopping the
/// monitor on every scan leaves the provider's internal FindInstance in a
/// stale state, causing subsequent starts to silently do nothing. Keeping the
/// monitor alive avoids that entirely.
pub struct NdiMonitor {
    monitor: gst::DeviceMonitor,
}

impl NdiMonitor {
    /// Blocking. Starts the monitor and waits for the NDI device provider's
    /// first poll (up to 1 s per the NDI SDK). Call once via `spawn_blocking`.
    pub fn start() -> Self {
        let monitor = gst::DeviceMonitor::new();
        monitor.add_filter(Some("Source/Audio/Video/Network"), None);
        if let Err(e) = monitor.start() {
            warn!("NDI device monitor failed to start: {e}");
        } else {
            // NDI SDK blocks up to 1 s on first poll; 1.5 s gives a safe margin.
            std::thread::sleep(Duration::from_millis(1500));
        }
        Self { monitor }
    }

    /// Returns all NDI sources currently visible on the network.
    pub fn current_sources(&self) -> Vec<NdiSource> {
        self.monitor
            .devices()
            .into_iter()
            .filter_map(device_to_source)
            .collect()
    }
}

impl Drop for NdiMonitor {
    fn drop(&mut self) {
        self.monitor.stop();
    }
}

fn device_to_source(device: gst::Device) -> Option<NdiSource> {
    let props = device.properties()?;
    let ndi_name: String = props.get("ndi-name").ok()?;
    let url_address: String = props.get("url-address").ok().unwrap_or_default();
    let display_name = device.display_name().to_string();
    let id = ndi_source_id(&ndi_name);
    match NdiSource::new(id, ndi_name.clone(), url_address, display_name) {
        Ok(src) => {
            info!(ndi_name = %ndi_name, "found NDI source");
            Some(src)
        }
        Err(e) => {
            warn!(ndi_name = %ndi_name, error = %e, "failed to create NDI source");
            None
        }
    }
}

fn ndi_source_id(ndi_name: &str) -> String {
    let slug: String = ndi_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    format!("ndi-{slug}")
}

// ── InputSource impl ──────────────────────────────────────────────────────────

impl InputSource for NdiSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Ndi
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        // NDI negotiates the format at runtime.
        None
    }

    fn fingerprint(&self) -> String {
        format!("{}|{}", self.ndi_name, self.url_address)
    }

    fn gst_src_element(&self) -> gst::Element {
        self.bin.clone().upcast()
    }

    fn timecode(&self) -> Option<String> {
        None
    }
}
