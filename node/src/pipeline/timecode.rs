//! SMPTE timecode on video frames, as a `VideoTimeCodeMeta` that qtmux
//! writes into a .mov's timecode track. Recordings and replay exports stamp
//! the time of day ([`stamp_time_of_day`]); a live source carries its own
//! (a Decklink's RP188) through its compositor.

use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_video as gst_video;

/// 29.97 and 59.94 count drop-frame, as their timecode does.
pub(crate) fn drop_frame_flags(fps: gst::Fraction) -> gst_video::VideoTimeCodeFlags {
    if fps.denom() == 1001 && fps.numer() % 30000 == 0 {
        gst_video::VideoTimeCodeFlags::DROP_FRAME
    } else {
        gst_video::VideoTimeCodeFlags::empty()
    }
}

/// `pad`'s current time segment, if it has had one.
pub(crate) fn time_segment(pad: &gst::Pad) -> Option<gst::FormattedSegment<gst::ClockTime>> {
    let event = pad.sticky_event::<gst::event::Segment>(0)?;
    event.segment().downcast_ref::<gst::ClockTime>().cloned()
}

/// The running time of `pts` in `pad`'s segment: `None` without a segment,
/// or outside it.
pub(crate) fn running_time(pad: &gst::Pad, pts: gst::ClockTime) -> Option<gst::ClockTime> {
    time_segment(pad)?.to_running_time(pts)
}

/// The frame rate of `pad`'s current caps, if it has a usable one.
fn framerate(pad: &gst::Pad) -> Option<gst::Fraction> {
    pad.current_caps()
        .and_then(|c| c.structure(0)?.get::<gst::Fraction>("framerate").ok())
        .filter(|f| f.numer() > 0 && f.denom() > 0)
}

/// Stamp each frame leaving `pad` with its local time of day as SMPTE
/// timecode. `to_utc_us` maps a PTS in its segment to UTC µs; using the clock
/// domain's offset makes files from nodes on one clock line up. qtmux writes
/// each file's first frame's into its timecode track. Existing timecode is
/// kept.
pub(crate) fn stamp_time_of_day(
    pad: &gst::Pad,
    to_utc_us: impl Fn(Option<&gst::FormattedSegment<gst::ClockTime>>, gst::ClockTime) -> Option<i64>
        + Send
        + Sync
        + 'static,
) {
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(pts) = info.buffer().and_then(|b| b.pts()) else {
            return gst::PadProbeReturn::Ok;
        };
        if info
            .buffer()
            .is_some_and(|b| b.meta::<gst_video::VideoTimeCodeMeta>().is_some())
        {
            return gst::PadProbeReturn::Ok;
        }
        let segment = time_segment(pad);
        let tc = framerate(pad)
            .zip(to_utc_us(segment.as_ref(), pts))
            .and_then(|(fps, utc_us)| time_of_day(utc_us, fps));
        if let (Some(tc), Some(buffer)) = (tc, info.buffer_mut()) {
            gst_video::VideoTimeCodeMeta::add(buffer.make_mut(), &tc);
        }
        gst::PadProbeReturn::Ok
    });
}

/// The timecode at `utc_us` (µs since the Unix epoch) in local time, at `fps`.
pub(crate) fn time_of_day(
    utc_us: i64,
    fps: gst::Fraction,
) -> Option<gst_video::ValidVideoTimeCode> {
    let dt = glib::DateTime::from_unix_local(utc_us.div_euclid(1_000_000))
        .ok()?
        .add(glib::TimeSpan::from_microseconds(
            utc_us.rem_euclid(1_000_000),
        ))
        .ok()?;
    gst_video::VideoTimeCode::from_date_time(fps, &dt, drop_frame_flags(fps), 0)
        .ok()?
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Timecode counts frames of the time of day; a second is a second.
    #[test]
    fn time_of_day_counts_frames_of_the_second() {
        gst::init().unwrap();
        let fps = gst::Fraction::new(25, 1);
        let t = 1_790_000_000_000_000; // a whole second
        let a = time_of_day(t, fps).unwrap();
        let b = time_of_day(t + 1_120_000, fps).unwrap();
        assert_eq!(a.frames(), 0);
        assert_eq!(b.frames(), 3, "0.12 s is 3 frames at 25 fps");
        assert_eq!((b.seconds() + 60 - a.seconds()) % 60, 1);
        assert!(!a
            .flags()
            .contains(gst_video::VideoTimeCodeFlags::DROP_FRAME));
    }

    #[test]
    fn drop_frame_only_at_ntsc_rates() {
        let df = |n, d| {
            drop_frame_flags(gst::Fraction::new(n, d))
                .contains(gst_video::VideoTimeCodeFlags::DROP_FRAME)
        };
        assert!(df(30000, 1001));
        assert!(df(60000, 1001));
        assert!(!df(24000, 1001));
        assert!(!df(30, 1));
        assert!(!df(25, 1));
    }
}
