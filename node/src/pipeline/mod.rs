pub mod monitor;
pub mod profile;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};

use crate::audio::AudioLevelState;
use crate::audio::AudioMeter;
use crate::audio::ChannelLevel;

// ── Shared element factory helper ─────────────────────────────────────────────

/// Create a named element and add it to the pipeline.
pub(super) fn make(pipeline: &gst::Pipeline, factory: &str, name: &str) -> Result<gst::Element> {
    let el = gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("create {factory}"))?;
    pipeline.add(&el).with_context(|| format!("add {name}"))?;
    Ok(el)
}

// ── Audio level message parsing ───────────────────────────────────────────────

pub(super) fn handle_level_message(s: &gst::StructureRef, meter: &AudioMeter) {
    use gstreamer::glib;

    let Ok(peak_arr) = s.get::<glib::ValueArray>("peak") else { return };
    let Ok(rms_arr) = s.get::<glib::ValueArray>("rms") else { return };

    let channels = peak_arr
        .iter()
        .zip(rms_arr.iter())
        .filter_map(|(p, r)| {
            Some(ChannelLevel {
                peak_db: p.get::<f64>().ok()?,
                rms_db: r.get::<f64>().ok()?,
            })
        })
        .collect();

    meter.update(AudioLevelState { channels });
}
