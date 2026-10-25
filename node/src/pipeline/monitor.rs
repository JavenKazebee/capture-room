//! The monitor: a source's always-on pipeline.
//!
//! ```text
//! source bin ─┬─ vtee ─┬─ videorate → videoscale → jpegenc → ThumbnailStore
//!             │        └─ appsink = video producer
//!             └─ atee ─┬─ level → AudioMeter → audio.levels events
//!                      └─ appsink = audio producer
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tracing::{error, warn};

use crate::api::types::MonitorSettingsDto;
use crate::sources::InputSource;

use super::{
    capsfilter, handle_level_message, link_tee, make_el, set_state, share_clock, AudioMeter,
    FirstError, ThumbnailStore,
};

/// The always-on pipeline for one source: thumbnails, audio levels, and the
/// raw producers that consumers take. Consumers run in their own pipelines so
/// a failing one can't stall the source or the others.
pub struct MonitorPipeline {
    pipeline: gst::Pipeline,
    clock: gst::Clock,
    pub thumbnail: ThumbnailStore,
    pub audio_meter: AudioMeter,
    pub video: StreamProducer,
    pub audio: StreamProducer,
    video_frames: Arc<FrameCount>,
    /// Shown on the source until the monitor restarts: an errored pipeline
    /// has stopped producing.
    error: FirstError,
    /// Reads the bus until aborted on drop: the bus stream never ends by
    /// itself, since the stream keeps the bus alive.
    bus_task: tokio::task::JoinHandle<()>,
    // Live-reconfigurable elements.
    thumb_rate_caps: gst::Element,
    thumb_scale_caps: gst::Element,
    level_el: gst::Element,
}

/// Frames a source has delivered. Frames counted against the span of their
/// timestamps (less marked gaps) shows a source falling behind, without the
/// jitter of comparing to the wall clock, since frames arrive in bursts.
#[derive(Debug, Clone, Copy, Default)]
pub struct VideoProgress {
    pub frames: u64,
    /// Running time.
    pub last_pts: Option<gst::ClockTime>,
    /// Total gaps before `DISCONT` frames (e.g. a looping file's restart),
    /// which aren't lost frames.
    pub skipped: gst::ClockTime,
}

/// A source's negotiated format; each part `None` until negotiated (or, for
/// the rate, if it's variable).
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceFormat {
    pub size: Option<(u32, u32)>,
    /// (numerator, denominator)
    pub rate: Option<(u32, u32)>,
    /// Channel count, and whether the channels have positions (a channel
    /// mask) rather than being plain numbered.
    pub audio: Option<(u32, bool)>,
}

impl MonitorPipeline {
    /// Build and start the monitor for `source` on `clock`.
    pub fn new(
        source: &dyn InputSource,
        config: &MonitorSettingsDto,
        clock: &gst::Clock,
    ) -> Result<Self> {
        let thumbnail = ThumbnailStore::default();
        let audio_meter = AudioMeter::default();

        let pipeline = gst::Pipeline::new();
        // Always the node clock: a device in the source bin (an audio
        // interface) would otherwise provide it, and recordings take the
        // monitor's clock.
        pipeline.use_clock(Some(clock));
        let src_bin = source.build_bin()?;
        pipeline.add(&src_bin).context("add source bin")?;

        let vtee = make_el("tee", "vtee")?;
        let atee = make_el("tee", "atee")?;
        pipeline.add_many([&vtee, &atee]).context("add tees")?;
        for (kind, tee) in [("video", &vtee), ("audio", &atee)] {
            src_bin
                .static_pad(kind)
                .with_context(|| format!("source {kind} pad"))?
                .link(&tee.static_pad("sink").context("tee sink")?)
                .with_context(|| format!("link source {kind} → tee"))?;
        }

        let (thumb_rate_caps, thumb_scale_caps) =
            add_thumbnail_branch(&pipeline, &vtee, thumbnail.clone(), config)?;
        let level_el = add_level_branch(&pipeline, &atee, config)?;
        let video = add_producer_branch(&pipeline, &vtee, "video")?;
        let audio = add_producer_branch(&pipeline, &atee, "audio")?;
        let video_frames = count_buffers(&video)?;

        let bus = pipeline.bus().context("pipeline has no bus")?;
        let audio_meter_ref = audio_meter.clone();
        let error = FirstError::default();
        let error_ref = error.clone();
        let bus_task = tokio::spawn(async move {
            let mut stream = bus.stream();
            while let Some(msg) = stream.next().await {
                match msg.view() {
                    gst::MessageView::Error(err) => {
                        error!(
                            src = %err.src().map(|s| s.name().to_string()).unwrap_or_default(),
                            msg = %err.error(),
                            debug = ?err.debug(),
                            "monitor pipeline error"
                        );
                        error_ref.set(err);
                    }
                    gst::MessageView::Warning(w) => {
                        warn!(msg = %w.error(), "monitor pipeline warning");
                    }
                    gst::MessageView::Element(el) => {
                        if let Some(s) = el.structure().filter(|s| s.name() == "level") {
                            handle_level_message(s, &audio_meter_ref);
                        }
                    }
                    _ => {}
                }
            }
        });

        // Don't wait on pipeline.state(): new() runs under the SourceManager
        // write lock, and blocking starves the WS emitter. The bus task
        // reports later errors.
        set_state(&pipeline, gst::State::Playing).map_err(|e| anyhow!("set PLAYING: {e:?}"))?;

        Ok(Self {
            pipeline,
            clock: clock.clone(),
            thumbnail,
            audio_meter,
            video,
            audio,
            video_frames,
            error,
            bus_task,
            thumb_rate_caps,
            thumb_scale_caps,
            level_el,
        })
    }

    pub fn error(&self) -> Option<String> {
        self.error.get()
    }

    pub fn video_frames(&self) -> u64 {
        self.video_frames.frames()
    }

    /// How far the source's video has got; see [`VideoProgress`].
    pub fn video_progress(&self) -> VideoProgress {
        self.video_frames.progress()
    }

    pub fn stop(&self) -> Result<()> {
        set_state(&self.pipeline, gst::State::Null)
            .map(|_| ())
            .map_err(|e| anyhow!("set NULL: {e:?}"))
    }

    pub fn clock(&self) -> &gst::Clock {
        &self.clock
    }

    /// The clock and base time a consumer pipeline should share, so buffer
    /// timestamps mean the same thing on both sides.
    pub fn timing(&self) -> (Option<gst::Clock>, Option<gst::ClockTime>) {
        (self.pipeline.clock(), self.pipeline.base_time())
    }

    /// Run `pipeline` on the monitor's clock and base time (see
    /// [`share_clock`]), answering the base time; `None`, leaving `pipeline`
    /// as it is, while the monitor has neither.
    pub fn share_timing(&self, pipeline: &gst::Pipeline) -> Option<gst::ClockTime> {
        let (Some(clock), Some(base_time)) = self.timing() else {
            return None;
        };
        share_clock(pipeline, &clock, base_time);
        Some(base_time)
    }

    /// Feed `video` and `audio` from the producers, until the links are
    /// dropped.
    pub fn feed(
        &self,
        video: &gst_app::AppSrc,
        audio: &gst_app::AppSrc,
    ) -> Result<Vec<ConsumptionLink>> {
        Ok(vec![
            self.video.add_consumer(video)?,
            self.audio.add_consumer(audio)?,
        ])
    }

    pub fn source_format(&self) -> SourceFormat {
        let current = |p: &StreamProducer| {
            p.appsink()
                .static_pad("sink")
                .and_then(|pad| pad.current_caps())
        };
        let audio = current(&self.audio).and_then(|caps| {
            let s = caps.structure(0)?;
            let channels = s.get::<i32>("channels").ok().filter(|&c| c > 0)? as u32;
            let mask = s.get::<gst::Bitmask>("channel-mask").map_or(0, |m| m.0);
            // Mono and stereo without a mask have their standard positions.
            Some((channels, mask != 0 || channels <= 2))
        });
        let caps = current(&self.video);
        let Some(s) = caps.as_ref().and_then(|c| c.structure(0)) else {
            return SourceFormat {
                audio,
                ..Default::default()
            };
        };
        let size = match (s.get::<i32>("width"), s.get::<i32>("height")) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
            _ => None,
        };
        let rate = s
            .get::<gst::Fraction>("framerate")
            .ok()
            .filter(|f| f.numer() > 0 && f.denom() > 0)
            .map(|f| (f.numer() as u32, f.denom() as u32));
        SourceFormat { size, rate, audio }
    }

    /// Apply a new config in place, without restarting the pipeline.
    pub fn reconfigure(&self, config: &MonitorSettingsDto) {
        self.level_el
            .set_property("interval", level_interval_ns(config));
        self.thumb_rate_caps
            .set_property("caps", thumb_rate_caps(config));
        self.thumb_scale_caps
            .set_property("caps", thumb_scale_caps(config));
    }
}

impl Drop for MonitorPipeline {
    fn drop(&mut self) {
        let _ = self.stop();
        self.bus_task.abort();
    }
}

/// vtee → queue → videorate → capsfilter(fps) → videoscale
///       → capsfilter(WxH) → videoconvert → jpegenc → appsink
///
/// Returns the two capsfilters for live reconfiguration.
fn add_thumbnail_branch(
    pipeline: &gst::Pipeline,
    vtee: &gst::Element,
    store: ThumbnailStore,
    config: &MonitorSettingsDto,
) -> Result<(gst::Element, gst::Element)> {
    let rate_caps = capsfilter("thumb-rate-caps", thumb_rate_caps(config))?;
    let scale_caps = capsfilter("thumb-scale-caps", thumb_scale_caps(config))?;

    // Not synced to the clock: videorate picks frames by timestamp anyway,
    // and a synced sink holds each frame for the pipeline's latency (about
    // 1 s for live sources). Its queue then fills and blocks the tee, which
    // stalls the producers and makes recordings drop frames.
    let appsink = gst_app::AppSink::builder()
        .name("thumb-sink")
        .caps(&gst::Caps::builder("image/jpeg").build())
        .max_buffers(1)
        .drop(true)
        .sync(false)
        // Not waited on to start (see `add_producer_branch`).
        .async_(false)
        .build();
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_preroll(|_| Ok(gst::FlowSuccess::Ok))
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                store.store(&map);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // Leaky, so a slow thumbnail never holds up the other branches.
    let tq = make_el("queue", "tq")?;
    tq.set_property_from_str("leaky", "downstream");
    tq.set_property("max-size-buffers", 2u32);
    let chain = [
        tq,
        make_el("videorate", "thumb-rate")?,
        rate_caps.clone(),
        make_el("videoscale", "thumb-scale")?,
        scale_caps.clone(),
        make_el("videoconvert", "thumb-conv")?,
        make_el("jpegenc", "thumb-enc")?,
        appsink.upcast(),
    ];
    pipeline.add_many(&chain).context("add thumbnail branch")?;
    gst::Element::link_many(&chain).context("link thumbnail branch")?;
    link_tee(vtee, &chain[0])?;

    Ok((rate_caps, scale_caps))
}

/// atee → queue → audioconvert → level → fakesink
///
/// Returns the level element for live reconfiguration.
fn add_level_branch(
    pipeline: &gst::Pipeline,
    atee: &gst::Element,
    config: &MonitorSettingsDto,
) -> Result<gst::Element> {
    let level = make_el("level", "level")?;
    level.set_property("interval", level_interval_ns(config));
    level.set_property("post-messages", true);
    let fakesink = make_el("fakesink", "level-sink")?;
    fakesink.set_property("sync", false);
    // Not waited on to start (see `add_producer_branch`).
    fakesink.set_property("async", false);

    let chain = [
        make_el("queue", "lq")?,
        make_el("audioconvert", "level-conv")?,
        level.clone(),
        fakesink,
    ];
    pipeline.add_many(&chain).context("add level branch")?;
    gst::Element::link_many(&chain).context("link level branch")?;
    link_tee(atee, &chain[0])?;

    Ok(level)
}

/// tee → queue → appsink, wrapped in a [`StreamProducer`] that fans out to
/// consumer pipelines. A consumer's errors never travel back to the tee.
fn add_producer_branch(
    pipeline: &gst::Pipeline,
    tee: &gst::Element,
    kind: &str,
) -> Result<StreamProducer> {
    let queue = make_el("queue", &format!("{kind}-producer-queue"))?;
    let appsink = gst_app::AppSink::builder()
        .name(format!("{kind}-producer"))
        // Forward as soon as buffers arrive; recordings don't render.
        .sync(false)
        // Nor wait to start: an async sink holds its first buffer until
        // PLAYING, so its queue can't drain, a compositor's allocation query
        // through the tee waits forever, the thumbnail sink (videorate needs
        // two frames) never prerolls, and the monitor never starts.
        .async_(false)
        .build();
    pipeline
        .add_many([&queue, appsink.upcast_ref()])
        .with_context(|| format!("add {kind} producer"))?;
    queue
        .link(&appsink)
        .with_context(|| format!("link {kind} queue → producer"))?;
    link_tee(tee, &queue)?;
    Ok(StreamProducer::from(&appsink))
}

/// Counts video frames from a streaming thread, for [`VideoProgress`].
pub struct FrameCount {
    frames: AtomicU64,
    /// Nanoseconds; `u64::MAX` until a timestamped frame arrives.
    last_pts: AtomicU64,
    skipped_ns: AtomicU64,
}

impl Default for FrameCount {
    fn default() -> Self {
        Self {
            frames: AtomicU64::new(0),
            last_pts: AtomicU64::new(u64::MAX),
            skipped_ns: AtomicU64::new(0),
        }
    }
}

impl FrameCount {
    /// Count a frame stamped `pts` (if it has a timestamp) that follows a
    /// `gap` the source marked: see [`VideoProgress::skipped`].
    pub fn record(&self, pts: Option<gst::ClockTime>, gap: gst::ClockTime) {
        if gap > gst::ClockTime::ZERO {
            self.skipped_ns.fetch_add(gap.nseconds(), Ordering::AcqRel);
        }
        if let Some(pts) = pts {
            self.last_pts.store(pts.nseconds(), Ordering::Release);
        }
        self.frames.fetch_add(1, Ordering::Release);
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    fn last_pts(&self) -> Option<gst::ClockTime> {
        let pts = self.last_pts.load(Ordering::Acquire);
        (pts != u64::MAX).then(|| gst::ClockTime::from_nseconds(pts))
    }

    pub fn progress(&self) -> VideoProgress {
        // A frame can land between these reads, leaving the count one ahead
        // of the timestamp: readers allow a frame of slack.
        let skipped = self.skipped_ns.load(Ordering::Acquire);
        let last_pts = self.last_pts();
        let frames = self.frames.load(Ordering::Acquire);
        VideoProgress {
            frames,
            last_pts,
            skipped: gst::ClockTime::from_nseconds(skipped),
        }
    }
}

fn count_buffers(producer: &StreamProducer) -> Result<Arc<FrameCount>> {
    let count = Arc::new(FrameCount::default());
    let counter = Arc::clone(&count);
    producer
        .appsink()
        .static_pad("sink")
        .context("producer sink pad")?
        .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            let pts = buffer.pts();
            let gap = match (pts, counter.last_pts()) {
                (Some(pts), Some(prev)) if buffer.flags().contains(gst::BufferFlags::DISCONT) => {
                    // The gap, less the frame interval that would be there anyway.
                    let interval = buffer.duration().unwrap_or(gst::ClockTime::ZERO);
                    pts.saturating_sub(prev).saturating_sub(interval)
                }
                _ => gst::ClockTime::ZERO,
            };
            counter.record(pts, gap);
            gst::PadProbeReturn::Ok
        });
    Ok(count)
}

fn thumb_rate_caps(config: &MonitorSettingsDto) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("framerate", gst::Fraction::new(config.thumb_fps, 1))
        .build()
}

/// Square pixels, which a JPEG assumes: given a free pixel aspect ratio,
/// `videoscale` would stretch other shapes to fill WxH. Fixed, it keeps the
/// source's shape and adds black borders.
fn thumb_scale_caps(config: &MonitorSettingsDto) -> gst::Caps {
    gst::Caps::builder("video/x-raw")
        .field("width", config.thumb_width)
        .field("height", config.thumb_height)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build()
}

fn level_interval_ns(config: &MonitorSettingsDto) -> u64 {
    config.level_interval_ms * 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{AudioTestSignal, TestSourceConfig, VideoTestPattern};
    use crate::sources::test::TestSource;
    use std::time::Duration;

    /// A 4:3 source's thumbnail is the configured size with square pixels:
    /// pillarboxed, not stretched to 16:9.
    #[tokio::test(flavor = "multi_thread")]
    async fn thumbnails_keep_the_source_aspect_ratio() {
        gst::init().unwrap();
        let source = TestSource::new(
            "t".into(),
            "t".into(),
            TestSourceConfig {
                pattern: VideoTestPattern::Smpte,
                width: 640,
                height: 480,
                fps_num: 30,
                fps_den: 1,
                audio_signal: AudioTestSignal::Tone,
                frequency: 440.0,
                channels: 2,
            },
        );
        let clock = gst::SystemClock::obtain();
        let monitor =
            MonitorPipeline::new(&source, &MonitorSettingsDto::default(), &clock).unwrap();
        while monitor.thumbnail.get().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let caps = monitor
            .thumb_scale_caps
            .static_pad("src")
            .unwrap()
            .current_caps()
            .unwrap();
        let s = caps.structure(0).unwrap();
        assert_eq!(s.get::<i32>("width").unwrap(), 320);
        assert_eq!(s.get::<i32>("height").unwrap(), 180);
        assert_eq!(
            s.get::<gst::Fraction>("pixel-aspect-ratio").unwrap(),
            gst::Fraction::new(1, 1)
        );
    }
}
