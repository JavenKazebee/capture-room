//! Replay buffers. A replay-enabled source records continuously, like a
//! recording leg fed by its monitor, into one-second segments of an
//! all-intra codec; the oldest are deleted past the buffer's length.
//!
//! ```text
//! appsrc(video) → videoconvert → [caps] → venc → [caps | parse] ─┐
//!                                                      splitmuxsink (qtmux, 1 s)
//! appsrc(audio) → queue → audioconvert → audioresample → S24LE ──┘
//! ```
//!
//! The [`Index`] knows each finished segment's span on the monitor's clock,
//! from splitmuxsink's fragment messages. It outlives the leg: a monitor
//! restart starts a new leg on the same index, leaving a gap. A channel
//! plays part of it through `splitmuxsrc`, given the finished files that
//! cover it (see [`Index::clip`]); a [`Hold`] keeps them from being deleted
//! meanwhile. Design and the prototype's findings in ARCHITECTURE.md
//! (Instant Replay).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::recording::set_property;
use super::{capsfilter, make_el};
use crate::api::types::ReplayCodec;

/// How long each segment runs. Replay reaches to within this of live: the
/// segment being written can't be read.
pub const SEGMENT: gst::ClockTime = gst::ClockTime::SECOND;

/// Neighbouring segments further apart than this have a gap between them.
const GAP: gst::ClockTime = gst::ClockTime::from_mseconds(100);

impl ReplayCodec {
    /// Encoder elements to try, in order.
    pub fn encoders(self) -> &'static [&'static str] {
        match self {
            Self::ProresProxy => &["vtenc_prores"],
            Self::H264Intra => &["vtenc_h264", "x264enc"],
            Self::Mjpeg => &["jpegenc"],
        }
    }

    /// The elements that decode it for playback. Software: VideoToolbox's
    /// decoder drops a frame now and then playing backwards.
    pub fn decoder(self) -> &'static [&'static str] {
        match self {
            Self::ProresProxy => &["avdec_prores"],
            Self::H264Intra => &["h264parse", "avdec_h264"],
            Self::Mjpeg => &["jpegdec"],
        }
    }

    /// Whether this node can record it and play it back.
    pub fn available(self) -> bool {
        let has = |e: &&str| gst::ElementFactory::find(e).is_some();
        self.encoders().iter().any(has) && self.decoder().iter().all(has)
    }

    /// The codecs this node can use for replay, the default first: ProRes
    /// where VideoToolbox has it, else Motion JPEG.
    pub fn usable() -> Vec<Self> {
        [Self::ProresProxy, Self::Mjpeg, Self::H264Intra]
            .into_iter()
            .filter(|c| c.available())
            .collect()
    }
}

// ── Index ─────────────────────────────────────────────────────────────────────

/// A finished segment: its file and the span of its frames on the clock.
#[derive(Debug, Clone)]
pub struct Segment {
    pub path: PathBuf,
    pub start: gst::ClockTime,
    pub end: gst::ClockTime,
    bytes: u64,
}

/// What a replay buffer holds. Shared between its leg's streaming thread,
/// the manager and the clips holding parts of it.
pub type ReplayIndex = Arc<Mutex<Index>>;

pub struct Index {
    dir: PathBuf,
    /// What the segments are in, and the clock their times are on: a
    /// different one on a new leg clears them.
    codec: Option<ReplayCodec>,
    clock: Option<gst::Clock>,
    segments: VecDeque<Segment>,
    /// The file being written, and when its first frame was due.
    open: Option<(PathBuf, gst::ClockTime)>,
    /// Numbers the files, across legs.
    next_number: u64,
    /// How long a span it keeps.
    keep: gst::ClockTime,
    /// Clips playing from it, with the earliest time each needs.
    holds: Vec<(u64, gst::ClockTime)>,
    next_hold: u64,
}

impl Index {
    /// A new, empty index over `dir`, whose old contents are deleted: what a
    /// previous run left has no index.
    pub fn create(dir: &Path, keep: gst::ClockTime) -> ReplayIndex {
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                warn!(dir = ?dir, error = %e, "can't clear old replay segments");
            }
        }
        Arc::new(Mutex::new(Self {
            dir: dir.to_path_buf(),
            codec: None,
            clock: None,
            segments: VecDeque::new(),
            open: None,
            next_number: 0,
            keep,
            holds: Vec::new(),
            next_hold: 0,
        }))
    }

    pub fn set_keep(&mut self, keep: gst::ClockTime) {
        self.keep = keep;
        self.trim();
    }

    /// The oldest and newest frame times that can be played.
    pub fn span(&self) -> Option<(gst::ClockTime, gst::ClockTime)> {
        Some((self.segments.front()?.start, self.segments.back()?.end))
    }

    /// Where each stretch after a gap starts.
    pub fn breaks(&self) -> Vec<gst::ClockTime> {
        self.segments
            .iter()
            .zip(self.segments.iter().skip(1))
            .filter(|(a, b)| b.start > a.end + GAP)
            .map(|(_, b)| b.start)
            .collect()
    }

    pub fn bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.bytes).sum()
    }

    /// The clock the segments' times are on.
    pub fn clock(&self) -> Option<&gst::Clock> {
        self.clock.as_ref()
    }

    /// Delete everything, for a new codec or clock: only while no leg
    /// records into it. The whole folder, since a leg stopped just after
    /// naming a file leaves one the index never heard of.
    fn clear(&mut self) {
        self.segments.clear();
        self.open = None;
        self.codec = None;
        self.clock = None;
        if self.dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.dir) {
                warn!(dir = ?self.dir, error = %e, "can't clear replay segments");
            }
        }
    }

    /// Delete the oldest segments past the length kept, except what a clip
    /// is holding (and everything after it).
    fn trim(&mut self) {
        let Some(newest) = self.segments.back().map(|s| s.end) else {
            return;
        };
        let floor = newest.saturating_sub(self.keep);
        let held = self.holds.iter().map(|(_, t)| *t).min();
        while let Some(oldest) = self.segments.front() {
            if oldest.end > floor || held.is_some_and(|h| oldest.end > h) {
                break;
            }
            let oldest = self.segments.pop_front().unwrap();
            remove(&oldest.path);
        }
    }

    fn opened(&mut self, path: PathBuf, start: gst::ClockTime) {
        // One left open by a leg that stopped is incomplete.
        if let Some((stale, _)) = self.open.replace((path, start)) {
            remove(&stale);
        }
    }

    fn closed(&mut self, path: &Path, end: gst::ClockTime) {
        let Some((open, start)) = self.open.take() else {
            return;
        };
        if open != path || end <= start {
            remove(path);
            return;
        }
        let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        self.segments.push_back(Segment {
            path: open,
            start,
            end,
            bytes,
        });
        self.trim();
    }

    /// The finished files covering `from` → `to` (clamped into the span),
    /// all from one stretch without gaps: the one `from` falls in, else the
    /// next one. Positions in a clip count from its first file's start.
    pub fn clip(
        index: &ReplayIndex,
        from: gst::ClockTime,
        to: Option<gst::ClockTime>,
    ) -> Result<ReplaySpan> {
        let mut this = index.lock().unwrap();
        let codec = this.codec.context("the replay buffer is empty")?;
        let segments = &this.segments;
        // The stretch: segments [first, last].
        let mut first = segments
            .iter()
            .position(|s| s.end > from)
            .context("the replay buffer has nothing that recent yet")?;
        while first > 0 && segments[first].start <= segments[first - 1].end + GAP {
            first -= 1;
        }
        let mut last = first;
        while last + 1 < segments.len() && segments[last + 1].start <= segments[last].end + GAP {
            last += 1;
        }
        let start = segments[first].start;
        let end = segments[last].end;
        let from = from.clamp(start, end);
        let to = to.unwrap_or(end).clamp(from, end);
        if to <= from {
            return Err(anyhow!("nothing to replay between those times"));
        }
        // Only the files the clip needs: from the one holding `from`.
        let begin = (first..=last)
            .find(|&i| segments[i].end > from)
            .unwrap_or(first);
        let stop = (begin..=last)
            .find(|&i| segments[i].end >= to)
            .unwrap_or(last);
        let base = segments[begin].start;
        let files = segments
            .range(begin..=stop)
            .map(|s| s.path.to_string_lossy().into_owned())
            .collect();
        let id = this.next_hold;
        this.next_hold += 1;
        this.holds.push((id, base));
        Ok(ReplaySpan {
            files,
            codec,
            base,
            from: from - base,
            to: to - base,
            hold: Arc::new(Hold {
                index: Arc::downgrade(index),
                id,
            }),
        })
    }
}

/// Part of a replay buffer to play: its files, and where the clip starts
/// and ends in them.
#[derive(Debug, Clone)]
pub struct ReplaySpan {
    pub files: Vec<String>,
    pub codec: ReplayCodec,
    /// The clock time position 0 shows.
    pub base: gst::ClockTime,
    pub from: gst::ClockTime,
    pub to: gst::ClockTime,
    /// Never read: kept for as long as the clip lives.
    #[allow(dead_code)]
    pub hold: Arc<Hold>,
}

/// Keeps a clip's segments on disk while it lives.
#[derive(Debug)]
pub struct Hold {
    index: Weak<Mutex<Index>>,
    id: u64,
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(index) = self.index.upgrade() {
            let mut index = index.lock().unwrap();
            index.holds.retain(|(id, _)| *id != self.id);
            index.trim();
        }
    }
}

impl Drop for Index {
    /// A clip still holding segments only has a weak reference: the files
    /// it has open stay readable until it closes them.
    fn drop(&mut self) {
        self.clear();
    }
}

fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(path = ?path, error = %e, "can't delete replay segment");
        }
    }
}

// ── Leg ───────────────────────────────────────────────────────────────────────

/// A replay buffer recording from a monitor.
pub struct ReplayLeg {
    pipeline: gst::Pipeline,
    /// Connections to the monitor's producers; dropping one stops the feed.
    links: Vec<ConsumptionLink>,
    encoder: &'static str,
    error: Arc<Mutex<Option<String>>>,
}

impl ReplayLeg {
    /// Record `monitor` into `index` with the first of `codec`'s encoders
    /// that works here.
    pub fn start(
        monitor: &MonitorPipeline,
        codec: ReplayCodec,
        index: &ReplayIndex,
        name: &str,
    ) -> Result<Self> {
        let (Some(clock), Some(base)) = monitor.timing() else {
            return Err(anyhow!("the source isn't running"));
        };
        {
            let mut i = index.lock().unwrap();
            if i.codec != Some(codec) || i.clock.as_ref() != Some(&clock) {
                i.clear();
                i.codec = Some(codec);
                i.clock = Some(clock.clone());
            }
            std::fs::create_dir_all(&i.dir)
                .with_context(|| format!("can't create {}", i.dir.display()))?;
        }
        let mut last_err = None;
        for &encoder in codec.encoders() {
            match Self::build(codec, encoder, index, name, &clock, base) {
                Ok(mut leg) => {
                    leg.links
                        .push(monitor.video.add_consumer(&leg.video_src()?)?);
                    leg.links
                        .push(monitor.audio.add_consumer(&leg.audio_src()?)?);
                    info!(name, encoder, "replay buffer recording");
                    return Ok(leg);
                }
                Err(e) => {
                    warn!(encoder, error = %format!("{e:#}"), "replay encoder unavailable");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("no encoder for {codec:?}")))
    }

    fn build(
        codec: ReplayCodec,
        encoder: &'static str,
        index: &ReplayIndex,
        name: &str,
        clock: &gst::Clock,
        base: gst::ClockTime,
    ) -> Result<Self> {
        let pipeline = gst::Pipeline::with_name(&format!("replay-{name}"));
        let error = Arc::new(Mutex::new(None));

        let video_src = gst_app::AppSrc::builder().name("video-src").build();
        let audio_src = gst_app::AppSrc::builder().name("audio-src").build();
        StreamProducer::configure_consumer(&video_src);
        StreamProducer::configure_consumer(&audio_src);

        let mut video = vec![
            video_src.upcast::<gst::Element>(),
            make_el("videoconvert", "vconv")?,
        ];
        let venc = make_el(encoder, "venc")?;
        let set = |k: &str, v: &str| set_property(&venc, k, v);
        match encoder {
            "vtenc_prores" => {
                // Every ProRes frame is a keyframe, but vtenc_prores marks
                // them deltas: splitmuxsink would never split.
                venc.static_pad("src")
                    .context("encoder src pad")?
                    .add_probe(gst::PadProbeType::BUFFER, |_, info| {
                        if let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() {
                            buffer.make_mut().unset_flags(gst::BufferFlags::DELTA_UNIT);
                        }
                        gst::PadProbeReturn::Ok
                    });
            }
            "vtenc_h264" => {
                set("realtime", "true")?;
                set("allow-frame-reordering", "false")?;
                set("max-keyframe-interval", "1")?;
                set("quality", "0.85")?;
            }
            "x264enc" => {
                set("key-int-max", "1")?;
                set("speed-preset", "superfast")?;
                set("pass", "qual")?;
                set("quantizer", "18")?;
            }
            "jpegenc" => {
                video.push(capsfilter(
                    "venc-caps",
                    gst::Caps::builder("video/x-raw")
                        .field("format", "I420")
                        .build(),
                )?);
                set("quality", "85")?;
            }
            _ => {}
        }
        video.push(venc);
        match codec {
            ReplayCodec::ProresProxy => video.push(capsfilter(
                "venc-out-caps",
                gst::Caps::builder("video/x-prores")
                    .field("variant", "proxy")
                    .build(),
            )?),
            ReplayCodec::H264Intra => video.push(make_el("h264parse", "vparse")?),
            ReplayCodec::Mjpeg => {}
        }

        let aq = gst::ElementFactory::make("queue")
            .name("aq")
            .property("max-size-time", gst::ClockTime::from_seconds(10).nseconds())
            .property("max-size-bytes", 0u32)
            .property("max-size-buffers", 0u32)
            .build()
            .context("create audio queue")?;
        let audio = vec![
            audio_src.upcast::<gst::Element>(),
            aq,
            make_el("audioconvert", "aconv")?,
            make_el("audioresample", "aresample")?,
            capsfilter(
                "aout-caps",
                gst::Caps::builder("audio/x-raw")
                    .field("format", "S24LE")
                    .build(),
            )?,
        ];

        let muxer = make_el("qtmux", "mux")?;
        let sink = gst::ElementFactory::make("splitmuxsink")
            .name("sink")
            .property("muxer", &muxer)
            .property("max-size-time", SEGMENT.nseconds())
            // Every frame is a keyframe.
            .property("send-keyframe-requests", false)
            .build()
            .context("create splitmuxsink")?;
        let numbering = Arc::downgrade(index);
        sink.connect("format-location", false, move |_| {
            let index = numbering.upgrade()?;
            let mut index = index.lock().unwrap();
            let file = index.dir.join(format!("{:08}.mov", index.next_number));
            index.next_number += 1;
            Some(file.to_string_lossy().into_owned().to_value())
        });

        // Each file's span, from splitmuxsink's messages (running time,
        // shared with the monitor). Errors are kept to show.
        let messages = Arc::downgrade(index);
        let error_ref = Arc::clone(&error);
        let leg_name = name.to_string();
        pipeline
            .bus()
            .context("replay pipeline has no bus")?
            .set_sync_handler(move |_, msg| {
                match msg.view() {
                    gst::MessageView::Element(e) => {
                        let Some(s) = e.structure() else {
                            return gst::BusSyncReply::Drop;
                        };
                        let opened = s.name() == "splitmuxsink-fragment-opened";
                        if !opened && s.name() != "splitmuxsink-fragment-closed" {
                            return gst::BusSyncReply::Drop;
                        }
                        let (Ok(location), Ok(rt), Some(index)) = (
                            s.get::<String>("location"),
                            s.get::<gst::ClockTime>("running-time"),
                            messages.upgrade(),
                        ) else {
                            return gst::BusSyncReply::Drop;
                        };
                        let mut index = index.lock().unwrap();
                        if opened {
                            index.opened(PathBuf::from(location), base + rt);
                        } else {
                            index.closed(Path::new(&location), base + rt);
                        }
                    }
                    gst::MessageView::Error(err) => {
                        let mut error = error_ref.lock().unwrap();
                        if error.is_none() {
                            warn!(name = %leg_name, error = %err.error(), debug = ?err.debug(), "replay buffer failed");
                            *error = Some(err.error().to_string());
                        }
                    }
                    _ => {}
                }
                gst::BusSyncReply::Drop
            });

        pipeline
            .add_many(video.iter().chain(&audio).chain([&sink]))
            .context("add replay elements")?;
        gst::Element::link_many(&video).context("link replay video")?;
        gst::Element::link_many(&audio).context("link replay audio")?;
        for (last, pad) in [(video.last(), "video"), (audio.last(), "audio_%u")] {
            let sink_pad = sink
                .request_pad_simple(pad)
                .with_context(|| format!("splitmuxsink {pad}"))?;
            last.unwrap()
                .static_pad("src")
                .context("src pad")?
                .link(&sink_pad)
                .with_context(|| format!("link {encoder} into .mov"))?;
        }

        // The monitor's clock and base time: forwarded timestamps mean the
        // same running time on both sides.
        pipeline.use_clock(Some(clock));
        pipeline.set_base_time(base);
        pipeline.set_start_time(gst::ClockTime::NONE);
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| anyhow!("{encoder}: replay buffer failed to start"))?;
        Ok(Self {
            pipeline,
            links: Vec::new(),
            encoder,
            error,
        })
    }

    fn video_src(&self) -> Result<gst_app::AppSrc> {
        self.appsrc("video-src")
    }

    fn audio_src(&self) -> Result<gst_app::AppSrc> {
        self.appsrc("audio-src")
    }

    fn appsrc(&self, name: &str) -> Result<gst_app::AppSrc> {
        self.pipeline
            .by_name(name)
            .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
            .with_context(|| format!("no {name}"))
    }

    pub fn encoder(&self) -> &'static str {
        self.encoder
    }

    /// Why it stopped recording, if it has.
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for ReplayLeg {
    /// The segment being written is lost (it has no index yet).
    fn drop(&mut self) {
        self.links.clear();
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{
        AudioTestSignal, MonitorSettingsDto, TestSourceConfig, VideoTestPattern,
    };
    use crate::sources::test::TestSource;
    use std::time::Duration;

    pub(crate) fn test_source() -> TestSource {
        TestSource::new(
            "t".into(),
            "t".into(),
            TestSourceConfig {
                pattern: VideoTestPattern::Smpte,
                width: 320,
                height: 180,
                fps_num: 30,
                fps_den: 1,
                audio_signal: AudioTestSignal::Tone,
                frequency: 440.0,
                channels: 2,
            },
        )
    }

    /// Records into one-second segments laid end to end on the monitor's
    /// clock, keeps only the length asked for, and a hold keeps a clip's
    /// segments past it.
    #[tokio::test(flavor = "multi_thread")]
    async fn buffer_rolls_and_holds() {
        gst::init().unwrap();
        let codec = ReplayCodec::Mjpeg;
        let source = test_source();
        let clock = gst::SystemClock::obtain();
        let monitor =
            MonitorPipeline::new(&source, &MonitorSettingsDto::default(), &clock).unwrap();
        let dir = std::env::temp_dir().join(format!("capture-room-replay-{}", std::process::id()));
        let index = Index::create(&dir, gst::ClockTime::from_seconds(3));
        while monitor.video_frames() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let leg = ReplayLeg::start(&monitor, codec, &index, "test").unwrap();
        assert_eq!(leg.encoder(), "jpegenc");

        let wait_for = |n: usize| {
            let index = index.clone();
            async move {
                for _ in 0..200 {
                    if index.lock().unwrap().segments.len() >= n {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                panic!("fewer than {n} segments");
            }
        };
        wait_for(3).await;
        {
            let i = index.lock().unwrap();
            for (a, b) in i.segments.iter().zip(i.segments.iter().skip(1)) {
                let gap = b.start.nseconds() as i64 - a.end.nseconds() as i64;
                assert!(gap.abs() < GAP.nseconds() as i64, "{a:?} → {b:?}");
                assert!(a.path.exists());
            }
            assert!(i.breaks().is_empty());
        }

        // Hold the oldest second, then let the buffer roll past it.
        let (start, _) = index.lock().unwrap().span().unwrap();
        let span = Index::clip(&index, start, Some(start + SEGMENT)).unwrap();
        assert_eq!(span.from, gst::ClockTime::ZERO);
        let held = PathBuf::from(&span.files[0]);
        wait_for(6).await;
        assert!(held.exists(), "a held segment was deleted");
        drop(span);
        let (oldest, newest) = index.lock().unwrap().span().unwrap();
        assert!(!held.exists(), "a released segment was kept");
        assert!(newest - oldest <= gst::ClockTime::from_seconds(4));

        // A clip from the middle starts in the file holding its first frame.
        let at = newest - gst::ClockTime::from_mseconds(1500);
        let span = Index::clip(&index, at, None).unwrap();
        assert!(span.base <= at && at < span.base + SEGMENT + GAP);
        assert_eq!(span.base + span.from, at);
        assert_eq!(span.base + span.to, newest);

        drop(leg);
        let _ = monitor.stop();
        drop(span);
        drop(index);
        assert!(!dir.exists(), "the buffer's folder was left behind");
    }
}
