//! Shared pieces for sources that decode in their own player pipeline and
//! push into appsrcs in the monitor's bin: file sources and playout channels.

use anyhow::{Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;
use tracing::{debug, warn};

use crate::pipeline::{make_el, set_property};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Video,
    Audio,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Audio => "audio",
        }
    }

    /// What converts a decoded stream for its appsink.
    fn elements(self) -> &'static [&'static str] {
        match self {
            Kind::Video => &["queue", "videoconvert"],
            Kind::Audio => &["queue", "audioconvert", "audioresample"],
        }
    }
}

pub fn uri(path: &str) -> Result<String> {
    Ok(glib::filename_to_uri(path, None)
        .with_context(|| format!("make a URI for {path}"))?
        .to_string())
}

/// How much a [`live_appsrc`] queues before it drops its oldest buffers.
pub enum QueueLimit {
    Buffers(u64),
    Time(gst::ClockTime),
}

/// A live appsrc in time format that drops its oldest buffers past `limit`
/// rather than block the player.
pub fn live_appsrc(
    name: &str,
    min_latency: gst::ClockTime,
    limit: QueueLimit,
) -> Result<gst_app::AppSrc> {
    let (max_buffers, max_time) = match limit {
        QueueLimit::Buffers(n) => (n, None),
        QueueLimit::Time(t) => (0, Some(t)),
    };
    let src = gst_app::AppSrc::builder()
        .name(name)
        .is_live(true)
        .format(gst::Format::Time)
        .max_bytes(0)
        .max_time(max_time)
        .min_latency(min_latency.nseconds() as i64)
        // Without its own latency a live appsrc reports a maximum of 0,
        // which the aggregators can't configure.
        .max_latency(-1)
        .build();
    src.set_property("max-buffers", max_buffers);
    set_property(src.upcast_ref(), "leaky-type", "downstream")?;
    Ok(src)
}

/// Add `kind`'s branch to `player`: converters → clock-paced appsink feeding
/// `callbacks`. Returns the first element, for the decoder to link to.
pub fn add_branch(
    player: &gst::Pipeline,
    prefix: &str,
    kind: Kind,
    callbacks: gst_app::AppSinkCallbacks,
) -> Result<gst::Element> {
    let tag = kind.tag();
    let mut chain = kind
        .elements()
        .iter()
        .map(|factory| make_el(factory, &format!("{prefix}-{factory}-{tag}")))
        .collect::<Result<Vec<_>>>()?;
    let sink = gst_app::AppSink::builder()
        .name(format!("{prefix}-sink-{tag}"))
        .caps(&gst::Caps::new_empty_simple(format!("{tag}/x-raw")))
        // Paced by the clock: this is what plays the media in real time.
        .sync(true)
        .max_buffers(2)
        .build();
    sink.set_callbacks(callbacks);
    chain.push(sink.upcast());
    player
        .add_many(&chain)
        .with_context(|| format!("add {tag} branch"))?;
    gst::Element::link_many(&chain).with_context(|| format!("link {tag} branch"))?;
    Ok(chain.swap_remove(0))
}

/// Link each video or audio stream `decode` exposes to `branch`'s sink pad;
/// drain others into a fakesink so the demuxer doesn't stop on an unlinked pad.
pub fn route_streams(
    player: &gst::Pipeline,
    decode: &gst::Element,
    branch: impl Fn(Kind) -> Option<gst::Pad> + Send + Sync + 'static,
) {
    let player_weak = player.downgrade();
    decode.connect_pad_added(move |_, pad| {
        let Some(player) = player_weak.upgrade() else {
            return;
        };
        let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
        // A replay's pads are named for what they carry, and its audio's
        // caps come later.
        let media = match pad.name().as_str() {
            "video" => "video/x-raw",
            "audio" => "audio/x-raw",
            _ => caps
                .structure(0)
                .map(|s| s.name().as_str())
                .unwrap_or_default(),
        };
        let kind = if media.starts_with("video/") {
            Some(Kind::Video)
        } else if media.starts_with("audio/") {
            Some(Kind::Audio)
        } else {
            None
        };
        let sink = match kind.and_then(&branch) {
            Some(sink) => sink,
            None => {
                let Ok(fake) = gst::ElementFactory::make("fakesink")
                    .property("sync", true)
                    .property("async", false)
                    .build()
                else {
                    return;
                };
                if player.add(&fake).is_err() || fake.sync_state_with_parent().is_err() {
                    return;
                }
                let Some(sink) = fake.static_pad("sink") else {
                    return;
                };
                debug!(player = %player.name(), pad = %pad.name(), media, "stream not used");
                sink
            }
        };
        if let Err(e) = pad.link(&sink) {
            warn!(player = %player.name(), pad = %pad.name(), error = ?e, "player pad link failed");
        }
    });
}

/// `sample`'s buffer stamped `pts` (no DTS), `DISCONT` if `discont`; `src`
/// gets the sample's caps first if they changed.
pub fn restamp(
    sample: &gst::Sample,
    src: &gst_app::AppSrc,
    pts: gst::ClockTime,
    discont: bool,
) -> Option<gst::Buffer> {
    let mut buffer = sample.buffer_owned()?;
    if let Some(caps) = sample.caps() {
        if src.caps().is_none_or(|c| c.as_ref() != caps) {
            src.set_caps(Some(&caps.to_owned()));
        }
    }
    {
        let buffer = buffer.make_mut();
        buffer.set_pts(pts);
        buffer.set_dts(gst::ClockTime::NONE);
        if discont {
            buffer.set_flags(gst::BufferFlags::DISCONT);
        }
    }
    Some(buffer)
}
