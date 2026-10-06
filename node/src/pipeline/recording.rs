//! Recording legs. Each output leg runs in its own pipeline, fed raw video and
//! audio by its source's [`MonitorPipeline`] producers:
//!
//! ```text
//! appsrc(video) → [videorate → caps] → [videoscale → caps] → videoconvert → [caps] → venc → [parse] ─┐
//!                                                                                                mux → filesink
//! appsrc(audio) → queue → audioconvert → audioresample → aenc ───────────────────────────────────┘
//! ```
//!
//! Whatever goes wrong in a leg — a codec the container rejects at runtime, a
//! full disk — fails only that leg's pipeline. The monitor and the other legs
//! keep running, and the failure is reported the moment it happens through the
//! leg's [`OnLegError`] callback.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::profile::{segment_number, AudioFormat, RecordingProfile, VideoEncoder, PCM_MAX_FILE};
use super::{capsfilter, make_el};
use crate::api::types::{Container, RateControl};

/// Crash safety for .mov/.mp4 (see where the muxer is configured in
/// [`RecordingLeg::build_with`]):
///
/// - Compressed audio: written as fragments while recording, each playable
///   once complete, and turned into an ordinary file when the leg finishes.
///   A crash loses at most the fragment being written. Costs nothing.
/// - PCM: qtmux writes fragments of raw audio that nothing can read back
///   (tested: ffmpeg and qtdemux both stop at the first one), so these keep
///   the index at the front of the file instead, rewritten every period.
///   That space is reserved up front, twice over (qtmux alternates two
///   copies): 2 × 550 B/s per track for the longest a file runs — about
///   32 MB for `PCM_MAX_FILE` (4 h), 4 MB for a 30-minute split. If the
///   space fills, qtmux stops the leg with an error ("Not enough free
///   reserved header space") rather than falling back, so these legs always
///   go through splitmuxsink and start a new file by then. 550 B/s is a
///   worst case: 1080p30 ProRes + PCM used about an eighth of it.
///
/// (qtmux's `moov-recovery-file` with `qtmoovrecover` was tried first: the
/// files it rebuilt from H.264 + AAC didn't decode.)
const CRASH_SAFE_FRAGMENT: gst::ClockTime = gst::ClockTime::from_seconds(2);
const CRASH_SAFE_PERIOD: gst::ClockTime = gst::ClockTime::from_seconds(10);

/// Called with the leg's output path and error message the first time a leg
/// fails. Runs on a GStreamer streaming thread, so it must not block.
pub type OnLegError = Arc<dyn Fn(&Path, String) + Send + Sync>;

/// One running output leg.
pub struct RecordingLeg {
    pipeline: gst::Pipeline,
    video_src: gst_app::AppSrc,
    audio_src: gst_app::AppSrc,
    /// Connections to the monitor's producers; dropping one stops the feed.
    links: Vec<ConsumptionLink>,
    /// The leg's (first) file.
    location: PathBuf,
    /// Every file written so far: just `location`, unless the leg splits.
    files: Arc<Mutex<Vec<PathBuf>>>,
    /// The video encoder element the leg was built with.
    encoder: &'static str,
}

/// Called when a splitting leg opens a new file. Runs on a GStreamer
/// streaming thread, so it must not block.
pub type OnLegFile = Arc<dyn Fn() + Send + Sync>;

/// Start one leg per `(path, profile)`, consuming `monitor`'s output. `tag`
/// names the pipelines in logs; `on_error` hears about any leg that fails.
///
/// Every leg is built before any is started, so configuration errors
/// (missing encoder, a codec the container can't carry) fail without opening
/// a file. If starting a leg fails, every leg of this attempt is rolled back
/// and its partial file deleted.
pub fn start_legs(
    monitor: &MonitorPipeline,
    tag: &str,
    legs: &[(PathBuf, RecordingProfile)],
    on_error: &OnLegError,
    on_file: &OnLegFile,
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
        if let Err(e) = leg.start(monitor) {
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

/// Build every leg without starting it, so a codec the container can't carry
/// or a missing encoder is caught without recording anything. Building opens
/// no files; the pipelines are dropped in NULL.
pub fn check_legs(legs: &[(PathBuf, RecordingProfile)]) -> Result<()> {
    let on_error: OnLegError = Arc::new(|_, _| {});
    let on_file: OnLegFile = Arc::new(|| {});
    for (i, (path, profile)) in legs.iter().enumerate() {
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
    /// Build and link the leg's pipeline with the first of the profile's
    /// encoders that this platform has and that links, leaving it in NULL:
    /// the output file isn't opened until [`Self::start`].
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
        let pipeline = gst::Pipeline::with_name(name);
        // Only errors and EOS are ever read from this bus; drop the rest so a
        // long recording doesn't accumulate messages nobody pops. Errors are
        // also reported straight away (once per leg) — they still pass, so
        // `stop` finds them too.
        let reported = AtomicBool::new(false);
        let error_path = path.to_path_buf();
        pipeline
            .bus()
            .context("recording pipeline has no bus")?
            .set_sync_handler(move |_, msg| match msg.view() {
                gst::MessageView::Error(err) => {
                    if !reported.swap(true, Ordering::Relaxed) {
                        on_error(&error_path, leg_error(err));
                    }
                    gst::BusSyncReply::Pass
                }
                gst::MessageView::Eos(_) => gst::BusSyncReply::Pass,
                _ => gst::BusSyncReply::Drop,
            });

        // Live, time format, and a 500 ms queue that drops the oldest buffers
        // if the encoder can't keep up — so a slow encoder only drops frames
        // here. `add_consumer` applies this too, but by then the pipeline is
        // playing and a base source only adopts its format before it starts.
        let video_src = gst_app::AppSrc::builder().name("video-src").build();
        let audio_src = gst_app::AppSrc::builder().name("audio-src").build();
        StreamProducer::configure_consumer(&video_src);
        StreamProducer::configure_consumer(&audio_src);

        let mut video = vec![video_src.clone().upcast::<gst::Element>()];
        // Interlaced sources are deinterlaced before scaling (`mode=auto`
        // passes progressive video straight through).
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
            // Pin the pixel aspect ratio so videoscale actually resizes instead
            // of satisfying the caps by changing PAR.
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
        if encoder == VideoEncoder::VtProRes {
            // vtenc_prores marks every frame a delta unit, but ProRes frames
            // are all keyframes: left as is, the muxer's keyframe table lists
            // almost none of them and splitmuxsink never finds a frame to
            // split at.
            venc.static_pad("src")
                .context("encoder src pad")?
                .add_probe(gst::PadProbeType::BUFFER, |_, info| {
                    if let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() {
                        buffer.make_mut().unset_flags(gst::BufferFlags::DELTA_UNIT);
                    }
                    gst::PadProbeReturn::Ok
                });
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

        // Large audio queue so the muxer can hold audio while the video encoder
        // produces its first frames, without the appsrc's 500 ms limit
        // dropping it.
        let aq = gst::ElementFactory::make("queue")
            .name("aq")
            .property("max-size-time", gst::ClockTime::from_seconds(10).nseconds())
            .property("max-size-bytes", 0u32)
            .property("max-size-buffers", 0u32)
            .build()
            .context("create audio queue")?;
        // A channel mix (stereo, picked channels) is audioconvert's matrix,
        // which has to be set before it negotiates.
        let aconv = make_el("audioconvert", "aconv")?;
        let mix = profile.audio_mix();
        if let Some(rows) = mix.as_ref().and_then(|m| m.matrix.as_ref()) {
            let matrix = gst::Array::new(
                rows.iter()
                    .map(|row| gst::Array::new(row.iter().copied()).to_send_value()),
            );
            aconv.set_property("mix-matrix", matrix);
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
            AudioFormat::Pcm24 => {}
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
            // Timestamps arrive as the monitor's running time; qtmux/mp4mux
            // start the file at zero on their own, matroskamux needs asking.
            // A cut-off .mkv is readable as it is.
            Container::Mkv => set_property(&muxer, "offset-to-zero", "true")?,
            // .mov/.mp4 need their index to play, and normally write it only
            // when they finish: make a crash or power cut leave a playable
            // file. See `CRASH_SAFE_*`.
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
                // Ask for a keyframe at the split point; ignored when a size
                // limit is set too, which then splits at the next one (at most
                // a keyframe interval late).
                split.set_property("send-keyframe-requests", profile.split_bytes().is_none());
                let template = template.clone();
                let first = path.to_path_buf();
                let leg_files = Arc::clone(&files);
                split.connect("format-location", false, move |args| {
                    let index = args[1].get::<u32>().unwrap_or(0);
                    // The first file is the leg's own path (plain, for a leg
                    // that only rolls over at `PCM_MAX_FILE`).
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
            pipeline,
            video_src,
            audio_src,
            links: Vec::new(),
            location: path.to_path_buf(),
            files,
            encoder: encoder.element(),
        })
    }

    /// Open the file, start the pipeline and connect it to the producers.
    fn start(&mut self, monitor: &MonitorPipeline) -> Result<()> {
        // Share the monitor's clock and base time, so the forwarded timestamps
        // mean the same running time on both sides.
        if let (Some(clock), Some(base_time)) = monitor.timing() {
            self.pipeline.use_clock(Some(&clock));
            self.pipeline.set_base_time(base_time);
            self.pipeline.set_start_time(gst::ClockTime::NONE);
        }

        if self.pipeline.set_state(gst::State::Playing).is_err() {
            // The reason (e.g. "Could not open file … for writing") is on the bus.
            let reason = self
                .pipeline
                .bus()
                .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error]))
                .and_then(|msg| match msg.view() {
                    gst::MessageView::Error(err) => Some(err.error().to_string()),
                    _ => None,
                })
                .unwrap_or_else(|| "pipeline failed to start".to_string());
            return Err(anyhow!("{}: {reason}", self.location.display()));
        }

        self.links
            .push(monitor.video.add_consumer(&self.video_src)?);
        self.links
            .push(monitor.audio.add_consumer(&self.audio_src)?);
        info!(path = ?self.location, "recording leg started");
        Ok(())
    }

    /// Video frames dropped so far because the leg couldn't keep up (its
    /// leaky input queue overflowed). Read from the appsrc rather than the
    /// `ConsumptionLink`: the link's count includes these same drops, plus
    /// keyframe waits that raw video never has.
    pub fn dropped_frames(&self) -> u64 {
        self.video_src.property::<u64>("dropped")
    }

    /// Stop feeding the leg, let it drain to EOS so the muxer writes its
    /// index, and wait up to `timeout` for that. An error the leg hit while
    /// recording, or a timeout, is returned as the stop's error.
    pub async fn stop(mut self, timeout: Duration) -> Result<()> {
        self.links.clear();
        let _ = self.video_src.end_of_stream();
        let _ = self.audio_src.end_of_stream();

        let bus = self
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
        let _ = self.pipeline.set_state(gst::State::Null);
        // A leg that never wrote anything (it failed straight away) leaves an
        // empty file behind. Anything with data is kept: a leg that failed
        // mid-recording (disk full) may still be recoverable.
        for file in self.files.lock().unwrap().iter() {
            if std::fs::metadata(file).is_ok_and(|m| m.len() == 0) {
                remove_file(file);
            }
        }
        result
    }

    /// Tear the leg down without finalizing it and delete its partial file.
    /// Only for rolling back a start that failed part-way, and for benchmark
    /// legs, whose files are thrown away.
    pub fn discard(mut self) {
        self.links.clear();
        let _ = self.pipeline.set_state(gst::State::Null);
        for file in self.files.lock().unwrap().iter() {
            remove_file(file);
        }
    }

    /// Every file the leg has written so far.
    pub fn files(&self) -> Vec<String> {
        self.files
            .lock()
            .unwrap()
            .iter()
            .map(|f| f.display().to_string())
            .collect()
    }

    /// The video encoder element the leg uses.
    pub fn encoder(&self) -> &'static str {
        self.encoder
    }
}

fn remove_file(location: &Path) {
    match std::fs::remove_file(location) {
        Ok(()) => info!(path = ?location, "removed unusable recording file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = ?location, error = %e, "could not remove recording file"),
    }
}

/// A leg's error message, naming the element that failed.
fn leg_error(err: &gst::message::Error) -> String {
    let element = err.src().map(|s| s.name().to_string()).unwrap_or_default();
    format!("recording failed in {element}: {}", describe_error(err))
}

/// GStreamer's streaming errors are a generic "Internal data stream error"
/// with the real reason only in the debug text ("… reason not-negotiated
/// (-4)"); surface it.
fn describe_error(err: &gst::message::Error) -> String {
    let reason = err.debug().and_then(|d| {
        d.split("reason ")
            .nth(1)
            .map(|r| r.split(' ').next().unwrap_or(r).to_string())
    });
    match reason.as_deref() {
        Some("not-negotiated") => format!(
            "{} (not-negotiated: the encoder's output isn't accepted by this container or format)",
            err.error()
        ),
        Some(reason) => format!("{} ({reason})", err.error()),
        None => err.error().to_string(),
    }
}

impl Drop for RecordingLeg {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

// ── Element builders ──────────────────────────────────────────────────────────

/// Link an encoder to a new muxer input, with an error that names the
/// combination when the container can't carry the codec.
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

/// Create `encoder` with the output's rate control (average bitrate unless
/// set; Auto sizes the bitrate from the frame size and rate), keyframe
/// interval, and speed preset — software encoders default fast enough to
/// keep up live.
///
/// Quality 1–100 maps onto each encoder's own scale: x264/x265 CRF 41→11,
/// VP9 `cq-level` 63→13, VideoToolbox `quality` 0.01→1.
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
                // `pass=cbr` (the default) is x264's ABR capped at the
                // bitrate; a 1 s buffer lets it borrow bits for complex
                // moments.
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
            // GStreamer defaults to `ssim`, which tunes for a metric rather
            // than for viewing; 0 is no tuning.
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
            (
                VideoEncoder::X264
                | VideoEncoder::X265
                | VideoEncoder::VtH264
                | VideoEncoder::VtH265,
                _,
            ) => set("bitrate", &kbps.to_string())?,
            _ => {}
        }
    }
    Ok(venc)
}

/// `set_property_from_str` panics on an unknown property; encoder builds
/// differ between platforms, so a missing one is an error instead.
fn set_property(el: &gst::Element, name: &str, value: &str) -> Result<()> {
    if el.find_property(name).is_none() {
        anyhow::bail!(
            "{} has no `{name}` property",
            el.factory().map(|f| f.name()).unwrap_or_default()
        );
    }
    el.set_property_from_str(name, value);
    Ok(())
}
