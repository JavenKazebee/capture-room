//! A clip's player pipeline.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use tracing::warn;

use super::lanes::{running_time, Lanes};
use super::playout::{ClipMode, ClipRequest, ModeCell, ReplayClip};
use crate::pipeline::make_el;
use crate::sources::player::{self, Kind};

/// uridecodebin ─┬→ queue → videoconvert → appsink ⇢ video lane
///               ├→ queue → audioconvert → audioresample → appsink ⇢ audio lane
///               └→ fakesink (any other stream)
///
/// [`Streams`] says which branches the file feeds, once it has prerolled.
pub(super) fn build_player(
    clip: &ClipRequest,
    lanes: &Arc<Lanes>,
    mode: &Arc<ModeCell>,
    marks: &Arc<Marks>,
) -> Result<(gst::Pipeline, Arc<Streams>)> {
    let player = gst::Pipeline::with_name(&format!("channel-player-{}", lanes.id));
    // The program's clock: timestamps map between the two.
    player.use_clock(Some(&lanes.clock()));
    let decode = match &clip.replay {
        Some(replay) => replay_source(&player, replay, &lanes.id)?,
        None => {
            let decode = make_el("uridecodebin", &format!("channel-decode-{}", lanes.id))?;
            decode.set_property("uri", player::uri(&clip.path)?);
            player.add(&decode).context("add uridecodebin")?;
            decode
        }
    };

    let video = add_branch(&player, Kind::Video, &lanes.id, lanes, mode, marks)?;
    let audio = add_branch(&player, Kind::Audio, &lanes.id, lanes, mode, marks)?;

    let streams = Arc::new(Streams::default());
    let streams_ref = streams.clone();
    let (video, audio) = (video.downgrade(), audio.downgrade());
    let branches = (video.clone(), audio.clone());
    player::route_streams(&player, &decode, move |kind| {
        let (branch, found) = match kind {
            Kind::Video => (video.upgrade(), &streams_ref.video),
            Kind::Audio => (audio.upgrade(), &streams_ref.audio),
        };
        let sink = branch?.static_pad("sink").filter(|p| !p.is_linked())?;
        found.store(true, Ordering::SeqCst);
        Some(sink)
    });

    // A file without audio (or video) leaves that branch unlinked; remove it
    // so the player can preroll.
    let player_weak = player.downgrade();
    decode.connect_no_more_pads(move |_| {
        let Some(player) = player_weak.upgrade() else {
            return;
        };
        for branch in [&branches.0, &branches.1] {
            if let Some(first) = branch
                .upgrade()
                .filter(|b| b.static_pad("sink").is_some_and(|p| !p.is_linked()))
            {
                remove_branch(&player, &first);
            }
        }
    });

    Ok((player, streams))
}

/// splitmuxsrc over a replay span's files plus the codec's decoder, exposing
/// raw video (and PCM audio) like uridecodebin.
///
/// splitmuxsrc ─┬→ decoder ─→ "video" ghost pad
///              └→ "audio" ghost pad
fn replay_source(player: &gst::Pipeline, replay: &ReplayClip, id: &str) -> Result<gst::Element> {
    let bin = gst::Bin::with_name(&format!("channel-replay-{id}"));
    let src = make_el("splitmuxsrc", &format!("channel-replay-src-{id}"))?;
    let files = replay.span.files.clone();
    src.connect("format-location", false, move |_| Some(files.to_value()));
    let decoder = replay
        .span
        .codec
        .decoder()
        .iter()
        .map(|f| make_el(f, &format!("channel-replay-{f}-{id}")))
        .collect::<Result<Vec<_>>>()?;
    bin.add(&src).context("add splitmuxsrc")?;
    bin.add_many(&decoder).context("add replay decoder")?;
    gst::Element::link_many(&decoder).context("link replay decoder")?;
    player.add(&bin).context("add replay source")?;

    let bin_weak = bin.downgrade();
    let first = decoder[0].clone();
    let last = decoder[decoder.len() - 1].clone();
    src.connect_pad_added(move |_, pad| {
        let Some(bin) = bin_weak.upgrade() else {
            return;
        };
        let exposed = if pad.name().starts_with("video") {
            let Some(sink) = first.static_pad("sink") else {
                return;
            };
            if let Err(e) = pad.link(&sink) {
                warn!(error = ?e, "replay video doesn't fit its decoder");
                return;
            }
            last.static_pad("src").and_then(|p| {
                gst::GhostPad::builder_with_target(&p)
                    .ok()
                    .map(|g| g.name("video").build())
            })
        } else if pad.name().starts_with("audio") {
            // PCM: its caps come after the pad.
            gst::GhostPad::builder_with_target(pad)
                .ok()
                .map(|g| g.name("audio").build())
        } else {
            None
        };
        if let Some(ghost) = exposed {
            let _ = ghost.set_active(true);
            if bin.add_pad(&ghost).is_err() {
                warn!(pad = %pad.name(), "can't expose replay stream");
            }
        }
    });
    let bin_weak = bin.downgrade();
    src.connect_no_more_pads(move |_| {
        if let Some(bin) = bin_weak.upgrade() {
            bin.no_more_pads();
        }
    });
    Ok(bin.upcast())
}

/// Where a player's video is, kept by its appsink.
pub(super) struct Marks {
    /// Running time of the latest video preroll: where playing starts.
    pub(super) first_rt: AtomicU64,
    /// Position (stream time) of the latest frame shown; `u64::MAX` before
    /// one. A player's own position is off after a step.
    shown: AtomicU64,
}

impl Default for Marks {
    fn default() -> Self {
        Self {
            first_rt: AtomicU64::new(0),
            shown: AtomicU64::new(u64::MAX),
        }
    }
}

impl Marks {
    fn show(&self, sample: &gst::Sample) {
        let at = sample.buffer().and_then(|b| b.pts()).and_then(|pts| {
            sample
                .segment()?
                .downcast_ref::<gst::ClockTime>()?
                .to_stream_time(pts)
        });
        if let Some(at) = at {
            self.shown.store(at.nseconds(), Ordering::SeqCst);
        }
    }

    pub(super) fn shown(&self) -> Option<gst::ClockTime> {
        let at = self.shown.load(Ordering::SeqCst);
        (at != u64::MAX).then(|| gst::ClockTime::from_nseconds(at))
    }
}

/// Which of a player's branches its file feeds.
#[derive(Default)]
pub(super) struct Streams {
    pub(super) video: AtomicBool,
    pub(super) audio: AtomicBool,
}

/// `kind`'s branch of channel `id`'s player, pushing into its lane per `mode`;
/// video tracks its position in `marks`. Returns the first element.
fn add_branch(
    player: &gst::Pipeline,
    kind: Kind,
    id: &str,
    lanes: &Arc<Lanes>,
    mode: &Arc<ModeCell>,
    marks: &Arc<Marks>,
) -> Result<gst::Element> {
    let player_weak = player.downgrade();
    let (on_sample, on_preroll) = (lanes.clone(), lanes.clone());
    let (mode_s, mode_p) = (mode.clone(), mode.clone());
    let (marks_s, marks_p) = (marks.clone(), marks.clone());
    let callbacks = gst_app::AppSinkCallbacks::builder()
        .new_sample(move |sink| {
            let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
            // Audio only at normal speed; the program plays silence meanwhile.
            let normal = sample.segment().is_none_or(|s| s.rate() == 1.0);
            if kind == Kind::Audio && !normal {
                return Ok(gst::FlowSuccess::Ok);
            }
            if mode_s.load() == ClipMode::Playing {
                let base = player_weak.upgrade().and_then(|p| p.base_time());
                if base.is_some() {
                    on_sample.push(kind, &sample, base);
                    if kind == Kind::Video {
                        marks_s.show(&sample);
                    }
                }
            }
            Ok(gst::FlowSuccess::Ok)
        })
        .new_preroll(move |sink| {
            let sample = sink.pull_preroll().map_err(|_| gst::FlowError::Eos)?;
            if kind == Kind::Video {
                let rt = running_time(&sample).unwrap_or(gst::ClockTime::ZERO);
                marks_p.first_rt.store(rt.nseconds(), Ordering::SeqCst);
                marks_p.show(&sample);
                // Cued or paused: show now. (While playing, a preroll is
                // followed by the same buffer as a sample.)
                if mode_p.load() == ClipMode::Cued {
                    on_preroll.push(kind, &sample, None);
                }
            }
            Ok(gst::FlowSuccess::Ok)
        })
        .build();
    player::add_branch(player, &format!("channel-{id}"), kind, callbacks)
}

/// Remove the branch starting at `first` from `player`.
fn remove_branch(player: &gst::Pipeline, first: &gst::Element) {
    let mut el = Some(first.clone());
    while let Some(e) = el {
        el = e
            .static_pad("src")
            .and_then(|p| p.peer())
            .and_then(|p| p.parent_element());
        let _ = e.set_state(gst::State::Null);
        let _ = player.remove(&e);
    }
}
