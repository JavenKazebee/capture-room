//! Replay buffers: a source records continuously into one-second segments of
//! an all-intra codec, deleting the oldest past the buffer's length.
//!
//! ```text
//! appsrc(video) → videoconvert → [caps] → venc → [caps | parse] ──────────┐
//!                                                                splitmuxsink (qtmux, 1 s)
//! appsrc(audio) → queue → audioconvert → audioresample → S24LE → audiorate ┘
//! ```
//!
//! The [`Index`] tracks each finished segment's span on the monitor's clock
//! from splitmuxsink's fragment messages. It outlives the leg: a monitor
//! restart continues on the same index, leaving a gap. A channel plays a
//! [`Index::clip`] through `splitmuxsrc`; a [`Hold`] keeps its files meanwhile.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::recording::stamp_timecode;
use super::{
    audio_hold_queue, capsfilter, make_el, mark_prores_keyframes, pcm_audiorate, set_property,
    share_clock, start_or_reason, ConsumerPipeline,
};
use crate::api::types::ReplayCodec;

/// Segment length. Replay reaches only this close to live: the open segment
/// can't be read.
pub const SEGMENT: gst::ClockTime = gst::ClockTime::SECOND;

/// Neighbouring segments further apart than this have a gap between them.
const GAP: gst::ClockTime = gst::ClockTime::from_mseconds(100);

impl ReplayCodec {
    pub fn encoders(self) -> &'static [&'static str] {
        match self {
            Self::ProresProxy => &["vtenc_prores", "avenc_prores_ks"],
            Self::H264Intra => &["vtenc_h264", "x264enc"],
            Self::Mjpeg => &["jpegenc"],
        }
    }

    /// Playback decoders. Software: VideoToolbox's decoder drops frames
    /// playing backwards.
    pub fn decoder(self) -> &'static [&'static str] {
        match self {
            Self::ProresProxy => &["avdec_prores"],
            Self::H264Intra => &["h264parse", "avdec_h264"],
            Self::Mjpeg => &["jpegdec"],
        }
    }

    pub fn available(self) -> bool {
        self.missing().is_empty()
    }

    /// Missing decoder elements, plus every encoder when none is present.
    pub fn missing(self) -> Vec<&'static str> {
        let mut list = crate::plugins::missing(self.decoder());
        let encoders = crate::plugins::missing(self.encoders());
        if encoders.len() == self.encoders().len() {
            list.extend(encoders);
        }
        list
    }

    /// The codecs this node can use for replay, the default first: ProRes
    /// where VideoToolbox or FFmpeg has it, else Motion JPEG.
    pub fn usable() -> Vec<Self> {
        [Self::ProresProxy, Self::Mjpeg, Self::H264Intra]
            .into_iter()
            .filter(|c| c.available())
            .collect()
    }
}

/// A finished segment: its file and the span of its frames on the clock.
#[derive(Debug, Clone)]
pub struct Segment {
    pub path: PathBuf,
    pub start: gst::ClockTime,
    pub end: gst::ClockTime,
    bytes: u64,
}

/// Shared by the leg's streaming thread, the manager and clips.
pub type ReplayIndex = Arc<Mutex<Index>>;

pub struct Index {
    dir: PathBuf,
    /// A new leg with a different codec or clock clears the segments.
    codec: Option<ReplayCodec>,
    clock: Option<gst::Clock>,
    segments: VecDeque<Segment>,
    /// The file being written, and when its first frame was due.
    open: Option<(PathBuf, gst::ClockTime)>,
    /// File numbering, across legs.
    next_number: u64,
    /// Bytes of every segment finished, deleted ones included.
    written: u64,
    keep: gst::ClockTime,
    /// Clips playing from it, with the earliest time each needs.
    holds: Vec<(u64, gst::ClockTime)>,
    next_hold: u64,
}

impl Index {
    /// A new index over `dir`. It touches no files: what a previous run left
    /// there goes with [`Index::clear`], or when the first leg starts.
    pub fn create(dir: &Path, keep: gst::ClockTime) -> ReplayIndex {
        Arc::new(Mutex::new(Self {
            dir: dir.to_path_buf(),
            codec: None,
            clock: None,
            segments: VecDeque::new(),
            open: None,
            next_number: 0,
            written: 0,
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

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn clock(&self) -> Option<&gst::Clock> {
        self.clock.as_ref()
    }

    /// Delete everything; only while no leg records. Removes the whole
    /// folder, since a leg stopped just after naming a file leaves an
    /// unindexed one. The folder goes once `index` is unlocked: a large one
    /// takes a while, and the manager reads the index to list buffers.
    pub fn clear(index: &ReplayIndex) {
        let dir = {
            let mut index = index.lock().unwrap();
            index.forget();
            index.dir.clone()
        };
        remove_dir(&dir);
    }

    fn forget(&mut self) {
        self.segments.clear();
        self.open = None;
        self.codec = None;
        self.clock = None;
    }

    /// Delete segments past `keep`, except from the oldest hold on.
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
        self.written += bytes;
        self.segments.push_back(Segment {
            path: open,
            start,
            end,
            bytes,
        });
        self.trim();
    }

    /// The files covering `from` → `to`, within one gapless stretch: the one
    /// `from` falls in, else the next. Positions count from the first file's
    /// start. Without `to`, the span reaches the stretch's newest frame for
    /// now, and [`Hold::newer`] has what's recorded after.
    pub fn clip(
        index: &ReplayIndex,
        from: gst::ClockTime,
        to: Option<gst::ClockTime>,
    ) -> Result<ReplaySpan> {
        let mut this = index.lock().unwrap();
        let codec = this.codec.context("the replay buffer is empty")?;
        let segments = &this.segments;
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
            hold: Some(Arc::new(Hold {
                index: Arc::downgrade(index),
                id,
            })),
        })
    }

    /// The segments finished after the one at `last`, up to a gap: what a
    /// clip plays on into. `None` once `last` has left the index.
    fn after(&self, last: &Path) -> Option<Vec<PathBuf>> {
        let at = self.segments.iter().position(|s| s.path == last)?;
        let newer = self.segments.range(at..);
        Some(
            newer
                .clone()
                .zip(newer.skip(1))
                .take_while(|(a, b)| b.start <= a.end + GAP)
                .map(|(_, b)| b.path.clone())
                .collect(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ReplaySpan {
    pub files: Vec<String>,
    pub codec: ReplayCodec,
    /// The clock time position 0 shows.
    pub base: gst::ClockTime,
    pub from: gst::ClockTime,
    /// The out point asked for, else the newest frame when it was made.
    pub to: gst::ClockTime,
    /// Held for the clip's life. `None` for a saved clip, which owns its
    /// files.
    pub hold: Option<Arc<Hold>>,
}

/// Keeps a clip's segments on disk while it lives, and finds the ones it can
/// play on into.
#[derive(Debug)]
pub struct Hold {
    index: Weak<Mutex<Index>>,
    id: u64,
}

impl Hold {
    /// The files recorded after `last` (the span's own last, or the last of
    /// an earlier answer) without a gap. `None` once the buffer has gone or
    /// started again.
    pub fn newer(&self, last: &str) -> Option<Vec<String>> {
        let index = self.index.upgrade()?;
        let files = index.lock().unwrap().after(Path::new(last))?;
        Some(
            files
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        )
    }
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
    /// Clips only hold a weak reference; files they have open stay readable
    /// until closed.
    fn drop(&mut self) {
        self.forget();
        remove_dir(&self.dir);
    }
}

fn remove_dir(dir: &Path) {
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            warn!(dir = ?dir, error = %e, "can't clear replay segments");
        }
    }
}

fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(path = ?path, error = %e, "can't delete replay segment");
        }
    }
}

pub struct ReplayLeg {
    /// Dropping it loses the segment being written (it has no index yet).
    consumer: ConsumerPipeline,
    encoder: &'static str,
}

impl ReplayLeg {
    /// Record `monitor` into `index` with the first of `codec`'s encoders
    /// that works.
    pub fn start(
        monitor: &MonitorPipeline,
        codec: ReplayCodec,
        index: &ReplayIndex,
        name: &str,
    ) -> Result<Self> {
        let (Some(clock), Some(base)) = monitor.timing() else {
            return Err(anyhow!("the source isn't running"));
        };
        let stale = {
            let i = index.lock().unwrap();
            i.codec != Some(codec) || i.clock.as_ref() != Some(&clock)
        };
        if stale {
            Index::clear(index);
        }
        let dir = {
            let mut i = index.lock().unwrap();
            i.codec = Some(codec);
            i.clock = Some(clock.clone());
            i.dir.clone()
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
        let mut last_err = None;
        for &encoder in codec.encoders() {
            match Self::build(codec, encoder, index, name, &clock, base) {
                Ok(mut leg) => {
                    leg.consumer.connect(monitor)?;
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
        let consumer = ConsumerPipeline::new(&format!("replay-{name}"));
        let ConsumerPipeline {
            pipeline,
            video_src,
            audio_src,
            error,
            ..
        } = &consumer;

        let mut video = vec![
            video_src.clone().upcast::<gst::Element>(),
            make_el("videoconvert", "vconv")?,
        ];
        let venc = make_el(encoder, "venc")?;
        let set = |k: &str, v: &str| set_property(&venc, k, v);
        match encoder {
            "vtenc_prores" => mark_prores_keyframes(&venc)?,
            "avenc_prores_ks" => set("profile", "proxy")?,
            "vtenc_h264" => {
                set("realtime", "true")?;
                set("allow-frame-reordering", "false")?;
                set("max-keyframe-interval", "1")?;
                // Matches ProRes Proxy's VMAF at ~60% of its size. A bitrate
                // is ignored when every frame is a keyframe.
                set("quality", "0.75")?;
            }
            "x264enc" => {
                set("key-int-max", "1")?;
                set("speed-preset", "superfast")?;
                set("pass", "qual")?;
                set("quantizer", "18")?;
                // `qual` caps the rate at `bitrate` (2 Mbit/s by default):
                // lift the cap so the quantizer alone sets the quality.
                set("bitrate", "2048000")?;
            }
            "jpegenc" => {
                // jpegdec reads JPEG as full-range BT.601; limited-range
                // BT.709 input comes back with lifted blacks and shifted
                // colours.
                video.push(capsfilter(
                    "venc-caps",
                    gst::Caps::builder("video/x-raw")
                        .field("format", "I420")
                        .field("colorimetry", "1:4:0:0")
                        .build(),
                )?);
                set("quality", "85")?;
            }
            _ => {}
        }
        video.push(venc);
        match codec {
            // vtenc_prores picks its profile from downstream caps.
            ReplayCodec::ProresProxy if encoder == "vtenc_prores" => video.push(capsfilter(
                "venc-out-caps",
                gst::Caps::builder("video/x-prores")
                    .field("variant", "proxy")
                    .build(),
            )?),
            ReplayCodec::H264Intra => video.push(make_el("h264parse", "vparse")?),
            ReplayCodec::ProresProxy | ReplayCodec::Mjpeg => {}
        }

        let audio = vec![
            audio_src.clone().upcast::<gst::Element>(),
            audio_hold_queue()?,
            make_el("audioconvert", "aconv")?,
            make_el("audioresample", "aresample")?,
            capsfilter(
                "aout-caps",
                gst::Caps::builder("audio/x-raw")
                    .field("format", "S24LE")
                    .build(),
            )?,
            pcm_audiorate()?,
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

        // Each file's span from splitmuxsink's messages, in running time
        // shared with the monitor.
        let messages = Arc::downgrade(index);
        let error_ref = error.clone();
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
                    // Logged once: a later error only follows from the first.
                    gst::MessageView::Error(err) if error_ref.set(err) => {
                        warn!(name = %leg_name, error = %err.error(), debug = ?err.debug(), "replay buffer failed");
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

        share_clock(pipeline, clock, base);
        start_or_reason(pipeline, error, "replay buffer failed to start")
            .map_err(|reason| anyhow!("{encoder}: {reason}"))?;
        Ok(Self { consumer, encoder })
    }

    pub fn encoder(&self) -> &'static str {
        self.encoder
    }

    pub fn dropped_frames(&self) -> u64 {
        self.consumer.dropped_frames()
    }

    pub fn error(&self) -> Option<String> {
        self.consumer.error.get()
    }
}

/// Keep a span's files in `dir` as a saved clip: hard links, so saving is free
/// and the buffer can still delete its own names; copies across volumes.
pub fn save(span: &ReplaySpan, dir: &Path) -> Result<Vec<String>> {
    let saved = (|| {
        std::fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
        let mut files = Vec::with_capacity(span.files.len());
        for (i, file) in span.files.iter().enumerate() {
            let to = dir.join(format!("{i:08}.mov"));
            if std::fs::hard_link(file, &to).is_err() {
                std::fs::copy(file, &to).with_context(|| format!("can't copy {file}"))?;
            }
            files.push(to.to_string_lossy().into_owned());
        }
        Ok(files)
    })();
    if saved.is_err() {
        let _ = std::fs::remove_dir_all(dir);
    }
    saved
}

/// Remux `from` → `to` of `files` (positions as in a [`ReplaySpan`]) into
/// one .mov at `out` without re-encoding; all-intra, so any frame can be the
/// cut. Blocks. With `timecode` (position 0's clock time, the domain's UTC
/// offset) the file gets time-of-day timecode, so a group's angles line up
/// in an NLE.
///
/// ```text
/// splitmuxsrc ─┬→ [gate] → qtmux → filesink
///              └→ [gate] ↗
/// ```
pub fn export(
    files: &[String],
    from: gst::ClockTime,
    to: gst::ClockTime,
    timecode: Option<(gst::ClockTime, i64)>,
    out: &Path,
) -> Result<()> {
    let pipeline = gst::Pipeline::with_name("replay-export");
    let src = make_el("splitmuxsrc", "src")?;
    let list = files.to_vec();
    src.connect("format-location", false, move |_| Some(list.to_value()));
    let mux = make_el("qtmux", "mux")?;
    // Not prerolled: nothing reaches it until the seek has been made.
    let sink = gst::ElementFactory::make("filesink")
        .name("sink")
        .property("location", out.to_string_lossy().as_ref())
        .property("async", false)
        .build()
        .context("create filesink")?;
    pipeline
        .add_many([&src, &mux, &sink])
        .context("add export elements")?;
    mux.link(&sink).context("link qtmux")?;
    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    let mux_weak = mux.downgrade();
    src.connect_pad_added(move |_, pad| {
        let Some(mux) = mux_weak.upgrade() else {
            return;
        };
        let video = pad.name().starts_with("video");
        let template = if video {
            "video_%u"
        } else if pad.name().starts_with("audio") {
            "audio_%u"
        } else {
            return;
        };
        // Ahead of the gate, which drops the part's own segment.
        let started_tx = started_tx.clone();
        pad.add_probe(
            gst::PadProbeType::EVENT_DOWNSTREAM,
            move |_, info| match info.event().map(|e| e.type_()) {
                Some(gst::EventType::Segment) => {
                    let _ = started_tx.send(());
                    gst::PadProbeReturn::Remove
                }
                _ => gst::PadProbeReturn::Ok,
            },
        );
        gate(pad, from);
        match mux.request_pad_simple(template) {
            Some(sink) => {
                // After the gate the segment is the seek's, whose stream
                // time is the position. The cut frame can start just before.
                if let (true, Some((base, offset))) = (video, timecode) {
                    stamp_timecode(&sink, move |segment, pts| {
                        let position = match segment?.to_stream_time_full(pts)? {
                            gst::Signed::Positive(p) => p.useconds() as i64,
                            gst::Signed::Negative(p) => -(p.useconds() as i64),
                        };
                        Some(base.useconds() as i64 + position + offset)
                    });
                }
                if let Err(e) = pad.link(&sink) {
                    warn!(pad = %pad.name(), error = ?e, "can't export replay stream");
                }
            }
            None => warn!(pad = %pad.name(), "qtmux has no pad for replay stream"),
        }
    });

    let result = (|| {
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|_| anyhow!("the clip's files can't be read"))?;
        let (changed, _, _) = pipeline.state(gst::ClockTime::from_seconds(10));
        changed.map_err(|_| anyhow!("the clip's files can't be read"))?;
        // splitmuxsrc (seen in 1.24) reports PAUSED before it starts its
        // first part. A seek that lands while it does can miss a stream,
        // which then plays on under the part's segment: the gate drops all
        // of it and the file has no sound, or no pictures. So the seek waits
        // for every stream's first segment.
        let streams = src
            .src_pads()
            .iter()
            .filter(|p| p.name().starts_with("video") || p.name().starts_with("audio"))
            .count();
        for _ in 0..streams {
            started
                .recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|_| anyhow!("the clip's files can't be read"))?;
        }
        // At the source: the muxer would handle a seek itself.
        src.seek(
            1.0,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            from,
            gst::SeekType::Set,
            to,
        )
        .context("can't seek to the clip's in point")?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| anyhow!("export failed to start"))?;
        // Remuxing runs far faster than real time; this is only a backstop.
        let timeout = gst::ClockTime::from_seconds(60) + (to - from) * 2;
        let bus = pipeline.bus().context("export pipeline has no bus")?;
        let msg = bus
            .timed_pop_filtered(timeout, &[gst::MessageType::Eos, gst::MessageType::Error])
            .context("export timed out")?;
        match msg.view() {
            gst::MessageView::Error(e) => Err(anyhow!("{}", e.error())),
            _ => Ok(()),
        }
    })();
    let _ = pipeline.set_state(gst::State::Null);
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

/// Drop everything on `pad` before the seek's segment to `from`, and all
/// flushes: a flushing seek through a muxer spoils its file. Audio is trimmed
/// to the segment: qtdemux hands out PCM in chunks that straddle the cuts,
/// and qtmux would drop the first whole and write the last whole.
///
/// splitmuxsrc (seen in 1.24) can flush again after the segment and resend
/// the in point's buffers under the same segment. With that flush dropped,
/// the repeats would reach qtmux: extra frames, extra sound, or "Could not
/// multiplex stream". So nothing goes back over what's been let through:
/// pictures at or before the last one are dropped, and sound is trimmed to
/// start where the last buffer ended (which also takes out any overlap
/// where two files meet).
fn gate(pad: &gst::Pad, from: gst::ClockTime) {
    // An accurate seek's segment starts where it was asked to.
    const NEAR: gst::ClockTime = gst::ClockTime::from_mseconds(1);
    // The segment's start and stop, once let through.
    let span = Mutex::new(None::<(gst::ClockTime, Option<gst::ClockTime>)>);
    // The last picture's PTS, or where the last sound ended.
    let reached = Mutex::new(None::<gst::ClockTime>);
    pad.add_probe(gst::PadProbeType::DATA_DOWNSTREAM, move |pad, info| {
        let open = *span.lock().unwrap();
        match &mut info.data {
            Some(gst::PadProbeData::Event(e)) => match e.view() {
                gst::EventView::FlushStart(_) | gst::EventView::FlushStop(_) => {
                    gst::PadProbeReturn::Drop
                }
                gst::EventView::Segment(seg) => {
                    // Stream time: splitmuxsrc's segments start at 1000 s
                    // plus the position.
                    let segment = seg.segment().downcast_ref::<gst::ClockTime>();
                    let at = segment.and_then(|s| s.to_stream_time(s.start()?));
                    if at
                        .is_some_and(|at| at.nseconds().abs_diff(from.nseconds()) < NEAR.nseconds())
                    {
                        *span.lock().unwrap() = segment
                            .and_then(|s| s.start())
                            .map(|start| (start, segment.and_then(|s| s.stop())));
                        gst::PadProbeReturn::Ok
                    } else {
                        gst::PadProbeReturn::Drop
                    }
                }
                _ => gst::PadProbeReturn::Ok,
            },
            Some(gst::PadProbeData::Buffer(_)) | Some(gst::PadProbeData::BufferList(_))
                if open.is_none() =>
            {
                gst::PadProbeReturn::Drop
            }
            Some(gst::PadProbeData::Buffer(buffer)) => {
                let (Some((start, stop)), Some(pts)) = (open, buffer.pts()) else {
                    return gst::PadProbeReturn::Ok;
                };
                let mut reached = reached.lock().unwrap();
                if !pad.name().starts_with("audio") {
                    if reached.is_some_and(|r| pts <= r) {
                        return gst::PadProbeReturn::Drop;
                    }
                    *reached = Some(pts);
                    return gst::PadProbeReturn::Ok;
                }
                let floor = reached.map_or(start, |r| r.max(start));
                let end = buffer.duration().map(|d| pts + d);
                let head = (pts < floor).then(|| floor - pts);
                let tail = match (stop, end) {
                    (Some(stop), Some(end)) if end > stop => Some(end - stop),
                    _ => None,
                };
                if head.is_none() && tail.is_none() {
                    *reached = Some(end.unwrap_or(pts));
                    return gst::PadProbeReturn::Ok;
                }
                match trim_audio(
                    pad,
                    buffer,
                    head.unwrap_or_default(),
                    tail.unwrap_or_default(),
                ) {
                    Some(trimmed) => {
                        let at = trimmed.pts().unwrap_or(pts);
                        *reached = Some(trimmed.duration().map_or(at, |d| at + d));
                        *buffer = trimmed;
                        gst::PadProbeReturn::Ok
                    }
                    None => gst::PadProbeReturn::Drop,
                }
            }
            _ => gst::PadProbeReturn::Ok,
        }
    });
}

/// `buffer` (raw PCM on `pad`) without its first `head` and last `tail` of
/// sound, each rounded up to a whole sample; `None` if nothing's left.
fn trim_audio(
    pad: &gst::Pad,
    buffer: &gst::Buffer,
    head: gst::ClockTime,
    tail: gst::ClockTime,
) -> Option<gst::Buffer> {
    let caps = pad.current_caps()?;
    let s = caps.structure(0)?;
    let rate = u64::try_from(s.get::<i32>("rate").ok()?).ok()?;
    let channels = usize::try_from(s.get::<i32>("channels").ok()?).ok()?;
    let width = match s.get::<&str>("format").ok()? {
        "S16LE" | "S16BE" => 2,
        "S24LE" | "S24BE" => 3,
        "S32LE" | "S32BE" | "F32LE" | "F32BE" => 4,
        "F64LE" | "F64BE" => 8,
        _ => return Some(buffer.clone()),
    };
    let frame = channels * width;
    let samples = |t: gst::ClockTime| {
        t.nseconds()
            .saturating_mul(rate)
            .div_ceil(gst::ClockTime::SECOND.nseconds())
    };
    let (skip, drop) = (samples(head), samples(tail));
    let offset = usize::try_from(skip).ok()? * frame;
    let end = buffer
        .size()
        .checked_sub(usize::try_from(drop).ok()? * frame)?;
    if offset >= end {
        return None;
    }
    let mut trimmed = buffer
        .copy_region(
            gst::BufferCopyFlags::MEMORY | gst::BufferCopyFlags::FLAGS,
            offset..end,
        )
        .ok()?;
    let kept = gst::ClockTime::from_nseconds(
        ((end - offset) / frame) as u64 * gst::ClockTime::SECOND.nseconds() / rate,
    );
    let skipped = gst::ClockTime::from_nseconds(
        skip.saturating_mul(gst::ClockTime::SECOND.nseconds()) / rate,
    );
    let b = trimmed.make_mut();
    b.set_pts(buffer.pts().map(|t| t + skipped));
    b.set_duration(buffer.duration().map(|_| kept));
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{
        AudioTestSignal, MonitorSettingsDto, TestSourceConfig, VideoTestPattern,
    };
    use crate::pipeline::recording::time_code;
    use crate::sources::test::TestSource;
    use gstreamer_app as gst_app;
    use gstreamer_video as gst_video;
    use std::time::Duration;

    pub(crate) fn test_source() -> TestSource {
        test_pattern(VideoTestPattern::Smpte)
    }

    fn test_pattern(pattern: VideoTestPattern) -> TestSource {
        TestSource::new(
            "t".into(),
            "t".into(),
            TestSourceConfig {
                pattern,
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

    /// An index holding segments at these spans (seconds), without files.
    fn index_of(spans: &[(f64, f64)]) -> ReplayIndex {
        let dir = std::env::temp_dir().join(format!("cr-replay-index-{}", uuid::Uuid::new_v4()));
        let index = Index::create(&dir, gst::ClockTime::from_seconds(600));
        let mut i = index.lock().unwrap();
        i.codec = Some(ReplayCodec::Mjpeg);
        for &(start, end) in spans {
            i.record(start, end);
        }
        drop(i);
        index
    }

    fn newer_than(span: &ReplaySpan, last: &str) -> Option<Vec<String>> {
        span.hold.as_ref().unwrap().newer(last)
    }

    fn secs(s: f64) -> gst::ClockTime {
        gst::ClockTime::from_nseconds((s * 1e9) as u64)
    }

    impl Index {
        fn record(&mut self, start: f64, end: f64) {
            let path = self.dir.join(format!("{:08}.mov", self.next_number));
            self.next_number += 1;
            self.opened(path.clone(), secs(start));
            self.closed(&path, secs(end));
        }
    }

    /// Open-ended, a clip runs to the newest frame of its stretch for now,
    /// and plays on into what's recorded after it, up to a gap.
    #[test]
    fn open_clip_plays_on_into_newer_segments() {
        let index = index_of(&[(10.0, 11.0), (11.0, 12.0), (12.0, 13.0)]);
        let span = Index::clip(&index, secs(11.5), None).unwrap();
        assert_eq!(span.files.len(), 2, "from the file holding its first frame");
        assert_eq!(span.base, secs(11.0));
        assert_eq!(span.from, secs(0.5));
        assert_eq!(span.to, secs(2.0), "to the newest frame");
        let last = span.files.last().unwrap().clone();
        assert_eq!(newer_than(&span, &last), Some(vec![]), "nothing newer yet");

        // Two more seconds, then a gap: the source restarted.
        {
            let mut i = index.lock().unwrap();
            i.record(13.0, 14.0);
            i.record(14.0, 15.0);
            i.record(20.0, 21.0);
        }
        let newer = newer_than(&span, &last).unwrap();
        assert_eq!(newer.len(), 2, "up to the gap: {newer:?}");
        assert_eq!(newer_than(&span, newer.last().unwrap()), Some(vec![]));

        // A closed clip stops at its out point's file, but can still grow.
        let closed = Index::clip(&index, secs(10.2), Some(secs(11.5))).unwrap();
        assert_eq!(closed.files.len(), 2);
        assert_eq!(closed.to, secs(1.5));
        assert_eq!(
            newer_than(&closed, closed.files.last().unwrap())
                .unwrap()
                .len(),
            3
        );

        // After a gap, a clip is within the later stretch.
        let later = Index::clip(&index, secs(16.0), None).unwrap();
        assert_eq!(later.base, secs(20.0));
        assert_eq!(later.from, gst::ClockTime::ZERO);
        assert_eq!(later.to, secs(1.0));

        // Nor once the buffer has gone.
        drop(index);
        assert_eq!(newer_than(&span, &last), None);
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

    /// Motion JPEG keeps the picture's levels: white comes back from a
    /// segment as white (235) on a channel, not grey (218, read as full range).
    #[tokio::test(flavor = "multi_thread")]
    async fn mjpeg_keeps_levels() {
        gst::init().unwrap();
        let source = test_pattern(VideoTestPattern::White);
        let clock = gst::SystemClock::obtain();
        let monitor =
            MonitorPipeline::new(&source, &MonitorSettingsDto::default(), &clock).unwrap();
        let dir = std::env::temp_dir().join(format!("capture-room-levels-{}", std::process::id()));
        let index = Index::create(&dir, gst::ClockTime::from_seconds(3));
        while monitor.video_frames() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let leg = ReplayLeg::start(&monitor, ReplayCodec::Mjpeg, &index, "test").unwrap();
        let mut first = None;
        for _ in 0..100 {
            first = index
                .lock()
                .unwrap()
                .segments
                .front()
                .map(|s| s.path.clone());
            if first.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let white = mean_luma(&first.expect("no segment closed"));
        assert!((white - 235.0).abs() < 3.0, "white came back as {white}");
        drop(leg);
        let _ = monitor.stop();
    }

    /// Sets `src`'s file as a property: in a launch string, a Windows path's
    /// backslashes would be read as escapes.
    fn set_location(p: &gst::Pipeline, file: &Path) {
        p.by_name("src")
            .unwrap()
            .set_property("location", file.to_str().unwrap());
    }

    /// A JPEG segment's first frame's mean luma, decoded as a channel's player
    /// does and converted to the program's format.
    fn mean_luma(file: &Path) -> f64 {
        let p = gst::parse::launch(
            "filesrc name=src ! qtdemux ! jpegdec ! videoconvert \
             ! video/x-raw,format=I420,colorimetry=bt709 ! appsink name=out",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        set_location(&p, file);
        let sink = p
            .by_name("out")
            .unwrap()
            .downcast::<gst_app::AppSink>()
            .unwrap();
        p.set_state(gst::State::Playing).unwrap();
        let sample = sink
            .try_pull_sample(gst::ClockTime::from_seconds(5))
            .expect("no frame decoded");
        p.set_state(gst::State::Null).unwrap();
        let info = gst_video::VideoInfo::from_caps(sample.caps().unwrap()).unwrap();
        let map = sample.buffer().unwrap().map_readable().unwrap();
        let (stride, width, height) = (
            info.stride()[0] as usize,
            info.width() as usize,
            info.height() as usize,
        );
        let sum: u64 = (0..height)
            .flat_map(|y| &map[y * stride..y * stride + width])
            .map(|&v| v as u64)
            .sum();
        sum as f64 / (width * height) as f64
    }

    /// Video frames in a .mov, and how long its sound lasts.
    fn count_frames(file: &Path) -> (usize, gst::ClockTime) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let p = gst::parse::launch(
            "filesrc name=src ! qtdemux name=d \
             d.video_0 ! queue ! fakesink name=v sync=false \
             d.audio_0 ! queue ! fakesink name=a sync=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        set_location(&p, file);
        let counts: Vec<Arc<AtomicUsize>> = ["v", "a"]
            .iter()
            .map(|name| {
                let n = Arc::new(AtomicUsize::new(0));
                let counter = Arc::clone(&n);
                // Frames counted, and the sound's length in ns.
                let audio = *name == "a";
                p.by_name(name)
                    .unwrap()
                    .static_pad("sink")
                    .unwrap()
                    .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                        let add = match info.buffer() {
                            Some(b) if audio => b.duration().map_or(0, |d| d.nseconds() as usize),
                            _ => 1,
                        };
                        counter.fetch_add(add, Ordering::SeqCst);
                        gst::PadProbeReturn::Ok
                    });
                n
            })
            .collect();
        p.set_state(gst::State::Playing).unwrap();
        let msg = p
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .unwrap_or_else(|| {
                // A missing track leaves its fakesink unprerolled, so the
                // read stalls in PAUSED rather than failing.
                let tracks: Vec<_> = p.by_name("d").unwrap().src_pads();
                let tracks: Vec<_> = tracks.iter().map(|pad| pad.name()).collect();
                panic!("reading the export timed out; its tracks: {tracks:?}")
            });
        assert!(matches!(msg.view(), gst::MessageView::Eos(_)), "{msg:?}");
        p.set_state(gst::State::Null).unwrap();
        (
            counts[0].load(Ordering::SeqCst),
            gst::ClockTime::from_nseconds(counts[1].load(Ordering::SeqCst) as u64),
        )
    }

    /// A saved clip keeps its files once the buffer has let them go, and
    /// exports to one .mov holding just its frames, with its sound.
    #[tokio::test(flavor = "multi_thread")]
    async fn saved_clip_outlives_buffer_and_exports() {
        gst::init().unwrap();
        let source = test_source();
        let clock = gst::SystemClock::obtain();
        let monitor =
            MonitorPipeline::new(&source, &MonitorSettingsDto::default(), &clock).unwrap();
        let root = std::env::temp_dir().join(format!("cr-replay-clip-{}", uuid::Uuid::new_v4()));
        let index = Index::create(&root.join("buffer"), gst::ClockTime::from_seconds(30));
        while monitor.video_frames() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let leg = ReplayLeg::start(&monitor, ReplayCodec::Mjpeg, &index, "test").unwrap();
        for _ in 0..200 {
            if index.lock().unwrap().segments.len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let (start, _) = index.lock().unwrap().span().expect("nothing recorded");

        // One second from the middle of the first segment, across into the
        // second.
        let from = start + gst::ClockTime::from_mseconds(500);
        let span = Index::clip(&index, from, Some(from + gst::ClockTime::SECOND)).unwrap();
        assert_eq!(span.files.len(), 2);
        let clip_dir = root.join("clip");
        let files = save(&span, &clip_dir).unwrap();
        let (base, from, to) = (span.base, span.from, span.to);
        drop(span);
        drop(leg);
        let _ = monitor.stop();
        drop(index);
        assert!(!root.join("buffer").exists());
        assert!(
            files.iter().all(|f| Path::new(f).is_file()),
            "a saved file went with the buffer"
        );

        let out = root.join("export.mov");
        let saved = files.clone();
        // An hour off this machine's own mapping, as a controller's might be.
        let offset = crate::clock::local_utc_offset_us(&clock).unwrap() + 3_600_000_000;
        let timecode = Some((base, offset));
        tokio::task::spawn_blocking(move || export(&saved, from, to, timecode, &out))
            .await
            .unwrap()
            .unwrap();
        // The in point's time of day, give or take the frame on the cut.
        let fps = gst::Fraction::new(30, 1);
        let frame = 1_000_000 / 30;
        let at = (base + from).useconds() as i64 + offset;
        let want: Vec<String> = [at - frame, at, at + frame]
            .iter()
            .map(|&t| time_code(t, fps).unwrap().to_string())
            .collect();
        match std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-show_entries", "stream_tags=timecode", "-of", "csv=p=0"])
            .arg(root.join("export.mov"))
            .output()
        {
            Ok(out) => {
                let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
                println!("export timecode {got}, expected about {}", want[1]);
                assert!(
                    want.contains(&got),
                    "the export's timecode: {got}, not {want:?}"
                );
            }
            Err(_) => println!("no ffprobe: not reading the timecode back"),
        }
        let (video, audio) = count_frames(&root.join("export.mov"));
        // 30 fps: a second's frames, give or take the one on the cut.
        assert!((29..=31).contains(&video), "{video} frames exported");
        // …and the sound that goes with them.
        let want = to - from;
        assert!(
            audio.nseconds().abs_diff(want.nseconds())
                < gst::ClockTime::from_mseconds(5).nseconds(),
            "{audio} of sound for {want} of pictures"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
