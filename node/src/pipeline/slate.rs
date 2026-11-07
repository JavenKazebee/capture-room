//! Slates: a still card (a title, then take, date and source) shown at the
//! start of a take or before a playlist item. Deliberately one fixed layout,
//! centred text on a plain background in two colours: not a graphics system.
//!
//! [`render`] draws a card once, in whatever raw format its consumer needs,
//! in a throwaway pipeline:
//!
//! ```text
//! videotestsrc(solid colour, 1 frame) → AYUV → textoverlay → videoconvert → caps → appsink
//! ```
//!
//! A recording leg swaps it in for the frames of its first seconds
//! ([`cover_start`]), inside its own pipeline: the picture is covered, not
//! delayed, so timecode and synchronized starts are as without one, and the
//! sound runs under it. A channel pushes it into its video lane once, ahead
//! of the clip.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;
use tracing::warn;

use super::make_el;
use super::timecode::running_time;
use crate::api::types::{SlateLength, SlateSettings};

/// How long a render may take: a card is a single frame, but the first
/// textoverlay loads its fonts.
const RENDER_TIMEOUT: Duration = Duration::from_secs(10);

/// What a card says and how it looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub title: String,
    /// Under the title, smaller; blank ones are left out.
    pub lines: Vec<String>,
    /// ARGB.
    pub background: u32,
    pub text: u32,
}

impl Card {
    /// The default look, for playout.
    pub fn plain(title: String, lines: Vec<String>) -> Self {
        Self {
            title,
            lines,
            background: 0xff1a1a1a,
            text: 0xffffffff,
        }
    }

    /// Pango markup for textoverlay: the title larger and bold.
    pub fn markup(&self) -> String {
        let mut markup = format!(
            "<span size=\"200%\" weight=\"bold\">{}</span>",
            glib::markup_escape_text(&self.title)
        );
        for line in self.lines.iter().filter(|l| !l.trim().is_empty()) {
            markup.push('\n');
            markup.push_str(&glib::markup_escape_text(line));
        }
        markup
    }
}

/// A recording leg's slate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegSlate {
    pub card: Card,
    pub length: SlateLength,
}

/// How long `length` shows; `None` for a single frame.
pub fn duration(length: SlateLength) -> Option<gst::ClockTime> {
    match length {
        SlateLength::Frame => None,
        SlateLength::Seconds3 => Some(gst::ClockTime::from_seconds(3)),
        SlateLength::Seconds5 => Some(gst::ClockTime::from_seconds(5)),
        SlateLength::Seconds10 => Some(gst::ClockTime::from_seconds(10)),
    }
}

/// `#rrggbb` as opaque ARGB.
pub fn parse_colour(colour: &str) -> Option<u32> {
    let hex = colour.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16)
        .ok()
        .map(|rgb| 0xff00_0000 | rgb)
}

/// Settings a preset can save: known title tokens and valid colours.
pub fn check(settings: &SlateSettings) -> Result<(), String> {
    if let Some(token) = super::profile::unknown_title_token(&settings.title) {
        return Err(format!("{token} isn't a slate title token"));
    }
    for (what, colour) in [
        ("background", &settings.background),
        ("text", &settings.text),
    ] {
        if parse_colour(colour).is_none() {
            return Err(format!("the slate's {what} colour must be #rrggbb"));
        }
    }
    Ok(())
}

/// Whether slates can be drawn here: textoverlay is GStreamer's pango plugin.
pub fn available() -> bool {
    gst::ElementFactory::find("textoverlay").is_some()
}

/// Draw `card` as one frame in `caps` (raw video, system memory; any
/// interlacing is left to the buffer flags of the frames it stands in for).
/// Blocks until drawn.
pub fn render(card: &Card, caps: &gst::Caps) -> Result<gst::Buffer> {
    let target = render_caps(caps)?;
    let s = target.structure(0).context("empty caps")?;
    let width = s.get::<i32>("width").context("caps without a width")?;
    let height = s.get::<i32>("height").context("caps without a height")?;
    let par = s
        .get::<gst::Fraction>("pixel-aspect-ratio")
        .unwrap_or(gst::Fraction::new(1, 1));

    let pipeline = gst::Pipeline::with_name("slate-render");
    let src = gst::ElementFactory::make("videotestsrc")
        .property("num-buffers", 1)
        .property("foreground-color", card.background)
        .build()
        .context("create videotestsrc")?;
    super::set_property(&src, "pattern", "solid-color")?;
    let canvas = super::capsfilter(
        "slate-canvas",
        gst::Caps::builder("video/x-raw")
            .field("format", "AYUV")
            .field("width", width)
            .field("height", height)
            .field("pixel-aspect-ratio", par)
            .build(),
    )?;
    let overlay = gst::ElementFactory::make("textoverlay")
        .property("text", card.markup())
        // Scaled with the frame from this size, at 640 wide.
        .property("font-desc", "Sans 16")
        .property("color", card.text)
        .property("draw-shadow", false)
        .property("draw-outline", false)
        .build()
        .map_err(|_| anyhow!("slates need GStreamer's pango plugin (textoverlay)"))?;
    for (name, value) in [
        ("halignment", "center"),
        ("valignment", "center"),
        ("line-alignment", "center"),
    ] {
        super::set_property(&overlay, name, value)?;
    }
    let convert = make_el("videoconvert", "slate-convert")?;
    let out = super::capsfilter("slate-out", target.clone())?;
    let sink = gst_app::AppSink::builder()
        .sync(false)
        .max_buffers(1)
        .build();
    let elements = [&src, &canvas, &overlay, &convert, &out, sink.upcast_ref()];
    pipeline.add_many(elements).context("add slate elements")?;
    gst::Element::link_many(elements).context("link slate elements")?;

    let result = (|| {
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| anyhow!("can't draw the slate"))?;
        let timeout = gst::ClockTime::from_nseconds(RENDER_TIMEOUT.as_nanos() as u64);
        match sink.try_pull_sample(timeout) {
            Some(sample) => sample.buffer_owned().context("slate without a frame"),
            None => {
                let bus = pipeline.bus().context("slate pipeline has no bus")?;
                let reason = bus
                    .pop_filtered(&[gst::MessageType::Error])
                    .and_then(|m| match m.view() {
                        gst::MessageView::Error(e) => Some(e.error().to_string()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "timed out".into());
                bail!("can't draw the slate: {reason}")
            }
        }
    })();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// `caps` as a progressive frame to draw: videotestsrc makes no fields, and
/// an interleaved frame has the same layout.
fn render_caps(caps: &gst::Caps) -> Result<gst::Caps> {
    let features = caps.features(0);
    if features.is_some_and(|f| {
        !f.is_equal(gst::CapsFeatures::new_empty().as_ref()) && !f.contains("memory:SystemMemory")
    }) {
        bail!("can't draw a slate into {caps}");
    }
    let mut s = caps
        .structure(0)
        .filter(|s| s.name() == "video/x-raw")
        .with_context(|| format!("can't draw a slate into {caps}"))?
        .to_owned();
    for field in ["interlace-mode", "field-order", "framerate"] {
        s.remove_field(field);
    }
    Ok(gst::Caps::builder_full().structure(s).build())
}

/// Swap the frames leaving `pad` for `slate` until it has run its length
/// from the first, then get out of the way. Drawn on the first frame, in its
/// caps; if it can't be, the take records without one.
pub fn cover_start(pad: &gst::Pad, slate: LegSlate) {
    struct State {
        until: Option<gst::ClockTime>,
        drawn: Option<(gst::Caps, gst::Buffer)>,
    }
    let state = Mutex::new(State {
        until: None,
        drawn: None,
    });
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(buffer) = info.buffer() else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(pts) = buffer.pts() else {
            return gst::PadProbeReturn::Ok;
        };
        let rt = running_time(pad, pts).unwrap_or(pts);
        let mut state = state.lock().unwrap();
        // A single frame covers just the first.
        let until = *state.until.get_or_insert_with(|| {
            rt + duration(slate.length).unwrap_or(gst::ClockTime::from_nseconds(1))
        });
        if rt >= until {
            return gst::PadProbeReturn::Remove;
        }
        let Some(caps) = pad.current_caps() else {
            return gst::PadProbeReturn::Ok;
        };
        if state.drawn.as_ref().is_none_or(|(c, _)| *c != caps) {
            match render(&slate.card, &caps) {
                Ok(frame) => state.drawn = Some((caps, frame)),
                Err(e) => {
                    warn!(error = %e, "recording without its slate");
                    return gst::PadProbeReturn::Remove;
                }
            }
        }
        let (_, frame) = state.drawn.as_ref().expect("drawn above");
        let mut covered = frame.copy();
        {
            let covered = covered.make_mut();
            covered.set_pts(buffer.pts());
            covered.set_dts(buffer.dts());
            covered.set_duration(buffer.duration());
            covered.set_offset(buffer.offset());
            covered.set_offset_end(buffer.offset_end());
            covered.set_flags(buffer.flags());
        }
        info.data = Some(gst::PadProbeData::Buffer(covered));
        gst::PadProbeReturn::Ok
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Card {
        Card::plain(
            "Service <AM> & more".into(),
            vec!["Take 02".into(), String::new(), "Cam 1".into()],
        )
    }

    #[test]
    fn markup_escapes_and_skips_blank_lines() {
        assert_eq!(
            card().markup(),
            "<span size=\"200%\" weight=\"bold\">Service &lt;AM&gt; &amp; more</span>\nTake 02\nCam 1"
        );
    }

    #[test]
    fn colours_are_rrggbb() {
        assert_eq!(parse_colour("#1A1a1a"), Some(0xff1a1a1a));
        for bad in ["1a1a1a", "#1a1a1", "#1a1a1g", "#1a1a1a1a", ""] {
            assert_eq!(parse_colour(bad), None, "{bad}");
        }
    }

    #[test]
    fn checks_title_tokens_and_colours() {
        let mut settings = SlateSettings {
            length: SlateLength::Seconds5,
            title: "{preset} {source_name} take {take}".into(),
            background: "#000000".into(),
            text: "#ffffff".into(),
        };
        assert_eq!(check(&settings), Ok(()));
        settings.title = "{source_name}.{ext}".into();
        assert_eq!(
            check(&settings),
            Err("{ext} isn't a slate title token".into())
        );
        settings.title = String::new();
        settings.text = "white".into();
        assert!(check(&settings).is_err());
    }

    /// A frame's bytes for the luma of its top-left and centre.
    fn luma(buffer: &gst::Buffer, caps: &gst::Caps, x: u32, y: u32) -> u8 {
        use gstreamer_video::VideoFrameExt;
        let info = gstreamer_video::VideoInfo::from_caps(caps).unwrap();
        let frame =
            gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).unwrap();
        let stride = frame.plane_stride()[0] as usize;
        frame.plane_data(0).unwrap()[y as usize * stride + x as usize]
    }

    #[test]
    fn renders_in_the_caps_asked_for() {
        gst::init().unwrap();
        if !available() {
            eprintln!("no textoverlay: skipped");
            return;
        }
        for caps in [
            "video/x-raw,format=I420,width=1280,height=720,framerate=25/1",
            "video/x-raw,format=NV12,width=1920,height=1080,interlace-mode=interleaved,framerate=30000/1001",
            "video/x-raw,format=I420_10LE,width=640,height=360,framerate=50/1",
        ] {
            let caps: gst::Caps = caps.parse().unwrap();
            let frame = render(&card(), &caps).unwrap();
            let info = gstreamer_video::VideoInfo::from_caps(&render_caps(&caps).unwrap()).unwrap();
            assert_eq!(frame.size(), info.size(), "{caps}");
        }
        // Dark background, white text in the middle.
        let caps: gst::Caps = "video/x-raw,format=I420,width=640,height=360"
            .parse()
            .unwrap();
        let card = Card::plain("█████████".into(), vec![]);
        let frame = render(&card, &caps).unwrap();
        assert!(luma(&frame, &caps, 2, 2) < 40);
        assert!(luma(&frame, &caps, 320, 180) > 200);
    }

    /// White frames at 30 fps through a slated convert: which came out
    /// dark (the slate) and which white.
    fn covered(length: SlateLength, frames: i32) -> Vec<bool> {
        gst::init().unwrap();
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", frames)
            .build()
            .unwrap();
        super::super::set_property(&src, "pattern", "white").unwrap();
        let caps = super::super::capsfilter(
            "caps",
            "video/x-raw,format=I420,width=320,height=180,framerate=30/1,interlace-mode=interleaved"
                .parse()
                .unwrap(),
        )
        .unwrap();
        let convert = make_el("videoconvert", "vconv").unwrap();
        let sink = gst_app::AppSink::builder().sync(false).build();
        let elements = [&src, &caps, &convert, sink.upcast_ref()];
        pipeline.add_many(elements).unwrap();
        gst::Element::link_many(elements).unwrap();
        cover_start(
            &convert.static_pad("sink").unwrap(),
            LegSlate {
                card: card(),
                length,
            },
        );
        pipeline.set_state(gst::State::Playing).unwrap();
        let mut dark = Vec::new();
        while let Ok(sample) = sink.pull_sample() {
            let caps = sample.caps().unwrap().to_owned();
            let buffer = sample.buffer().unwrap();
            dark.push(luma(&buffer.to_owned(), &caps, 2, 2) < 40);
        }
        pipeline.set_state(gst::State::Null).unwrap();
        dark
    }

    #[test]
    fn covers_the_first_seconds_then_gets_out_of_the_way() {
        gst::init().unwrap();
        if !available() {
            eprintln!("no textoverlay: skipped");
            return;
        }
        let dark = covered(SlateLength::Seconds3, 120);
        assert_eq!(dark.len(), 120);
        assert!(dark[..90].iter().all(|&d| d), "{dark:?}");
        assert!(dark[90..].iter().all(|&d| !d), "{dark:?}");

        let dark = covered(SlateLength::Frame, 10);
        assert_eq!(
            dark,
            [true, false, false, false, false, false, false, false, false, false]
        );
    }

    #[test]
    fn refuses_caps_it_cant_draw_into() {
        gst::init().unwrap();
        let caps: gst::Caps = "audio/x-raw,format=S16LE".parse().unwrap();
        assert!(render(&card(), &caps).is_err());
    }
}
