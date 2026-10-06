use anyhow::{bail, Result};
use gstreamer::{self as gst, prelude::*};

use crate::api::types::{SourceType, SourceTypeSupport};

/// Every GStreamer element the application may need, grouped by the system
/// package that provides it so the error message can say exactly what to
/// install. Includes every encoder a preset can select, so a missing one is
/// caught at startup rather than when someone presses record.
const REQUIRED: &[Package] = &[
    Package {
        name: "gst-plugins-base",
        hint: "",
        elements: &[
            "videoconvert",
            "audioconvert",
            "audioresample",
            "videoscale",
            "videotestsrc",
            "audiotestsrc",
        ],
    },
    Package {
        name: "gst-plugins-good",
        hint: "pacman -S gst-plugins-good  /  apt install gstreamer1.0-plugins-good",
        elements: &[
            "qtmux",
            "mp4mux",
            "matroskamux",
            "opusenc",
            "vp9enc",
            "jpegenc",
            "level",
            "videorate",
        ],
    },
    Package {
        name: "gst-plugins-ugly",
        hint: "pacman -S gst-plugins-ugly  /  apt install gstreamer1.0-plugins-ugly",
        elements: &["x264enc"],
    },
    Package {
        name: "gst-plugins-bad",
        hint: "pacman -S gst-plugins-bad  /  apt install gstreamer1.0-plugins-bad",
        elements: &["h264parse", "h265parse", "x265enc"],
    },
    Package {
        name: "gst-libav",
        hint: "pacman -S gst-libav  /  apt install gstreamer1.0-libav",
        elements: &["avenc_prores_ks", "avenc_aac"],
    },
    // Statically linked.
    Package {
        name: "gst-plugin-ndi",
        hint: "",
        elements: &["ndisrc", "ndisrcdemux"],
    },
    Package {
        name: "gst-plugin-fallbackswitch",
        hint: "",
        elements: &["fallbacksrc"],
    },
];

/// Whether this node has GStreamer element `name`.
pub fn has(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// Source types that need more than the required plugins, and whether this
/// node can run them. A type missing an element is unavailable rather than
/// stopping the node.
pub fn source_types() -> Vec<SourceTypeSupport> {
    let stream_protocols: Vec<String> = crate::sources::stream::PROTOCOLS
        .iter()
        .filter(|(_, element)| has(element))
        .map(|(scheme, _)| scheme.to_string())
        .collect();
    let mut stream = support(SourceType::Stream, &["uridecodebin3"]);
    if stream.missing.is_empty() && stream_protocols.is_empty() {
        stream.missing.push("rtspsrc".into());
    }
    stream.protocols = stream_protocols;
    let mut device = support(SourceType::Device, &[]);
    // Device providers ship with the platform's plugins (video4linux2 /
    // pipewire, applemedia, mediafoundation / wasapi2).
    let has_provider = gst::DeviceProviderFactory::factories(gst::Rank::NONE)
        .iter()
        .any(|f| f.name() != "ndideviceprovider");
    if !has_provider {
        device.missing.push("a capture device provider".into());
    }
    let whip = support(
        SourceType::Whip,
        &["whipserversrc", "webrtcbin", "nicesrc", "decodebin3"],
    );
    vec![stream, device, whip]
}

fn support(source_type: SourceType, elements: &[&str]) -> SourceTypeSupport {
    SourceTypeSupport {
        source_type,
        missing: elements
            .iter()
            .filter(|e| !has(e))
            .map(|e| e.to_string())
            .collect(),
        protocols: Vec::new(),
    }
}

struct Package {
    name: &'static str,
    hint: &'static str,
    elements: &'static [&'static str],
}

/// Check that every required GStreamer element is registered on this machine.
///
/// Returns an error listing all missing elements and the packages that provide
/// them, so the user (or installer) knows exactly what to install.
pub fn check_required_plugins() -> Result<()> {
    let mut msg = String::new();
    for package in REQUIRED {
        let missing: Vec<&str> = package
            .elements
            .iter()
            .copied()
            .filter(|e| gst::ElementFactory::find(e).is_none())
            .collect();
        if missing.is_empty() {
            continue;
        }
        msg.push_str(&format!(
            "\n  {}  (provides: {})\n",
            package.name,
            missing.join(", ")
        ));
        if !package.hint.is_empty() {
            msg.push_str(&format!("    install: {}\n", package.hint));
        }
    }

    if msg.is_empty() {
        return Ok(());
    }
    bail!(
        "Missing GStreamer plugins — install the following packages:\n{msg}\n\
         On macOS/Windows, install the GStreamer runtime from https://gstreamer.freedesktop.org/download/\n"
    )
}
