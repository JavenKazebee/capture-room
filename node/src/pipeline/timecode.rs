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

/// The timecode of the frame showing `ns` after the frame stamped `tc`
/// (before it, if negative): whole frames at `tc`'s rate, so a moment between
/// two frames belongs to the earlier one. Timestamps are rounded down to the
/// nanosecond, so one a hair before a frame's exact time counts as that frame.
pub(crate) fn timecode_after(
    tc: &gst_video::ValidVideoTimeCode,
    ns: i64,
) -> Option<gst_video::ValidVideoTimeCode> {
    let fps = tc.fps();
    let (num, den) = (i128::from(fps.numer()), i128::from(fps.denom()));
    if num <= 0 || den <= 0 {
        return None;
    }
    const SLACK_NS: i128 = 1_000;
    let frames = ((i128::from(ns) + SLACK_NS) * num).div_euclid(den * 1_000_000_000);
    let mut tc = tc.clone();
    tc.add_frames(i64::try_from(frames).ok()?);
    Some(tc)
}

/// `fps` as `num/den`, e.g. `30000/1001`.
pub(crate) fn fps_text(fps: gst::Fraction) -> String {
    format!("{}/{}", fps.numer(), fps.denom())
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

    fn tc(fps: gst::Fraction, h: u32, m: u32, s: u32, f: u32) -> gst_video::ValidVideoTimeCode {
        gst_video::ValidVideoTimeCode::new(fps, None, drop_frame_flags(fps), h, m, s, f, 0).unwrap()
    }

    #[test]
    fn timecode_after_counts_whole_frames() {
        gst::init().unwrap();
        let fps = gst::Fraction::new(25, 1);
        let at = tc(fps, 10, 0, 0, 0);
        let frame = 40_000_000; // 1/25 s
        assert_eq!(timecode_after(&at, 0).unwrap().to_string(), "10:00:00:00");
        assert_eq!(
            timecode_after(&at, frame - 2_000).unwrap().to_string(),
            "10:00:00:00"
        );
        assert_eq!(
            timecode_after(&at, frame - 1).unwrap().to_string(),
            "10:00:00:01"
        );
        assert_eq!(
            timecode_after(&at, 3 * frame).unwrap().to_string(),
            "10:00:00:03"
        );
        assert_eq!(
            timecode_after(&at, 26 * frame).unwrap().to_string(),
            "10:00:01:01"
        );
        // A moment just before the frame belongs to the one before it.
        assert_eq!(
            timecode_after(&at, -2_000).unwrap().to_string(),
            "09:59:59:24"
        );
    }

    /// 29.97 drop-frame skips frames 0 and 1 at each minute but every tenth.
    #[test]
    fn timecode_after_drops_frames_at_the_minute() {
        gst::init().unwrap();
        let fps = gst::Fraction::new(30000, 1001);
        let at = tc(fps, 10, 0, 59, 29);
        let frame = 1_001_000_000_000 / 30_000 + 1;
        assert_eq!(
            timecode_after(&at, frame).unwrap().to_string(),
            "10:01:00;02"
        );
        let at = tc(fps, 10, 9, 59, 29);
        assert_eq!(
            timecode_after(&at, frame).unwrap().to_string(),
            "10:10:00;00"
        );
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
