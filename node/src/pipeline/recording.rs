//! Recording legs, each in its own pipeline fed by the source's
//! [`MonitorPipeline`]:
//!
//! ```text
//! appsrc(video) → [videorate → caps] → [videoscale → caps] → videoconvert → [caps] → venc → [parse] ─┐
//!                                                                                                mux → filesink
//! appsrc(audio) → queue → audioconvert → audioresample → [caps] → aenc | audiorate (PCM) ────────┘
//! ```
//!
//! A failure (a codec the container rejects at runtime, a full disk) fails
//! only that leg, reported at once through [`OnLegError`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::profile::{
    available_encoders, has_encoder, segment_number, AudioFormat, RecordingProfile, VideoEncoder,
    PCM_MAX_FILE,
};
use super::{
    audio_hold_queue, capsfilter, describe_error, make_el, mark_prores_keyframes, pcm_audiorate,
    set_mix_matrix, set_property, start_or_reason, ConsumerPipeline,
};
use crate::api::types::{Container, RateControl};

/// Crash safety for .mov/.mp4:
///
/// - Compressed audio: written as fragments, finalised into an ordinary file
///   at the end. A crash loses at most the current fragment.
/// - PCM: qtmux's PCM fragments can't be read back (ffmpeg and qtdemux stop
///   at the first), so the index is kept at the front of the file and
///   rewritten every period. qtmux reserves 2 × ~550 B/s per track for the
///   longest file (~32 MB for 4 h) and errors when it fills, so these legs
///   always use splitmuxsink to start a new file in time.
///
/// `moov-recovery-file` + `qtmoovrecover` doesn't work: rebuilt H.264 + AAC
/// files didn't decode.
const CRASH_SAFE_FRAGMENT: gst::ClockTime = gst::ClockTime::from_seconds(2);
const CRASH_SAFE_PERIOD: gst::ClockTime = gst::ClockTime::from_seconds(10);

/// Called with the leg's output path and error message the first time a leg
/// fails. Runs on a GStreamer streaming thread, so it must not block.
pub type OnLegError = Arc<dyn Fn(&Path, String) + Send + Sync>;

pub struct RecordingLeg {
    consumer: ConsumerPipeline,
    /// The leg's (first) file.
    location: PathBuf,
    files: Arc<Mutex<Vec<PathBuf>>>,
    encoder: &'static str,
    /// Running time of the first video frame recorded; `u64::MAX` until one.
    first_frame: Arc<AtomicU64>,
    base_time: Option<gst::ClockTime>,
    /// The encoder's src pad, stamped with timecode for MOV.
    timecode_pad: Option<gst::Pad>,
}

/// Called when a splitting leg opens a new file. Runs on a GStreamer
/// streaming thread, so it must not block.
pub type OnLegFile = Arc<dyn Fn() + Send + Sync>;

/// Start one leg per `(path, profile)` on `monitor`. All legs are built
/// before any starts, so config errors open no file; if one fails to start,
/// all are rolled back and their files deleted.
///
/// `start_at` (monitor clock) drops earlier frames, so nodes sharing a clock
/// begin on the same frame. `utc_offset_us` (UTC minus the monitor's clock)
/// gives MOV files time-of-day timecode.
pub fn start_legs(
    monitor: &MonitorPipeline,
    tag: &str,
    legs: &[(PathBuf, RecordingProfile)],
    on_error: &OnLegError,
    on_file: &OnLegFile,
    start_at: Option<gst::ClockTime>,
    utc_offset_us: Option<i64>,
) -> Result<Vec<RecordingLeg>> {
    let built = legs
        .iter()
        .enumerate()
        .map(|(i, (path, profile))| {
            RecordingLeg::build(
                path,
                profile,
                &format!("rec-{tag}-{i}"),
                Arc::clone(on_error),
                Arc::clone(on_file),
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let mut started: Vec<RecordingLeg> = Vec::with_capacity(built.len());
    for mut leg in built {
        if let Err(e) = leg.start(monitor, start_at, utc_offset_us) {
            leg.discard();
            for leg in started {
                leg.discard();
            }
            return Err(e);
        }
        started.push(leg);
    }
    Ok(started)
}

/// Build every leg without starting it, to catch config errors. Opens no
/// files. A leg whose encoders this node lacks is skipped: another node may
/// have them, and a start there checks again.
pub fn check_legs(legs: &[(PathBuf, RecordingProfile)]) -> Result<()> {
    let on_error: OnLegError = Arc::new(|_, _| {});
    let on_file: OnLegFile = Arc::new(|| {});
    let available = available_encoders();
    for (i, (path, profile)) in legs.iter().enumerate() {
        if !has_encoder(profile, &available) {
            continue;
        }
        RecordingLeg::build(
            path,
            profile,
            &format!("check-{i}"),
            Arc::clone(&on_error),
            Arc::clone(&on_file),
        )?;
    }
    Ok(())
}

impl RecordingLeg {
    /// Build the pipeline with the first of the profile's encoders that
    /// links, left in NULL: the file isn't opened until [`Self::start`].
    fn build(
        path: &Path,
        profile: &RecordingProfile,
        name: &str,
        on_error: OnLegError,
        on_file: OnLegFile,
    ) -> Result<Self> {
        let encoders = profile.encoders();
        let mut last_err = None;
        for (i, &encoder) in encoders.iter().enumerate() {
            match Self::build_with(
                path,
                profile,
                encoder,
                name,
                Arc::clone(&on_error),
                Arc::clone(&on_file),
            ) {
                Ok(leg) => {
                    info!(path = ?path, encoder = encoder.element(), hardware = encoder.is_hardware(), "recording leg built");
                    return Ok(leg);
                }
                Err(e) if i + 1 < encoders.len() => {
                    warn!(encoder = encoder.element(), error = %format!("{e:#}"), "encoder unavailable, trying the next");
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("no encoder for {:?}", profile.video_codec)))
    }

    fn build_with(
        path: &Path,
        profile: &RecordingProfile,
        encoder: VideoEncoder,
        name: &str,
        on_error: OnLegError,
        on_file: OnLegFile,
    ) -> Result<Self> {
        let location = path.to_str().context("output path not valid UTF-8")?;
        let consumer = ConsumerPipeline::new(name);
        let ConsumerPipeline {
            pipeline,
            video_src,
            audio_src,
            error,
            ..
        } = &consumer;
        // Only errors and EOS are read; drop the rest so a long recording
        // doesn't pile up messages. Errors are reported at once and still
        // pass, so `stop` finds them too.
        let first_error = error.clone();
        let error_path = path.to_path_buf();
        pipeline
            .bus()
            .context("recording pipeline has no bus")?
            .set_sync_handler(move |_, msg| match msg.view() {
                gst::MessageView::Error(err) => {
                    if first_error.set(err) {
                        on_error(&error_path, leg_error(err));
                    }
                    gst::BusSyncReply::Pass
                }
                gst::MessageView::Eos(_) => gst::BusSyncReply::Pass,
                _ => gst::BusSyncReply::Drop,
            });

        let mut video = vec![video_src.clone().upcast::<gst::Element>()];
        // Before scaling; `mode=auto` passes progressive video through.
        if profile.deinterlace() {
            video.push(make_el("deinterlace", "deint")?);
        }
        if let Some((num, den)) = profile.framerate {
            // skip-to-first: otherwise videorate fills from the segment start
            // by duplicating the first frame.
            video.push(
                gst::ElementFactory::make("videorate")
                    .name("vrate")
                    .property("skip-to-first", true)
                    .build()
                    .context("create videorate")?,
            );
            video.push(capsfilter(
                "vrate-caps",
                gst::Caps::builder("video/x-raw")
                    .field("framerate", gst::Fraction::new(num as i32, den as i32))
                    .build(),
            )?);
        }
        if let Some((width, height)) = profile.resolution {
            video.push(make_el("videoscale", "vscale")?);
            // Pin PAR so videoscale resizes instead of changing PAR.
            video.push(capsfilter(
                "vscale-caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", width as i32)
                    .field("height", height as i32)
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .build(),
            )?);
        }
        // Handles formats the encoder can't take directly (NDI UYVY → I420).
        video.push(make_el("videoconvert", "vconv")?);
        if let Some(format) = profile.encoder_input_format(encoder) {
            video.push(capsfilter(
                "venc-caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", format)
                    .build(),
            )?);
        }
        let venc = build_video_encoder(profile, encoder)?;
        let timecode_pad = (profile.container == Container::Mov)
            .then(|| venc.static_pad("src"))
            .flatten();
        if encoder == VideoEncoder::VtProRes {
            mark_prores_keyframes(&venc)?;
        }
        video.push(venc);
        if let (VideoEncoder::VtProRes, Some(variant)) = (encoder, profile.prores_profile()) {
            // vtenc_prores picks its profile from downstream caps.
            video.push(capsfilter(
                "venc-out-caps",
                gst::Caps::builder("video/x-prores")
                    .field("variant", variant)
                    .build(),
            )?);
        }
        if let Some(parser) = profile.video_parser_element() {
            video.push(make_el(parser, "vparse")?);
        }

        let aq = audio_hold_queue()?;
        let aconv = make_el("audioconvert", "aconv")?;
        let mix = profile.audio_mix();
        if let Some(rows) = mix.as_ref().and_then(|m| m.matrix.as_ref()) {
            set_mix_matrix(&aconv, rows);
        }
        let mut audio = vec![
            audio_src.clone().upcast::<gst::Element>(),
            aq,
            aconv,
            make_el("audioresample", "aresample")?,
        ];
        let pcm = profile.audio_format() == AudioFormat::Pcm24;
        if mix.is_some() || pcm {
            let mut caps = gst::Caps::builder("audio/x-raw");
            if let Some(channels) = mix.map(|m| m.channels) {
                caps = caps.field("channels", channels as i32);
                // Mono needs no mask, stereo gets left/right, more are plain
                // numbered channels.
                match channels {
                    1 => {}
                    2 => caps = caps.field("channel-mask", gst::Bitmask::new(0x3)),
                    _ => caps = caps.field("channel-mask", gst::Bitmask::new(0)),
                }
            }
            if pcm {
                caps = caps.field("format", "S24LE");
            }
            audio.push(capsfilter("aout-caps", caps.build())?);
        }
        if profile.relabel_pcm_channels() {
            // Same samples, layout dropped: see `relabel_pcm_channels`.
            audio.push(
                gst::ElementFactory::make("capssetter")
                    .name("apcm-relabel")
                    .property(
                        "caps",
                        gst::Caps::builder("audio/x-raw")
                            .field("channel-mask", gst::Bitmask::new(0))
                            .build(),
                    )
                    .build()
                    .context("create capssetter")?,
            );
        }
        match profile.audio_format() {
            // AAC and Opus tolerate timestamp gaps.
            AudioFormat::Pcm24 => audio.push(pcm_audiorate()?),
            AudioFormat::Aac { bitrate } => {
                let aenc = make_el("avenc_aac", "aenc")?;
                set_property(&aenc, "bitrate", &bitrate.to_string())?;
                audio.push(aenc);
            }
            AudioFormat::Opus { bitrate } => {
                let aenc = make_el("opusenc", "aenc")?;
                set_property(&aenc, "bitrate", &bitrate.to_string())?;
                audio.push(aenc);
            }
        }

        let muxer = make_el(profile.muxer_element(), "mux")?;
        match profile.container {
            // Timestamps are the monitor's running time: qtmux/mp4mux start
            // the file at zero themselves, matroskamux needs asking. A cut-off
            // .mkv is readable as is.
            Container::Mkv => set_property(&muxer, "offset-to-zero", "true")?,
            // See `CRASH_SAFE_*`.
            Container::Mov | Container::Mp4 if profile.audio_format() == AudioFormat::Pcm24 => {
                // Longest file plus slack: the file splits at a keyframe
                // after the limit.
                let longest = profile.max_file_duration().unwrap_or(PCM_MAX_FILE);
                let reserve = gst::ClockTime::from_nseconds(
                    longest.as_nanos() as u64 + CRASH_SAFE_PERIOD.nseconds(),
                );
                set_property(
                    &muxer,
                    "reserved-max-duration",
                    &reserve.nseconds().to_string(),
                )?;
                set_property(
                    &muxer,
                    "reserved-moov-update-period",
                    &CRASH_SAFE_PERIOD.nseconds().to_string(),
                )?;
            }
            Container::Mov | Container::Mp4 => {
                set_property(
                    &muxer,
                    "fragment-duration",
                    &CRASH_SAFE_FRAGMENT.mseconds().to_string(),
                )?;
                set_property(&muxer, "fragment-mode", "first-moov-then-finalise")?;
            }
        }

        let files = Arc::new(Mutex::new(Vec::new()));
        let sink = match &profile.segment_template {
            None => {
                files.lock().unwrap().push(path.to_path_buf());
                gst::ElementFactory::make("filesink")
                    .name("sink")
                    .property("location", location)
                    .build()
                    .context("create filesink")?
            }
            Some(template) => {
                let split = gst::ElementFactory::make("splitmuxsink")
                    .name("sink")
                    .property("muxer", &muxer)
                    .build()
                    .context("create splitmuxsink")?;
                if let Some(d) = profile.max_file_duration() {
                    split.set_property("max-size-time", d.as_nanos() as u64);
                }
                if let Some(bytes) = profile.split_bytes() {
                    split.set_property("max-size-bytes", bytes);
                }
                // Keyframe requests are ignored with a size limit, which
                // then splits at the next natural keyframe.
                split.set_property("send-keyframe-requests", profile.split_bytes().is_none());
                let template = template.clone();
                let first = path.to_path_buf();
                let leg_files = Arc::clone(&files);
                split.connect("format-location", false, move |args| {
                    let index = args[1].get::<u32>().unwrap_or(0);
                    // The first file is the leg's own path.
                    let file = match index {
                        0 => first.clone(),
                        _ => PathBuf::from(template.replace("{segment}", &segment_number(index))),
                    };
                    // `{segment}` may name a folder.
                    if let Some(dir) = file.parent() {
                        if let Err(e) = std::fs::create_dir_all(dir) {
                            warn!(dir = ?dir, error = %e, "could not create folder for split file");
                        }
                    }
                    info!(path = ?file, "recording leg opened file");
                    leg_files.lock().unwrap().push(file.clone());
                    on_file();
                    Some(file.to_string_lossy().into_owned().to_value())
                });
                split
            }
        };

        pipeline
            .add_many(video.iter().chain(&audio).chain([&sink]))
            .context("add recording elements")?;
        if profile.segment_template.is_none() {
            pipeline.add(&muxer).context("add muxer")?;
        }
        gst::Element::link_many(&video).context("link video chain")?;
        gst::Element::link_many(&audio).context("link audio chain")?;
        if profile.segment_template.is_none() {
            link_to_muxer(video.last().unwrap(), &muxer, "video_%u", profile)?;
            link_to_muxer(audio.last().unwrap(), &muxer, "audio_%u", profile)?;
            muxer.link(&sink).context("link mux → filesink")?;
        } else {
            // splitmuxsink hands its pads to the muxer it was given.
            link_to_muxer(video.last().unwrap(), &sink, "video", profile)?;
            link_to_muxer(audio.last().unwrap(), &sink, "audio_%u", profile)?;
        }

        Ok(Self {
            consumer,
            location: path.to_path_buf(),
            files,
            encoder: encoder.element(),
            first_frame: Arc::new(AtomicU64::new(u64::MAX)),
            base_time: None,
            timecode_pad,
        })
    }

    fn start(
        &mut self,
        monitor: &MonitorPipeline,
        start_at: Option<gst::ClockTime>,
        utc_offset_us: Option<i64>,
    ) -> Result<()> {
        self.base_time = monitor.share_timing(&self.consumer.pipeline);
        let start_rt = start_at
            .zip(self.base_time)
            .map(|(at, base)| at.saturating_sub(base));
        gate(
            &self.consumer.video_src,
            start_rt,
            Some(Arc::clone(&self.first_frame)),
        )?;
        gate(&self.consumer.audio_src, start_rt, None)?;
        if let (Some(pad), Some(base), Some(offset)) =
            (&self.timecode_pad, self.base_time, utc_offset_us)
        {
            stamp_timecode(pad, move |segment, pts| {
                let rt = segment.map_or(Some(pts), |s| s.to_running_time(pts))?;
                Some((base + rt).useconds() as i64 + offset)
            });
        }

        // The bus handler keeps the reason (e.g. "Could not open file … for writing").
        let ConsumerPipeline {
            pipeline, error, ..
        } = &self.consumer;
        start_or_reason(pipeline, error, "pipeline failed to start")
            .map_err(|reason| anyhow!("{}: {reason}", self.location.display()))?;

        self.consumer.connect(monitor)?;
        info!(path = ?self.location, "recording leg started");
        Ok(())
    }

    /// When the first video frame recorded was due, on the monitor's clock.
    pub fn first_frame(&self) -> Option<gst::ClockTime> {
        let rt = self.first_frame.load(Ordering::Acquire);
        let base = self.base_time?;
        (rt != u64::MAX).then(|| gst::ClockTime::from_nseconds(rt) + base)
    }

    pub fn dropped_frames(&self) -> u64 {
        self.consumer.dropped_frames()
    }

    /// Drain to EOS so the muxer writes its index, waiting up to `timeout`.
    /// A recording error or a timeout is returned.
    pub async fn stop(mut self, timeout: Duration) -> Result<()> {
        self.consumer.disconnect();
        let _ = self.consumer.video_src.end_of_stream();
        let _ = self.consumer.audio_src.end_of_stream();

        let bus = self
            .consumer
            .pipeline
            .bus()
            .context("recording pipeline has no bus")?;
        let wait = gst::ClockTime::from_nseconds(timeout.as_nanos() as u64);
        let msg = tokio::task::spawn_blocking(move || {
            bus.timed_pop_filtered(wait, &[gst::MessageType::Eos, gst::MessageType::Error])
        })
        .await?;

        let result = match msg.as_ref().map(|m| m.view()) {
            Some(gst::MessageView::Eos(_)) => {
                info!(path = ?self.location, "recording leg finished");
                Ok(())
            }
            Some(gst::MessageView::Error(err)) => {
                warn!(path = ?self.location, error = %err.error(), debug = ?err.debug(), "recording leg failed");
                Err(anyhow!("{}: {}", self.location.display(), leg_error(err)))
            }
            _ => {
                warn!(path = ?self.location, "recording leg EOS timed out after {timeout:?}");
                Err(anyhow!(
                    "recording did not finish writing within {timeout:?}; the file may be incomplete"
                ))
            }
        };
        self.consumer.stop();
        // Remove empty files only: a leg that failed mid-recording (disk
        // full) may still be recoverable.
        for file in self.files.lock().unwrap().iter() {
            if std::fs::metadata(file).is_ok_and(|m| m.len() == 0) {
                remove_file(file);
            }
        }
        result
    }

    /// Tear down without finalising and delete the files: for rollback and
    /// benchmark legs.
    pub fn discard(mut self) {
        self.consumer.stop();
        for file in self.files.lock().unwrap().iter() {
            remove_file(file);
        }
    }

    pub fn files(&self) -> Vec<String> {
        self.files
            .lock()
            .unwrap()
            .iter()
            .map(|f| f.display().to_string())
            .collect()
    }

    pub fn encoder(&self) -> &'static str {
        self.encoder
    }
}

/// Drop `src`'s buffers due before `start` (running time), then note the
/// first one let through in `first` and get out of the way.
fn gate(
    src: &gst_app::AppSrc,
    start: Option<gst::ClockTime>,
    first: Option<Arc<AtomicU64>>,
) -> Result<()> {
    let pad = src.static_pad("src").context("appsrc src pad")?;
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(pts) = info.buffer().and_then(|b| b.pts()) else {
            return gst::PadProbeReturn::Ok;
        };
        // The producer forwards the monitor's segment; without one, the
        // timestamps are running time already.
        let rt = pad
            .sticky_event::<gst::event::Segment>(0)
            .and_then(|e| {
                e.segment()
                    .downcast_ref::<gst::ClockTime>()
                    .and_then(|s| s.to_running_time(pts))
            })
            .unwrap_or(pts);
        if start.is_some_and(|start| rt < start) {
            return gst::PadProbeReturn::Drop;
        }
        if let Some(first) = &first {
            first.store(rt.nseconds(), Ordering::Release);
        }
        gst::PadProbeReturn::Remove
    })
    .context("add start gate")?;
    Ok(())
}

/// Stamp each frame leaving `pad` with its local time of day as SMPTE
/// timecode. `to_utc_us` maps a PTS in its segment to UTC µs; using the clock
/// domain's offset makes files from nodes on one clock line up. qtmux writes
/// each file's first frame's into its timecode track. Existing timecode is
/// kept.
pub(super) fn stamp_timecode(
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
        let segment = pad.sticky_event::<gst::event::Segment>(0);
        let segment = segment
            .as_ref()
            .and_then(|e| e.segment().downcast_ref::<gst::ClockTime>());
        let fps = pad
            .current_caps()
            .and_then(|c| c.structure(0)?.get::<gst::Fraction>("framerate").ok())
            .filter(|f| f.numer() > 0 && f.denom() > 0);
        let tc = fps
            .zip(to_utc_us(segment, pts))
            .and_then(|(fps, utc_us)| time_code(utc_us, fps));
        if let (Some(tc), Some(buffer)) = (tc, info.buffer_mut()) {
            gst_video::VideoTimeCodeMeta::add(buffer.make_mut(), &tc);
        }
        gst::PadProbeReturn::Ok
    });
}

/// The timecode at `utc_us` (µs since the Unix epoch) in local time, at
/// `fps`. 29.97 and 59.94 count drop-frame, as their timecode does.
pub(super) fn time_code(utc_us: i64, fps: gst::Fraction) -> Option<gst_video::ValidVideoTimeCode> {
    let dt = glib::DateTime::from_unix_local(utc_us.div_euclid(1_000_000))
        .ok()?
        .add(glib::TimeSpan::from_microseconds(
            utc_us.rem_euclid(1_000_000),
        ))
        .ok()?;
    let flags = if fps.denom() == 1001 && fps.numer() % 30000 == 0 {
        gst_video::VideoTimeCodeFlags::DROP_FRAME
    } else {
        gst_video::VideoTimeCodeFlags::empty()
    };
    gst_video::VideoTimeCode::from_date_time(fps, &dt, flags, 0)
        .ok()?
        .try_into()
        .ok()
}

fn remove_file(location: &Path) {
    match std::fs::remove_file(location) {
        Ok(()) => info!(path = ?location, "removed unusable recording file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = ?location, error = %e, "could not remove recording file"),
    }
}

fn leg_error(err: &gst::message::Error) -> String {
    let element = err.src().map(|s| s.name().to_string()).unwrap_or_default();
    format!("recording failed in {element}: {}", describe_error(err))
}

/// Link an encoder to a new muxer pad, naming the combination on failure.
fn link_to_muxer(
    encoder: &gst::Element,
    muxer: &gst::Element,
    kind: &str,
    profile: &RecordingProfile,
) -> Result<()> {
    let incompatible = || {
        anyhow!(
            "{} output can't be muxed into .{} ({kind})",
            encoder
                .factory()
                .map(|f| f.name().to_string())
                .unwrap_or_default(),
            profile.file_extension(),
        )
    };
    let sink = muxer.request_pad_simple(kind).ok_or_else(incompatible)?;
    encoder
        .static_pad("src")
        .context("encoder src pad")?
        .link(&sink)
        .map_err(|_| incompatible())?;
    Ok(())
}

/// Create `encoder` with the output's rate control, keyframe interval and
/// speed preset. Quality 1–100 maps to x264/x265 CRF and VA-API QP 41→11,
/// VP9 `cq-level` 63→13, VideoToolbox `quality` 0.01→1. A rate control the
/// VA-API driver lacks fails the build, so the leg falls back to software.
fn build_video_encoder(profile: &RecordingProfile, encoder: VideoEncoder) -> Result<gst::Element> {
    let venc = make_el(encoder.element(), "venc")?;
    let set = |name: &str, value: &str| set_property(&venc, name, value);
    let rc = profile.rate_control();
    let quality = f64::from(profile.quality());
    let crf = (41.0 - 0.3 * quality).round().clamp(0.0, 51.0);
    let bitrate = profile.bitrate();
    let keyint = profile.keyframe_interval().to_string();
    match encoder {
        VideoEncoder::X264 => {
            // No `tune=zerolatency`: that's for streaming, and costs quality.
            set("speed-preset", profile.speed_preset())?;
            set("key-int-max", &keyint)?;
            match rc {
                // `pass=cbr` (default) is ABR capped at the bitrate; a 1 s
                // buffer lets it borrow bits for complex moments.
                RateControl::Average => set("vbv-buf-capacity", "1000")?,
                // HRD signalling makes x264 pad to a true constant rate.
                RateControl::Constant => set("nal-hrd", "cbr")?,
                RateControl::Quality => {
                    set("pass", "qual")?;
                    set("quantizer", &crf.to_string())?;
                }
            }
        }
        VideoEncoder::X265 => {
            set("speed-preset", profile.speed_preset())?;
            // The default `ssim` tunes for a metric, not viewing.
            set("tune", "0")?;
            set("key-int-max", &keyint)?;
            match (rc, bitrate) {
                (RateControl::Constant, Some(kbps)) => set(
                    "option-string",
                    &format!("vbv-maxrate={kbps}:vbv-bufsize={kbps}:strict-cbr=1"),
                )?,
                (RateControl::Quality, _) => set("option-string", &format!("crf={crf}"))?,
                _ => {}
            }
        }
        VideoEncoder::Vp9 => {
            // Realtime: libvpx's default "good" deadline can't keep up live.
            set("deadline", "1")?;
            set("cpu-used", "8")?;
            set("row-mt", "true")?;
            set("keyframe-max-dist", &keyint)?;
            match rc {
                RateControl::Average => {}
                RateControl::Constant => set("end-usage", "cbr")?,
                RateControl::Quality => {
                    set("end-usage", "cq")?;
                    set(
                        "cq-level",
                        &(63.0 - 0.5 * quality).round().clamp(0.0, 63.0).to_string(),
                    )?;
                }
            }
        }
        VideoEncoder::VtH264 | VideoEncoder::VtH265 => {
            let interval = (profile.keyframe_secs() * 1e9) as u64;
            set("max-keyframe-interval-duration", &interval.to_string())?;
            match rc {
                RateControl::Average => {}
                RateControl::Constant => set("rate-control", "cbr")?,
                // With no bitrate VideoToolbox encodes to `quality`.
                RateControl::Quality => set("quality", &(quality / 100.0).to_string())?,
            }
        }
        VideoEncoder::VaH264 | VideoEncoder::VaH265 => {
            // VA-API caps the GOP at 1024 frames.
            set(
                "key-int-max",
                &profile.keyframe_interval().min(1024).to_string(),
            )?;
            match rc {
                // VBR averages `target-percentage` (66% by default) of its
                // `bitrate` peak; the bitrate below is raised to match.
                RateControl::Average => set("rate-control", "vbr")?,
                RateControl::Constant => set("rate-control", "cbr")?,
                RateControl::Quality => {
                    set("rate-control", "cqp")?;
                    let qp = crf.to_string();
                    for frame in ["qpi", "qpp", "qpb"] {
                        set(frame, &qp)?;
                    }
                }
            }
        }
        VideoEncoder::ProResKs => {
            if let Some(prores) = profile.prores_profile() {
                set("profile", prores)?;
            }
        }
        VideoEncoder::VtProRes | VideoEncoder::Uncompressed => {}
    }
    if let Some(kbps) = bitrate {
        match (encoder, rc) {
            // In constant-quality mode VP9 treats the bitrate as a ceiling:
            // give it room.
            (VideoEncoder::Vp9, RateControl::Quality) => {
                set("target-bitrate", &(u64::from(kbps) * 4000).to_string())?
            }
            (VideoEncoder::Vp9, _) => set("target-bitrate", &(u64::from(kbps) * 1000).to_string())?,
            (_, RateControl::Quality) => {}
            (VideoEncoder::VaH264 | VideoEncoder::VaH265, RateControl::Average) => {
                set("bitrate", &(u64::from(kbps) * 3 / 2).to_string())?
            }
            (
                VideoEncoder::X264
                | VideoEncoder::X265
                | VideoEncoder::VtH264
                | VideoEncoder::VtH265
                | VideoEncoder::VaH264
                | VideoEncoder::VaH265,
                _,
            ) => set("bitrate", &kbps.to_string())?,
            _ => {}
        }
    }
    Ok(venc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{
        AudioTestSignal, ChromaSubsampling, MonitorSettingsDto, OutputAdvanced, PresetOutputInput,
        TestSourceConfig, VideoCodec, VideoTestPattern,
    };
    use crate::sources::test::TestSource;

    /// A leg told to start at a time begins with the first frame due at or
    /// after it; one without starts with whatever arrives.
    #[tokio::test(flavor = "multi_thread")]
    async fn legs_start_at_the_requested_time() {
        gst::init().unwrap();
        let source = TestSource::new(
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
        );
        let clock = gst::SystemClock::obtain();
        let monitor =
            MonitorPipeline::new(&source, &MonitorSettingsDto::default(), &clock).unwrap();
        while monitor.video_frames() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let dir = std::env::temp_dir().join(format!("capture-room-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile = RecordingProfile::from_output(&PresetOutputInput {
            name: "a".into(),
            codec: VideoCodec::H264,
            container: Container::Mkv,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: String::new(),
            preview: false,
            advanced: OutputAdvanced::default(),
        })
        .unwrap();
        let on_error: OnLegError = Arc::new(|_, e| panic!("leg failed: {e}"));
        let on_file: OnLegFile = Arc::new(|| {});

        let start_at = clock.time().unwrap() + gst::ClockTime::from_mseconds(1500);
        let gated = start_legs(
            &monitor,
            "gated",
            &[(dir.join("gated.mkv"), profile.clone())],
            &on_error,
            &on_file,
            Some(start_at),
            None,
        )
        .unwrap();
        let free = start_legs(
            &monitor,
            "free",
            &[(dir.join("free.mkv"), profile)],
            &on_error,
            &on_file,
            None,
            None,
        )
        .unwrap();

        // Waited for rather than slept: a loaded machine (the whole suite in
        // parallel) can take a while to get the first frame encoded.
        let free_first = loop {
            if let Some(first) = free[0].first_frame() {
                break first;
            }
            assert!(
                clock.time().unwrap() < start_at,
                "ungated leg recorded nothing"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(free_first < start_at);
        assert!(
            gated[0].first_frame().is_none(),
            "recorded before its start"
        );

        while clock.time().unwrap() < start_at + gst::ClockTime::from_mseconds(800) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let first = gated[0].first_frame().expect("gated leg never started");
        assert!(first >= start_at, "{first} is before {start_at}");
        let frame = gst::ClockTime::SECOND / 30;
        assert!(
            first < start_at + frame,
            "{first} is more than a frame after {start_at}"
        );

        for leg in gated.into_iter().chain(free) {
            leg.stop(Duration::from_secs(5)).await.unwrap();
        }
        let _ = monitor.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A MOV leg's file carries the timecode of its first frame's time of
    /// day on the clock (drop-frame at 29.97), from the UTC mapping given.
    #[tokio::test(flavor = "multi_thread")]
    async fn mov_files_carry_time_of_day_timecode() {
        gst::init().unwrap();
        let source = TestSource::new(
            "t".into(),
            "t".into(),
            TestSourceConfig {
                pattern: VideoTestPattern::Smpte,
                width: 320,
                height: 180,
                fps_num: 30000,
                fps_den: 1001,
                audio_signal: AudioTestSignal::Tone,
                frequency: 440.0,
                channels: 2,
            },
        );
        let clock = gst::SystemClock::obtain();
        let monitor =
            MonitorPipeline::new(&source, &MonitorSettingsDto::default(), &clock).unwrap();
        while monitor.video_frames() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let dir = std::env::temp_dir().join(format!("capture-room-tc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tc.mov");
        let profile = RecordingProfile::from_output(&PresetOutputInput {
            name: "a".into(),
            codec: VideoCodec::H264,
            container: Container::Mov,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: String::new(),
            preview: false,
            advanced: OutputAdvanced::default(),
        })
        .unwrap();
        let on_error: OnLegError = Arc::new(|_, e| panic!("leg failed: {e}"));
        let on_file: OnLegFile = Arc::new(|| {});
        // An hour off this machine's own mapping, as a controller's might be.
        let offset = crate::clock::local_utc_offset_us(&clock).unwrap() + 3_600_000_000;
        let legs = start_legs(
            &monitor,
            "tc",
            &[(file.clone(), profile)],
            &on_error,
            &on_file,
            None,
            Some(offset),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let first = legs[0].first_frame().expect("nothing recorded");
        for leg in legs {
            leg.stop(Duration::from_secs(5)).await.unwrap();
        }
        let _ = monitor.stop();

        let want = time_code(
            first.useconds() as i64 + offset,
            gst::Fraction::new(30000, 1001),
        )
        .unwrap()
        .to_string();
        assert!(want.contains(';'), "drop-frame: {want}");
        // Read back with ffprobe where it's installed.
        match std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-show_entries", "stream_tags=timecode", "-of", "csv=p=0"])
            .arg(&file)
            .output()
        {
            Ok(out) => {
                let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
                println!("file timecode {got}, expected {want}");
                assert_eq!(got, want, "the file's timecode");
            }
            Err(_) => println!("no ffprobe: not reading the file back"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Timecode counts frames of the time of day; a second is a second.
    #[test]
    fn time_code_counts_the_time_of_day() {
        gst::init().unwrap();
        let fps = gst::Fraction::new(25, 1);
        let t = 1_790_000_000_000_000; // a whole second
        let a = time_code(t, fps).unwrap();
        let b = time_code(t + 1_120_000, fps).unwrap();
        assert_eq!(a.frames(), 0);
        assert_eq!(b.frames(), 3, "0.12 s is 3 frames at 25 fps");
        assert_eq!((b.seconds() + 60 - a.seconds()) % 60, 1);
        assert!(!a
            .flags()
            .contains(gst_video::VideoTimeCodeFlags::DROP_FRAME));
    }
}
