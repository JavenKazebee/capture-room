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
//! keep running.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_utils::{ConsumptionLink, StreamProducer};
use tracing::{info, warn};

use super::monitor::MonitorPipeline;
use super::profile::RecordingProfile;

/// One running output leg.
pub struct RecordingLeg {
    pipeline: gst::Pipeline,
    video_src: gst_app::AppSrc,
    audio_src: gst_app::AppSrc,
    /// Connections to the monitor's producers; dropping one stops the feed.
    links: Vec<ConsumptionLink>,
    location: PathBuf,
}

/// Start one leg per `(path, profile)`, consuming `monitor`'s output. `tag`
/// names the pipelines in logs.
///
/// Every leg is built before any is started, so configuration errors
/// (missing encoder, a codec the container can't carry) fail without opening
/// a file. If starting a leg fails, every leg of this attempt is rolled back
/// and its partial file deleted.
pub fn start_legs(
    monitor: &MonitorPipeline,
    tag: &str,
    legs: &[(PathBuf, &RecordingProfile)],
) -> Result<Vec<RecordingLeg>> {
    let built = legs
        .iter()
        .enumerate()
        .map(|(i, (path, profile))| RecordingLeg::build(path, profile, &format!("rec-{tag}-{i}")))
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

impl RecordingLeg {
    /// Build and link the leg's pipeline, leaving it in NULL: the output file
    /// isn't opened until [`Self::start`].
    fn build(path: &Path, profile: &RecordingProfile, name: &str) -> Result<Self> {
        let location = path.to_str().context("output path not valid UTF-8")?;
        let pipeline = gst::Pipeline::with_name(name);
        // Only errors and EOS are ever read from this bus; drop the rest so a
        // long recording doesn't accumulate messages nobody pops.
        pipeline.bus().context("recording pipeline has no bus")?.set_sync_handler(|_, msg| {
            match msg.type_() {
                gst::MessageType::Error | gst::MessageType::Eos => gst::BusSyncReply::Pass,
                _ => gst::BusSyncReply::Drop,
            }
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
        if let Some(format) = profile.encoder_input_format() {
            video.push(capsfilter(
                "venc-caps",
                gst::Caps::builder("video/x-raw").field("format", format).build(),
            )?);
        }
        video.push(build_video_encoder(profile)?);
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
        let audio = vec![
            audio_src.clone().upcast::<gst::Element>(),
            aq,
            make_el("audioconvert", "aconv")?,
            make_el("audioresample", "aresample")?,
            build_audio_encoder(profile)?,
        ];

        let muxer = make_el(profile.muxer_element(), "mux")?;
        if profile.muxer_element() == "matroskamux" {
            // Timestamps arrive as the monitor's running time; qtmux/mp4mux
            // start the file at zero on their own, matroskamux needs asking.
            set_property(&muxer, "offset-to-zero", "true")?;
        }
        let filesink = gst::ElementFactory::make("filesink")
            .name("sink")
            .property("location", location)
            .build()
            .context("create filesink")?;

        pipeline
            .add_many(video.iter().chain(&audio).chain([&muxer, &filesink]))
            .context("add recording elements")?;
        gst::Element::link_many(&video).context("link video chain")?;
        gst::Element::link_many(&audio).context("link audio chain")?;
        link_to_muxer(video.last().unwrap(), &muxer, "video", profile)?;
        link_to_muxer(audio.last().unwrap(), &muxer, "audio", profile)?;
        muxer.link(&filesink).context("link mux → filesink")?;

        Ok(Self { pipeline, video_src, audio_src, links: Vec::new(), location: path.to_path_buf() })
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

        self.links.push(monitor.video.add_consumer(&self.video_src)?);
        self.links.push(monitor.audio.add_consumer(&self.audio_src)?);
        info!(path = ?self.location, "recording leg started");
        Ok(())
    }

    /// Stop feeding the leg, let it drain to EOS so the muxer writes its
    /// index, and wait up to `timeout` for that. An error the leg hit while
    /// recording, or a timeout, is returned as the stop's error.
    pub async fn stop(mut self, timeout: Duration) -> Result<()> {
        self.links.clear();
        let _ = self.video_src.end_of_stream();
        let _ = self.audio_src.end_of_stream();

        let bus = self.pipeline.bus().context("recording pipeline has no bus")?;
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
                let element = err.src().map(|s| s.name().to_string()).unwrap_or_default();
                warn!(
                    path = ?self.location,
                    element = %element,
                    error = %err.error(),
                    debug = ?err.debug(),
                    "recording leg failed"
                );
                Err(anyhow!("recording failed in {element}: {}", describe_error(err)))
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
        if std::fs::metadata(&self.location).is_ok_and(|m| m.len() == 0) {
            remove_file(&self.location);
        }
        result
    }

    /// Tear the leg down without finalizing it and delete its partial file.
    /// Only for rolling back a start that failed part-way.
    fn discard(mut self) {
        self.links.clear();
        let _ = self.pipeline.set_state(gst::State::Null);
        remove_file(&self.location);
    }
}

fn remove_file(location: &Path) {
    match std::fs::remove_file(location) {
        Ok(()) => info!(path = ?location, "removed unusable recording file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = ?location, error = %e, "could not remove recording file"),
    }
}

/// GStreamer's streaming errors are a generic "Internal data stream error"
/// with the real reason only in the debug text ("… reason not-negotiated
/// (-4)"); surface it.
fn describe_error(err: &gst::message::Error) -> String {
    let reason = err
        .debug()
        .and_then(|d| d.split("reason ").nth(1).map(|r| r.split(' ').next().unwrap_or(r).to_string()));
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

fn make_el(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("create {factory} (is its GStreamer plugin installed?)"))
}

fn capsfilter(name: &str, caps: gst::Caps) -> Result<gst::Element> {
    gst::ElementFactory::make("capsfilter")
        .name(name)
        .property("caps", caps)
        .build()
        .with_context(|| format!("create {name}"))
}

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
            encoder.factory().map(|f| f.name().to_string()).unwrap_or_default(),
            profile.file_extension(),
        )
    };
    let sink = muxer.request_pad_simple(&format!("{kind}_%u")).ok_or_else(incompatible)?;
    encoder
        .static_pad("src")
        .context("encoder src pad")?
        .link(&sink)
        .map_err(|_| incompatible())?;
    Ok(())
}

fn build_video_encoder(profile: &RecordingProfile) -> Result<gst::Element> {
    let name = profile.video_encoder_element();
    let venc = make_el(name, "venc")?;
    if let Some(kbps) = profile.bitrate_kbps {
        match name {
            "x264enc" | "x265enc" => set_property(&venc, "bitrate", &kbps.to_string())?,
            "vp9enc" => set_property(&venc, "target-bitrate", &(kbps * 1000).to_string())?,
            _ => {}
        }
    }
    if let Some(prores) = profile.prores_profile() {
        set_property(&venc, "profile", prores)?;
    }
    if name == "x264enc" {
        set_property(&venc, "tune", "zerolatency")?;
    }
    Ok(venc)
}

/// `set_property_from_str` panics on an unknown property; encoder builds
/// differ between platforms, so a missing one is an error instead.
fn set_property(el: &gst::Element, name: &str, value: &str) -> Result<()> {
    if el.find_property(name).is_none() {
        anyhow::bail!("{} has no `{name}` property", el.factory().map(|f| f.name()).unwrap_or_default());
    }
    el.set_property_from_str(name, value);
    Ok(())
}

fn build_audio_encoder(profile: &RecordingProfile) -> Result<gst::Element> {
    make_el(profile.audio_encoder_element(), "aenc")
}
