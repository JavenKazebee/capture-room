use anyhow::{bail, Result};
use gstreamer as gst;

/// Every GStreamer element the application may need.
///
/// Grouped by the system package that provides it so the error message
/// can tell the user exactly what to install.
const REQUIRED: &[(&str, &str, &str)] = &[
    // (element, package-name, package-manager-hint)
    // ── gst-plugins-base (should always be present) ──────────────────────────
    ("videoconvert",  "gst-plugins-base",  ""),
    ("audioconvert",  "gst-plugins-base",  ""),
    ("videoscale",    "gst-plugins-base",  ""),
    ("videotestsrc",  "gst-plugins-base",  ""),
    ("audiotestsrc",  "gst-plugins-base",  ""),
    // ── gst-plugins-good ─────────────────────────────────────────────────────
    ("qtmux",         "gst-plugins-good",  "pacman -S gst-plugins-good  /  apt install gstreamer1.0-plugins-good"),
    ("mp4mux",        "gst-plugins-good",  "pacman -S gst-plugins-good  /  apt install gstreamer1.0-plugins-good"),
    ("matroskamux",   "gst-plugins-good",  "pacman -S gst-plugins-good  /  apt install gstreamer1.0-plugins-good"),
    ("opusenc",       "gst-plugins-good",  "pacman -S gst-plugins-good  /  apt install gstreamer1.0-plugins-good"),
    // ── gst-plugins-ugly ─────────────────────────────────────────────────────
    ("x264enc",       "gst-plugins-ugly",  "pacman -S gst-plugins-ugly  /  apt install gstreamer1.0-plugins-ugly"),
    // ── gst-libav ────────────────────────────────────────────────────────────
    ("avenc_prores",  "gst-libav",         "pacman -S gst-libav  /  apt install gstreamer1.0-libav"),
    ("avenc_aac",     "gst-libav",         "pacman -S gst-libav  /  apt install gstreamer1.0-libav"),
    // ── gst-plugins-bad ──────────────────────────────────────────────────────
    ("mxfmux",        "gst-plugins-bad",   "pacman -S gst-plugins-bad  /  apt install gstreamer1.0-plugins-bad"),
    ("jpegenc",       "gst-plugins-bad",   "pacman -S gst-plugins-bad  /  apt install gstreamer1.0-plugins-bad"),
    ("level",         "gst-plugins-bad",   "pacman -S gst-plugins-bad  /  apt install gstreamer1.0-plugins-bad"),
];

/// Check that every required GStreamer element is registered on this machine.
///
/// Returns an error listing all missing elements and the packages that provide
/// them, so the user (or installer) knows exactly what to install.
pub fn check_required_plugins() -> Result<()> {
    let mut missing: Vec<(&str, &str, &str)> = Vec::new();

    for &(element, package, hint) in REQUIRED {
        if gst::ElementFactory::find(element).is_none() {
            missing.push((element, package, hint));
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    // Group by package for a readable error message
    let mut by_package: Vec<(&str, Vec<&str>, &str)> = Vec::new();
    for (element, package, hint) in &missing {
        if let Some(entry) = by_package.iter_mut().find(|(p, _, _)| p == package) {
            entry.1.push(element);
        } else {
            by_package.push((package, vec![element], hint));
        }
    }

    let mut msg = String::from("Missing GStreamer plugins — install the following packages:\n");
    for (package, elements, hint) in &by_package {
        msg.push_str(&format!("\n  {package}  (provides: {})\n", elements.join(", ")));
        if !hint.is_empty() {
            msg.push_str(&format!("    install: {hint}\n"));
        }
    }
    msg.push_str(
        "\nOn macOS/Windows, install the GStreamer runtime from https://gstreamer.freedesktop.org/download/\n"
    );

    bail!(msg)
}
