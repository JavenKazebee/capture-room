use anyhow::{bail, Result};
use gstreamer::{self as gst, prelude::*};

use crate::api::types::{SourceType, SourceTypeSupport};

/// Every GStreamer element the app may need, grouped by the package that
/// provides it. Includes every preset encoder, so a missing one fails at
/// startup rather than on Record.
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

pub fn has(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// The elements of `names` this node doesn't have.
pub fn missing<'a>(names: &[&'a str]) -> Vec<&'a str> {
    names.iter().copied().filter(|e| !has(e)).collect()
}

/// Source types that need more than the required plugins, and whether this
/// node can run them; a missing element makes a type unavailable rather than
/// stopping the node. `can_display`: the node has a window system.
pub fn source_types(can_display: bool) -> Vec<SourceTypeSupport> {
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
        .any(|f| !["ndideviceprovider", "decklinkdeviceprovider"].contains(&f.name().as_str()));
    if !has_provider {
        device.missing.push("a capture device provider".into());
    }
    let whip = support(
        SourceType::Whip,
        &["whipserversrc", "webrtcbin", "nicesrc", "decodebin3"],
    );
    // A channel runs with any outputs this node can send, or none (it can
    // still be recorded).
    let mut channel = support(
        SourceType::Channel,
        &["compositor", "audiomixer", "uridecodebin"],
    );
    if ["ndisinkcombiner", "ndisink"].iter().all(|e| has(e)) {
        channel.protocols.push("ndi".into());
    }
    let srt = crate::pipeline::output::SRT_ELEMENTS.iter().all(|e| has(e))
        && crate::pipeline::output::LIVE_H264.iter().any(|e| has(e));
    if srt {
        channel.protocols.push("srt".into());
    }
    let rtsp = crate::pipeline::rtsp::RTSP_ELEMENTS.iter().all(|e| has(e))
        && crate::pipeline::output::LIVE_H264.iter().any(|e| has(e));
    if rtsp {
        channel.protocols.push("rtsp".into());
    }
    if crate::pipeline::whep::WHEP_ELEMENTS.iter().all(|e| has(e)) {
        channel.protocols.push("whep".into());
    }
    if can_display && has(crate::pipeline::output::DISPLAY_SINK) {
        channel.protocols.push("display".into());
    }
    // Decklink needs the driver too: unavailable without it, with why.
    let mut decklink = support(SourceType::Decklink, &[]);
    match crate::sources::decklink::unavailable(has) {
        Some(reason) => decklink.missing.push(reason),
        None => channel.protocols.push("decklink".into()),
    }
    vec![stream, device, whip, channel, decklink]
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

/// Folders that may hold the NDI library, most official first. The plugin only
/// looks in `NDI_RUNTIME_DIR_V6`/`_V5` and the default library path, but on
/// macOS the SDK has its own folder and NDI Tools bundles private copies.
#[cfg(target_os = "macos")]
const NDI_DIRS: &[&str] = &[
    "/Library/NDI SDK for Apple/lib/macOS",
    // NDI Tools: its camera plugin, then its apps.
    "/Library/CoreMediaIO/Plug-Ins/DAL/NDIVideoOut.plugin/Contents/Frameworks",
    "/Applications/NDI Scan Converter.app/Contents/Frameworks",
    "/Applications/NDI Router.app/Contents/Frameworks/NTFramework.framework/Versions/A/Frameworks",
];
#[cfg(not(target_os = "macos"))]
const NDI_DIRS: &[&str] = &[];

/// Point the NDI plugin at an installed library it wouldn't find, unless one
/// is set already. Call before any threads start: it sets an env var.
pub fn locate_ndi_runtime() -> Option<&'static str> {
    if ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
    {
        return None;
    }
    let dir = NDI_DIRS
        .iter()
        .find(|d| std::path::Path::new(d).join("libndi.dylib").is_file())?;
    std::env::set_var("NDI_RUNTIME_DIR_V6", dir);
    Some(dir)
}

/// From the macOS app bundle, use its own `GStreamer.framework` (so no other
/// GStreamer on the machine is mixed in) and a registry of its own. Call
/// before any threads start: it sets environment variables.
#[cfg(target_os = "macos")]
pub fn use_bundled_gstreamer() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let root = exe
        .parent()?
        .join("../Frameworks/GStreamer.framework/Versions/1.0")
        .canonicalize()
        .ok()?;
    let set = |var: &str, path: std::path::PathBuf| std::env::set_var(var, path);
    set("GST_PLUGIN_SYSTEM_PATH_1_0", root.join("lib/gstreamer-1.0"));
    set(
        "GST_PLUGIN_SCANNER_1_0",
        root.join("libexec/gstreamer-1.0/gst-plugin-scanner"),
    );
    set(
        "GST_PTP_HELPER_1_0",
        root.join("libexec/gstreamer-1.0/gst-ptp-helper"),
    );
    set("GIO_MODULE_DIR", root.join("lib/gio/modules"));
    if let Some(home) = std::env::home_dir() {
        set(
            "GST_REGISTRY_1_0",
            home.join("Library/Caches/capture-room/gstreamer-registry.bin"),
        );
    }
    std::env::remove_var("GST_PLUGIN_PATH_1_0");
    std::env::remove_var("GST_PLUGIN_PATH");
    Some(root)
}

/// From the installed Windows app, use the GStreamer beside it (`bin`,
/// `lib\gstreamer-1.0`, `libexec`), not any other on the machine, and a
/// registry of its own. The exe's own folder is searched first for the DLLs
/// it links; `PATH` covers the plugin scanner and the plugins' dependencies.
/// Call before any threads start: it sets environment variables.
#[cfg(target_os = "windows")]
pub fn use_bundled_gstreamer() -> Option<std::path::PathBuf> {
    let root = crate::service::app_path()?;
    let set = |var: &str, path: std::path::PathBuf| std::env::set_var(var, path);
    set(
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        root.join(r"lib\gstreamer-1.0"),
    );
    let libexec = root.join(r"libexec\gstreamer-1.0");
    for (var, exe) in [
        ("GST_PLUGIN_SCANNER_1_0", "gst-plugin-scanner.exe"),
        ("GST_PTP_HELPER_1_0", "gst-ptp-helper.exe"),
    ] {
        if libexec.join(exe).is_file() {
            set(var, libexec.join(exe));
        }
    }
    set("GIO_MODULE_DIR", root.join(r"lib\gio\modules"));
    set(
        "GST_REGISTRY_1_0",
        crate::service::data_dir().join("gstreamer-registry.bin"),
    );
    std::env::remove_var("GST_PLUGIN_PATH_1_0");
    std::env::remove_var("GST_PLUGIN_PATH");
    let mut path = vec![root.join("bin")];
    path.extend(
        std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths),
    );
    if let Ok(joined) = std::env::join_paths(path) {
        std::env::set_var("PATH", joined);
    }
    Some(root)
}

/// Check every required element is registered; the error lists what's missing
/// and the packages that provide it.
pub fn check_required_plugins() -> Result<()> {
    let mut msg = String::new();
    for package in REQUIRED {
        let missing = missing(package.elements);
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
