use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;
use gstreamer_pbutils as gst_pbutils;
use tracing::warn;

use super::player::{self, Kind, QueueLimit};
use super::{add_ghost_pad, InputSource};
use crate::api::types::{FileSourceConfig, MediaInfo, SourceCapabilitiesDto, SourceType};
use crate::pipeline::{capsfilter, make_el};

/// Extensions the file browser lists as media files.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mov", "mp4", "m4v", "mkv", "webm", "mxf", "avi", "mts", "m2ts", "ts", "mpg", "mpeg", "flv",
    "wmv",
];

/// Channels of the silence played for a file without audio.
const SILENCE_CHANNELS: u32 = 2;

/// The audio channels a file source delivers: the footage's, or
/// [`SILENCE_CHANNELS`] for footage without audio.
pub fn audio_channels(media: &MediaInfo) -> u32 {
    if media.audio_channels > 0 {
        media.audio_channels
    } else {
        SILENCE_CHANNELS
    }
}

// ── FileSource ────────────────────────────────────────────────────────────────

/// A media file on the node, played in a loop as a live feed.
///
/// The file plays in its own *player* pipeline, paced by the clock, whose
/// appsinks push into appsrcs in the monitor's bin. The appsrcs are live and
/// restamp every buffer with the monitor's running time, so the monitor sees
/// one continuous live feed like any other source, and the player can loop
/// with a flushing seek without disturbing it.
pub struct FileSource {
    id: String,
    name: String,
    config: FileSourceConfig,
}

impl FileSource {
    pub fn new(id: String, name: String, config: FileSourceConfig) -> Self {
        Self { id, name, config }
    }

    fn media(&self) -> Result<&MediaInfo> {
        self.config.media.as_ref().ok_or_else(|| {
            anyhow!(
                "{} hasn't been probed; save the source again",
                self.config.path
            )
        })
    }
}

impl InputSource for FileSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    fn source_type(&self) -> SourceType {
        SourceType::File
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        let media = self.config.media.as_ref()?;
        Some(SourceCapabilitiesDto {
            max_width: media.width,
            max_height: media.height,
            max_framerate: [media.fps_num, media.fps_den],
            audio_channels: audio_channels(media),
        })
    }

    fn fingerprint(&self) -> String {
        format!("{:?}", self.config)
    }

    fn build_bin(&self) -> Result<gst::Element> {
        Ok(build_bin(&self.id, &self.config.path, self.media()?)?.upcast())
    }
}

// ── Probe ─────────────────────────────────────────────────────────────────────

/// Blocking: read `path`'s streams. Fails unless it's a decodable file with a
/// video stream that isn't a still image.
pub fn probe(path: &str) -> Result<MediaInfo> {
    let p = Path::new(path);
    if !p.is_absolute() {
        bail!("path must be absolute");
    }
    if !p.is_file() {
        bail!("{path} is not a file");
    }
    let discoverer = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(10))
        .context("create discoverer")?;
    let info = discoverer
        .discover_uri(&player::uri(path)?)
        .map_err(|e| anyhow!("can't read {path}: {e}"))?;
    if info.result() != gst_pbutils::DiscovererResult::Ok {
        bail!("can't read {path}: {:?}", info.result());
    }
    let video = info
        .video_streams()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("{path} has no video"))?;
    if video.is_image() {
        bail!("{path} is a still image");
    }
    let rate = video.framerate();
    Ok(MediaInfo {
        width: video.width(),
        height: video.height(),
        fps_num: rate.numer().max(0) as u32,
        fps_den: rate.denom().max(1) as u32,
        audio_channels: info.audio_streams().first().map_or(0, |a| a.channels()),
        duration_ms: info.duration().map(|d| d.mseconds()),
    })
}

// ── Monitor side ──────────────────────────────────────────────────────────────

fn build_bin(id: &str, path: &str, media: &MediaInfo) -> Result<gst::Bin> {
    let bin = gst::Bin::with_name(&format!("filesrc-bin-{id}"));
    let latency = (media.fps_num > 0)
        .then(|| gst::ClockTime::SECOND.mul_div_ceil(media.fps_den as u64, media.fps_num as u64))
        .flatten()
        .unwrap_or(gst::ClockTime::ZERO);

    // ── Video: appsrc → videoconvert ──────────────────────────────────────────
    let vsrc = live_appsrc(&format!("file-vsrc-{id}"), latency);
    let vconv = make_el("videoconvert", &format!("file-vconv-{id}"))?;
    bin.add_many([vsrc.upcast_ref(), &vconv])
        .context("add file video elements")?;
    vsrc.link(&vconv).context("link file video")?;
    add_ghost_pad(&bin, &vconv, "video")?;

    // ── Audio: appsrc → audioconvert, or silence if the file has none ─────────
    let asrc = if media.audio_channels > 0 {
        let asrc = live_appsrc(&format!("file-asrc-{id}"), latency);
        let aconv = make_el("audioconvert", &format!("file-aconv-{id}"))?;
        bin.add_many([asrc.upcast_ref(), &aconv])
            .context("add file audio elements")?;
        asrc.link(&aconv).context("link file audio")?;
        add_ghost_pad(&bin, &aconv, "audio")?;
        Some(asrc)
    } else {
        let silence = make_el("audiotestsrc", &format!("file-silence-{id}"))?;
        silence.set_property("is-live", true);
        silence.set_property_from_str("wave", "silence");
        let caps = capsfilter(
            &format!("file-acaps-{id}"),
            gst::Caps::builder("audio/x-raw")
                .field("channels", SILENCE_CHANNELS as i32)
                .build(),
        )?;
        let audio = [
            silence,
            make_el("audioconvert", &format!("file-aconv-{id}"))?,
            caps,
        ];
        bin.add_many(&audio).context("add file silence")?;
        gst::Element::link_many(&audio).context("link file silence")?;
        add_ghost_pad(&bin, &audio[2], "audio")?;
        None
    };

    let player = build_player(id, path, &vsrc, asrc.as_ref())?;

    // Start the player once the monitor wants frames; until the appsrcs are
    // playing they would refuse them.
    let started = AtomicBool::new(false);
    let player_weak = player.downgrade();
    vsrc.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |_, _| {
                if started.swap(true, Ordering::SeqCst) {
                    return;
                }
                if let Some(player) = player_weak.upgrade() {
                    if let Err(e) = player.set_state(gst::State::Playing) {
                        warn!(error = ?e, "file player failed to start");
                    }
                }
            })
            .build(),
    );

    // Report the player's errors on the monitor, and loop it at the end.
    let bus = player.bus().context("player has no bus")?;
    let bus_task = tokio::spawn(watch_player(
        bus,
        player.downgrade(),
        vsrc.downgrade(),
        path.to_string(),
    ));

    // The player lives as long as the bin: the monitor drops the bin when it
    // stops, which stops the player.
    let _ = bin.add_weak_ref_notify(move || {
        bus_task.abort();
        let _ = player.set_state(gst::State::Null);
    });

    Ok(bin)
}

fn live_appsrc(name: &str, latency: gst::ClockTime) -> gst_app::AppSrc {
    // Never queue more than a few buffers: drop the oldest if the monitor
    // falls behind, as a live source would.
    player::live_appsrc(name, latency, QueueLimit::Buffers(4))
}

// ── Player ────────────────────────────────────────────────────────────────────

/// uridecodebin ─┬→ queue → videoconvert → appsink ⇢ `vsrc`
///               ├→ queue → audioconvert → audioresample → appsink ⇢ `asrc`
///               └→ fakesink (any other stream)
fn build_player(
    id: &str,
    path: &str,
    vsrc: &gst_app::AppSrc,
    asrc: Option<&gst_app::AppSrc>,
) -> Result<gst::Pipeline> {
    let player = gst::Pipeline::with_name(&format!("file-player-{id}"));
    let decode = make_el("uridecodebin", &format!("file-decode-{id}"))?;
    decode.set_property("uri", player::uri(path)?);
    player.add(&decode).context("add uridecodebin")?;

    let prefix = format!("file-{id}");
    let video = player::add_branch(&player, &prefix, Kind::Video, forward_to(vsrc))?;
    let audio = asrc
        .map(|asrc| player::add_branch(&player, &prefix, Kind::Audio, forward_to(asrc)))
        .transpose()?;

    // The first video and audio streams feed the source.
    let video = video.downgrade();
    let audio = audio.map(|a| a.downgrade());
    player::route_streams(&player, &decode, move |kind| {
        let branch = match kind {
            Kind::Video => video.upgrade(),
            Kind::Audio => audio.as_ref().and_then(|a| a.upgrade()),
        };
        branch?.static_pad("sink").filter(|p| !p.is_linked())
    });

    Ok(player)
}

/// An appsink's callbacks forwarding into `target`.
fn forward_to(target: &gst_app::AppSrc) -> gst_app::AppSinkCallbacks {
    let target = target.downgrade();
    let last_pts = AtomicU64::new(u64::MAX);
    gst_app::AppSinkCallbacks::builder()
        .new_sample(move |sink| {
            let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
            let Some(target) = target.upgrade() else {
                return Err(gst::FlowError::Flushing);
            };
            forward(&sample, &target, &last_pts);
            Ok(gst::FlowSuccess::Ok)
        })
        .build()
}

/// Push `sample` into `target`, stamped with `target`'s running time: the
/// file's own timestamps restart at every loop. The first buffer of each loop
/// is marked `DISCONT`, since a stream shorter than the file's longest one
/// leaves a gap there: counters can then tell a loop's gap from a feed
/// falling behind. `last_pts` is the file timestamp of the previous buffer.
fn forward(sample: &gst::Sample, target: &gst_app::AppSrc, last_pts: &AtomicU64) {
    // Not playing yet, or stopping.
    let Some(now) = target.current_running_time() else {
        return;
    };
    let looped = sample.buffer().and_then(|b| b.pts()).is_some_and(|pts| {
        let prev = last_pts.swap(pts.nseconds(), Ordering::Relaxed);
        prev != u64::MAX && pts.nseconds() <= prev
    });
    let Some(buffer) = player::restamp(sample, target, now, looped) else {
        return;
    };
    // Fails only while the monitor is stopping.
    let _ = target.push_buffer(buffer);
}

/// Loop the player at the end of the file, and post its errors on the
/// monitor (through `report_to`, an element in the monitor's bin) so they
/// show on the source. Runs until the player is dropped.
async fn watch_player(
    bus: gst::Bus,
    player: glib::WeakRef<gst::Pipeline>,
    report_to: glib::WeakRef<gst_app::AppSrc>,
    path: String,
) {
    // Loops with segment seeks: a segment seek ends in SEGMENT_DONE instead
    // of EOS, and the next (non-flushing) one continues straight on without a
    // gap. The first has to flush, so it's made as soon as the player has
    // prerolled.
    let segment_seek = |player: &gst::Pipeline, flags: gst::SeekFlags| {
        player.seek(
            1.0,
            flags | gst::SeekFlags::SEGMENT,
            gst::SeekType::Set,
            gst::ClockTime::ZERO,
            gst::SeekType::None,
            gst::ClockTime::NONE,
        )
    };
    let mut looping = false;
    let mut stream = bus.stream();
    while let Some(msg) = stream.next().await {
        let flags = match msg.view() {
            gst::MessageView::AsyncDone(_) if !looping => {
                looping = true;
                gst::SeekFlags::FLUSH
            }
            gst::MessageView::SegmentDone(_) => gst::SeekFlags::empty(),
            // Only if a segment seek didn't take.
            gst::MessageView::Eos(_) => gst::SeekFlags::FLUSH,
            gst::MessageView::Error(err) => {
                report(&path, &err.error().to_string(), &report_to, &player);
                return;
            }
            _ => continue,
        };
        let Some(p) = player.upgrade() else { return };
        if let Err(e) = segment_seek(&p, flags) {
            report(&path, &format!("can't loop: {e}"), &report_to, &player);
            return;
        }
    }
}

/// Post a player error on the monitor, and stop the player.
fn report(
    path: &str,
    error: &str,
    report_to: &glib::WeakRef<gst_app::AppSrc>,
    player: &glib::WeakRef<gst::Pipeline>,
) {
    warn!(path = %path, error = %error, "file player error");
    if let Some(src) = report_to.upgrade() {
        src.post_error_message(gst::error_msg!(
            gst::StreamError::Failed,
            ("{path}: {error}")
        ));
    }
    if let Some(player) = player.upgrade() {
        let _ = player.set_state(gst::State::Null);
    }
}
