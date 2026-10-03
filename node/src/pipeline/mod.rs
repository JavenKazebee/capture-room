pub mod monitor;
pub mod profile;
pub mod recording;

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};

use crate::api::types::ChannelLevelDto;

// ── Latest value ──────────────────────────────────────────────────────────────

/// The most recent value a pipeline produced, shared between the GStreamer
/// thread that writes it and the API that reads it. Clones share storage.
#[derive(Debug)]
pub struct Latest<T>(Arc<Mutex<Option<T>>>);

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Clone> Latest<T> {
    pub fn set(&self, value: T) {
        *self.0.lock().unwrap() = Some(value);
    }

    /// `None` until the first value arrives.
    pub fn get(&self) -> Option<T> {
        self.0.lock().unwrap().clone()
    }
}

/// Latest per-channel audio levels from the `level` element.
pub type AudioMeter = Latest<Vec<ChannelLevelDto>>;
/// Latest thumbnail JPEG.
pub type ThumbnailStore = Latest<Vec<u8>>;

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
            Some(ChannelLevelDto { peak_db: p.get::<f64>().ok()?, rms_db: r.get::<f64>().ok()? })
        })
        .collect();

    meter.set(channels);
}
