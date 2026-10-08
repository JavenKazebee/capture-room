pub mod monitor;
pub mod output;
pub mod profile;
pub mod recording;
pub mod rtsp;

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

// ── Element builders ──────────────────────────────────────────────────────────

pub(crate) fn make_el(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("create {factory} (is its GStreamer plugin installed?)"))
}

pub(crate) fn capsfilter(name: &str, caps: gst::Caps) -> Result<gst::Element> {
    gst::ElementFactory::make("capsfilter")
        .name(name)
        .property("caps", caps)
        .build()
        .with_context(|| format!("create {name}"))
}

/// Set `audioconvert`'s `mix-matrix`: a row of input gains per output
/// channel. It has to be set before the element negotiates.
pub(crate) fn set_mix_matrix(aconv: &gst::Element, rows: &[Vec<f32>]) {
    let matrix = gst::Array::new(
        rows.iter()
            .map(|row| gst::Array::new(row.iter().copied()).to_send_value()),
    );
    aconv.set_property("mix-matrix", matrix);
}

/// Link a new request pad of `tee` to `sink`'s static sink pad.
pub(super) fn link_tee(tee: &gst::Element, sink: &gst::Element) -> Result<()> {
    tee.request_pad_simple("src_%u")
        .with_context(|| format!("{} request pad", tee.name()))?
        .link(
            &sink
                .static_pad("sink")
                .with_context(|| format!("{} sink pad", sink.name()))?,
        )
        .with_context(|| format!("link {} → {}", tee.name(), sink.name()))?;
    Ok(())
}

// ── Audio level message parsing ───────────────────────────────────────────────

pub(super) fn handle_level_message(s: &gst::StructureRef, meter: &AudioMeter) {
    use gstreamer::glib;

    let Ok(peak_arr) = s.get::<glib::ValueArray>("peak") else {
        return;
    };
    let Ok(rms_arr) = s.get::<glib::ValueArray>("rms") else {
        return;
    };

    let channels = peak_arr
        .iter()
        .zip(rms_arr.iter())
        .filter_map(|(p, r)| {
            Some(ChannelLevelDto {
                peak_db: p.get::<f64>().ok()?,
                rms_db: r.get::<f64>().ok()?,
            })
        })
        .collect();

    meter.set(channels);
}
