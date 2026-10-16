//! Lanes: the program side of a clip.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use tracing::warn;

use super::LEAD;
use crate::pipeline::monitor::{FrameCount, VideoProgress};
use crate::sources::live::{Chain, Inputs};
use crate::sources::player::{self, Kind, QueueLimit};

/// The appsrcs a loaded clip pushes into, in the current program bin. The
/// monitor builds a new bin each (re)start; [`Lanes::attach`] moves the clip over.
pub struct Lanes {
    pub(super) id: String,
    pub(super) inner: Mutex<LanesInner>,
    played: Played,
}

/// How a playing clip's video has reached the program, for the benchmark.
#[derive(Default)]
struct Played {
    /// In program time, with the gaps the lane marked (a seek, a loop).
    frames: FrameCount,
    /// Frames that reached the lane after their slot; the compositor drops them.
    late: AtomicU64,
}

/// How far a channel's playing clip has got; see [`Playout::played`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayedProgress {
    /// Like a monitor's, in program time.
    pub progress: VideoProgress,
    pub late: u64,
}

#[derive(Default)]
pub(super) struct LanesInner {
    video_in: Option<Inputs>,
    audio_in: Option<Inputs>,
    serial: u32,
    /// Whether a clip wants lanes, and an audio one.
    want: Option<bool>,
    video: Lane,
    audio: Lane,
    #[cfg(test)]
    pub(super) pushed: Vec<Pushed>,
}

#[derive(Default)]
struct Lane {
    src: Option<gst_app::AppSrc>,
    chain: Option<Chain>,
    /// Start and end (program time) of the last buffer pushed.
    last: Option<(gst::ClockTime, gst::ClockTime)>,
}

/// A pushed buffer, for tests: its clip position (stream time) and program timestamp.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct Pushed {
    pub(super) kind: Kind,
    pub(super) clip: gst::ClockTime,
    pub(super) program: gst::ClockTime,
    pub(super) preroll: bool,
}

impl Lanes {
    pub(super) fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            inner: Mutex::default(),
            played: Played::default(),
        }
    }

    /// A new program bin was built: its inputs replace the old bin's, and a
    /// loaded clip gets lanes in it.
    pub(super) fn attach(&self, video: Inputs, audio: Inputs) {
        let mut inner = self.inner.lock().unwrap();
        inner.video_in = Some(video);
        inner.audio_in = Some(audio);
        inner.video = Lane::default();
        inner.audio = Lane::default();
        if let Some(audio) = inner.want {
            if let Err(e) = self.open_locked(&mut inner, audio) {
                warn!(channel = %self.id, error = %e, "channel lanes failed");
            }
        }
    }

    /// The program's clock, which a player must share for timestamps to map.
    /// The system clock until the program runs.
    pub(super) fn clock(&self) -> gst::Clock {
        let inner = self.inner.lock().unwrap();
        inner
            .video_in
            .as_ref()
            .and_then(Inputs::bin)
            .and_then(|bin| bin.clock())
            .unwrap_or_else(gst::SystemClock::obtain)
    }

    /// Add lanes for a clip (replacing any).
    pub(super) fn open(&self, audio: bool) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        self.close_locked(&mut inner);
        inner.want = Some(audio);
        self.open_locked(&mut inner, audio)
    }

    /// Like [`Lanes::open`], but keeps existing lanes and what they've queued,
    /// so one clip follows another without a gap.
    pub(super) fn ensure(&self, audio: bool) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.want = Some(audio);
        self.open_locked(&mut inner, audio)
    }

    fn open_locked(&self, inner: &mut LanesInner, audio: bool) -> Result<()> {
        // No bin yet: the lanes are made when the monitor builds one.
        let (Some(video_in), Some(audio_in)) = (inner.video_in.clone(), inner.audio_in.clone())
        else {
            return Ok(());
        };
        if inner.video.src.is_none() {
            inner.serial += 1;
            inner.video = self.lane(&video_in, &format!("v{}", inner.serial))?;
        }
        if !audio {
            if let Some(chain) = std::mem::take(&mut inner.audio).chain {
                audio_in.remove(chain);
            }
        } else if inner.audio.src.is_none() {
            inner.serial += 1;
            inner.audio = self.lane(&audio_in, &format!("a{}", inner.serial))?;
        }
        Ok(())
    }

    /// The base time a player must run at for its buffer at `rt` to follow on
    /// from what the video lane has queued (or play now if later). `None` until
    /// the program runs.
    pub(super) fn follow_on_base(&self, rt: gst::ClockTime) -> Option<gst::ClockTime> {
        let inner = self.inner.lock().unwrap();
        let src = inner.video.src.as_ref()?;
        let now = src.current_running_time()? + LEAD;
        let at = inner.video.last.map_or(now, |(_, end)| end.max(now));
        (src.base_time()? + at).checked_sub(LEAD + rt)
    }

    fn lane(&self, inputs: &Inputs, tag: &str) -> Result<Lane> {
        let bin = inputs.bin().context("program bin gone")?;
        let mut chain = inputs.add(tag)?;
        // Buffers arrive LEAD early; hold that and some, then drop the oldest
        // rather than block the player.
        let src = player::live_appsrc(
            &format!("channel-src-{tag}-{}", self.id),
            gst::ClockTime::ZERO,
            QueueLimit::Time(gst::ClockTime::from_mseconds(500)),
        );
        bin.add(&src).context("add lane")?;
        src.static_pad("src")
            .context("lane src pad")?
            .link(&chain.sink().context("lane chain sink")?)
            .context("link lane")?;
        src.sync_state_with_parent().context("start lane")?;
        chain.prepend(src.clone().upcast());
        Ok(Lane {
            src: Some(src),
            chain: Some(chain),
            last: None,
        })
    }

    /// Remove the clip's lanes: the program goes to black and silence.
    pub(super) fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.want = None;
        self.close_locked(&mut inner);
    }

    fn close_locked(&self, inner: &mut LanesInner) {
        for (lane, inputs) in [
            (std::mem::take(&mut inner.video), inner.video_in.clone()),
            (std::mem::take(&mut inner.audio), inner.audio_in.clone()),
        ] {
            if let (Some(chain), Some(inputs)) = (lane.chain, inputs) {
                inputs.remove(chain);
            }
        }
    }

    /// Push a player's buffer into its lane. `player_base` is the player's base
    /// time while playing; `None` for a preroll (cued or paused frame), shown now.
    pub(super) fn push(
        &self,
        kind: Kind,
        sample: &gst::Sample,
        player_base: Option<gst::ClockTime>,
    ) {
        let Some(duration) = sample.buffer().map(|b| b.duration()) else {
            return;
        };
        let (src, last) = {
            let inner = self.inner.lock().unwrap();
            let lane = match kind {
                Kind::Video => &inner.video,
                Kind::Audio => &inner.audio,
            };
            let Some(src) = lane.src.clone() else { return };
            (src, lane.last)
        };
        let Some(mut pts) = program_pts(sample, &src, player_base) else {
            return;
        };
        // A preroll shows after what's queued from playing, or it'd be dropped as late.
        if let (None, Some((_, end))) = (player_base, last) {
            pts = pts.max(end);
        }
        // Never backwards or repeated: after a seek, preroll or loop, buffers
        // stamped before those already queued are dropped.
        if last.is_some_and(|(start, end)| {
            pts <= start || pts + gst::ClockTime::from_mseconds(1) < end
        }) {
            return;
        }
        let end = last.map(|(_, end)| end);
        // A gap (resume, seek): let the mixer resync rather than wait.
        let gap = end.is_some_and(|end| pts > end + gst::ClockTime::from_mseconds(20));
        let Some(buffer) = player::restamp(sample, &src, pts, gap) else {
            return;
        };
        {
            let mut inner = self.inner.lock().unwrap();
            let lane = match kind {
                Kind::Video => &mut inner.video,
                Kind::Audio => &mut inner.audio,
            };
            // Lanes replaced meanwhile.
            if lane.src.as_ref() != Some(&src) {
                return;
            }
            lane.last = Some((pts, pts + duration.unwrap_or(gst::ClockTime::ZERO)));
            if kind == Kind::Video && player_base.is_some() {
                self.count_played(&src, pts, end);
            }
            #[cfg(test)]
            if let Some(clip) = sample.buffer().and_then(|b| b.pts()).and_then(|pts| {
                sample
                    .segment()?
                    .downcast_ref::<gst::ClockTime>()?
                    .to_stream_time(pts)
            }) {
                inner.pushed.push(Pushed {
                    kind,
                    clip,
                    program: pts,
                    preroll: player_base.is_none(),
                });
            }
        }
        // Fails only while the program is stopping.
        let _ = src.push_buffer(buffer);
    }
}

impl Lanes {
    /// Count a played video frame stamped `pts`. Late if the program is past
    /// it (`pts` includes [`LEAD`], so an on-time frame is ahead).
    fn count_played(
        &self,
        src: &gst_app::AppSrc,
        pts: gst::ClockTime,
        end: Option<gst::ClockTime>,
    ) {
        let played = &self.played;
        if src.current_running_time().is_some_and(|now| now > pts) {
            played.late.fetch_add(1, Ordering::Relaxed);
        }
        // A gap the lane marks DISCONT (see `push`).
        let gap = end
            .filter(|&end| pts > end + gst::ClockTime::from_mseconds(20))
            .map_or(gst::ClockTime::ZERO, |end| pts - end);
        played.frames.record(Some(pts), gap);
    }

    pub(super) fn played(&self) -> PlayedProgress {
        let played = &self.played;
        PlayedProgress {
            progress: played.frames.progress(),
            late: played.late.load(Ordering::Relaxed),
        }
    }
}

/// `sample`'s running time in its player.
pub(super) fn running_time(sample: &gst::Sample) -> Option<gst::ClockTime> {
    let pts = sample.buffer()?.pts()?;
    let segment = sample.segment()?.downcast_ref::<gst::ClockTime>()?;
    segment.to_running_time(pts)
}

/// When `sample` is due in the program's running time, plus [`LEAD`].
fn program_pts(
    sample: &gst::Sample,
    src: &gst_app::AppSrc,
    player_base: Option<gst::ClockTime>,
) -> Option<gst::ClockTime> {
    let Some(player_base) = player_base else {
        // A preroll: now. The program isn't running yet without a time.
        return Some(src.current_running_time()? + LEAD);
    };
    let rt = running_time(sample)?;
    (player_base + rt + LEAD).checked_sub(src.base_time()?)
}
