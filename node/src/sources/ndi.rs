//! NDI sources. Unlike other types they're discovered, not configured: each
//! sender on the network is a source. A node connects to every sender or only
//! the chosen ones and those in use (`ndi_subscriptions`), and never to its own
//! outputs.
//!
//! `ndisrc` timestamps in its `auto` mode, which puts two nodes receiving the
//! same sender within about 2 ms of each other once settled, so replay nodes
//! can each keep their own buffer of the same angle.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};
use tracing::warn;

use super::{add_ghost_pad, InputSource};
use crate::api::types::{SourceCapabilitiesDto, SourceType};
use crate::pipeline::make_el;

pub struct NdiSource {
    id: String,
    ndi_name: String,
    url_address: String,
    name: String,
    /// What the sender sees this node as (see [`receiver_name`]).
    receiver: String,
}

/// The name a node receives NDI under, so senders and Access Manager can tell nodes apart.
pub fn receiver_name(node_name: &str) -> String {
    format!("capture-room ({node_name})")
}

fn build_bin(id: &str, ndi_name: &str, url_address: &str, receiver: &str) -> Result<gst::Bin> {
    let bin = gst::Bin::with_name(&format!("ndisrc-bin-{id}"));

    let src = make_el("ndisrc", &format!("ndisrc-{id}"))?;
    src.set_property("ndi-name", ndi_name);
    src.set_property("url-address", url_address);
    src.set_property("receiver-ndi-name", receiver);
    let demux = make_el("ndisrcdemux", &format!("ndisrcdemux-{id}"))?;

    // Their static src pads anchor the ghost pads; the demux links its dynamic
    // pads to their sinks once the stream flows.
    let vconv = make_el("videoconvert", &format!("ndi-vconv-{id}"))?;
    let aconv = make_el("audioconvert", &format!("ndi-aconv-{id}"))?;

    bin.add_many([&src, &demux, &vconv, &aconv])
        .context("add elements to NDI bin")?;
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
        let Some(sink) = conv.upgrade().and_then(|c| c.static_pad("sink")) else {
            return;
        };
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

/// A DeviceMonitor kept running for the app's lifetime. The NDI provider is a
/// singleton: stopping and restarting it leaves its FindInstance stale and
/// later starts silently find nothing.
pub struct NdiMonitor {
    monitor: gst::DeviceMonitor,
    /// The NDI library loaded. The plugin tries once per process, so a
    /// runtime installed later needs a restart.
    runtime_loaded: bool,
    /// Given to new connections; a rename leaves connected ones as they are.
    receiver: String,
}

impl NdiMonitor {
    /// Blocking: waits 1.5 s for the provider's first poll. Call once via `spawn_blocking`.
    pub fn start() -> Self {
        let monitor = gst::DeviceMonitor::new();
        monitor.add_filter(Some("Source/Audio/Video/Network"), None);
        if let Err(e) = monitor.start() {
            warn!("NDI device monitor failed to start: {e}");
        } else {
            // The SDK blocks up to 1 s on first poll; 1.5 s gives margin.
            std::thread::sleep(Duration::from_millis(1500));
        }
        // A provider that fails to start (no NDI library) is dropped.
        let runtime_loaded = monitor
            .providers()
            .iter()
            .any(|p| p.as_str() == "ndideviceprovider");
        Self {
            monitor,
            runtime_loaded,
            receiver: receiver_name("capture-room"),
        }
    }

    /// Receive as `node_name` from now on.
    pub fn set_node_name(&mut self, node_name: &str) {
        self.receiver = receiver_name(node_name);
    }

    pub fn runtime_loaded(&self) -> bool {
        self.runtime_loaded
    }

    pub fn current_sources(&self) -> Vec<NdiSource> {
        self.monitor
            .devices()
            .into_iter()
            .filter_map(|d| device_to_source(d, &self.receiver))
            .collect()
    }
}

impl Drop for NdiMonitor {
    fn drop(&mut self) {
        self.monitor.stop();
    }
}

fn device_to_source(device: gst::Device, receiver: &str) -> Option<NdiSource> {
    let props = device.properties()?;
    let ndi_name: String = props.get("ndi-name").ok()?;
    Some(NdiSource {
        id: ndi_source_id(&ndi_name),
        url_address: props.get("url-address").ok().unwrap_or_default(),
        name: device.display_name().to_string(),
        ndi_name,
        receiver: receiver.to_string(),
    })
}

fn ndi_source_id(ndi_name: &str) -> String {
    let slug: String = ndi_name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("ndi-{slug}")
}

impl NdiSource {
    /// The sender's NDI name, `HOST (name)`.
    pub fn ndi_name(&self) -> &str {
        &self.ndi_name
    }

    /// A sender as the monitor would find it, for tests.
    #[cfg(test)]
    pub fn named(ndi_name: &str) -> Self {
        Self {
            id: ndi_source_id(ndi_name),
            ndi_name: ndi_name.into(),
            url_address: String::new(),
            name: ndi_name.into(),
            receiver: receiver_name("test"),
        }
    }
}

/// A node's own NDI outputs, as senders on the network: NDI names each one
/// `HOST (name)`, after the machine, in its own case.
pub struct OwnSenders {
    pub host: String,
    pub names: HashSet<String>,
}

impl OwnSenders {
    pub fn contains(&self, ndi_name: &str) -> bool {
        ndi_name
            .split_once(" (")
            .and_then(|(host, rest)| Some((host, rest.strip_suffix(')')?)))
            .is_some_and(|(host, name)| {
                host.eq_ignore_ascii_case(&self.host) && self.names.contains(name)
            })
    }
}

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

    fn build_bin(&self) -> Result<gst::Element> {
        Ok(build_bin(&self.id, &self.ndi_name, &self.url_address, &self.receiver)?.upcast())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_senders_match_host_and_name() {
        let own = OwnSenders {
            host: "studio-mac".into(),
            names: HashSet::from(["Program".to_string()]),
        };
        assert!(own.contains("STUDIO-MAC (Program)"));
        assert!(!own.contains("STUDIO-MAC (Camera 1)"));
        assert!(!own.contains("OTHER-MAC (Program)"));
        assert!(!own.contains("Program"));
    }
}
