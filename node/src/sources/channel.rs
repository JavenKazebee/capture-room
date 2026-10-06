//! Playout channels.
//!
//! A channel's *program* is a source like any other: a bin with `"video"` and
//! `"audio"` pads, run by an ordinary monitor pipeline (so it has thumbnails
//! and meters, and can be recorded). The bin is a compositor over black and a
//! mixer over silence at the channel's fixed format (see
//! [`live::video_output`]), so the program never goes dark or changes format.
//!
//! A loaded clip plays in its own *player* pipeline, paced by the clock, whose
//! appsinks push into appsrcs added to the program bin for that clip (its
//! *lanes*). Removing the lanes leaves black and silence. A bad file fails
//! its player, never the program.
//!
//! Timestamps: a player buffer at running time `rt` is rendered when the clock
//! reads `player base time + rt`. It's restamped to that moment in the
//! program's running time, plus [`LEAD`] so it reaches the compositor ahead
//! of its slot. The same mapping applies to video and audio, so they stay in
//! sync, and since a paused pipeline moves its base time on resume, across
//! pauses too.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, glib, prelude::*};
use gstreamer_app as gst_app;
use tracing::{debug, warn};

use super::live::{self, Chain, Inputs};
use super::InputSource;
use crate::api::types::{
    ChannelConfig, ClipEnd, LiveVideoFormat, OutputConfig, SourceCapabilitiesDto, SourceType,
    TransportState,
};
use crate::pipeline::make_el;

/// How far ahead of its slot a clip's buffer is stamped. Covers the
/// program-side conversion, so frames are never late at the compositor.
const LEAD: gst::ClockTime = gst::ClockTime::from_mseconds(100);
/// How long a clip may take to load (open, decode and preroll).
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

// ── ChannelSource ─────────────────────────────────────────────────────────────

pub struct ChannelSource {
    id: String,
    name: String,
    config: ChannelConfig,
    playout: Arc<Playout>,
}

impl ChannelSource {
    pub fn new(id: String, name: String, config: ChannelConfig) -> Self {
        let playout = Arc::new(Playout::new(&id));
        Self {
            id,
            name,
            config,
            playout,
        }
    }
}

impl InputSource for ChannelSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn source_type(&self) -> SourceType {
        SourceType::Channel
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        let f = &self.config.format;
        Some(SourceCapabilitiesDto {
            max_width: f.width,
            max_height: f.height,
            max_framerate: [f.fps_num, f.fps_den],
            audio_channels: self.config.audio_channels,
        })
    }

    /// Outputs are left out: changing them restarts the outputs, not the
    /// program (and the clip playing on it).
    fn fingerprint(&self) -> String {
        format!(
            "{}|{:?}|{}",
            self.name, self.config.format, self.config.audio_channels
        )
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let id = &self.id;
        let bin = gst::Bin::with_name(&format!("channel-bin-{id}"));
        let (video, _) = live::video_output(&bin, id, program_caps(&self.config.format))?;
        let compositor = video.aggregator().context("compositor")?;
        live::add_background(&bin, id, &compositor)?;
        let audio = live::audio_output(&bin, id, self.config.audio_channels)?;
        self.playout.lanes.attach(video, audio);
        Ok(bin.upcast())
    }

    fn timecode(&self) -> Option<String> {
        None
    }

    fn playout(&self) -> Option<&Arc<Playout>> {
        Some(&self.playout)
    }

    fn outputs(&self) -> &[OutputConfig] {
        &self.config.outputs
    }
}

/// Check a config before it's saved. `others` are the node's other
/// channels, whose NDI names this one mustn't reuse.
pub fn validate(
    cfg: &ChannelConfig,
    name: &str,
    others: &[(&str, &ChannelConfig)],
) -> Result<(), String> {
    let f = &cfg.format;
    if f.width == 0 || f.height == 0 || f.width > 7680 || f.height > 4320 {
        return Err("the size must be from 1x1 up to 7680x4320".into());
    }
    if !f.width.is_multiple_of(2) || !f.height.is_multiple_of(2) {
        return Err("the width and height must be even".into());
    }
    if f.fps_num == 0 || f.fps_den == 0 || f.fps_num / f.fps_den > 240 {
        return Err("the frame rate must be above 0 and at most 240".into());
    }
    if !(1..=16).contains(&cfg.audio_channels) {
        return Err("audio must have 1 to 16 channels".into());
    }
    let ndi_names = |cfg: &ChannelConfig, name: &str| -> Vec<String> {
        cfg.outputs
            .iter()
            .map(|o| match o {
                OutputConfig::Ndi { ndi_name } => ndi_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(name)
                    .to_lowercase(),
            })
            .collect()
    };
    let mine = ndi_names(cfg, name);
    for (i, n) in mine.iter().enumerate() {
        if mine[..i].contains(n) {
            return Err(format!("two outputs are both called {n}"));
        }
        if let Some((other, _)) = others
            .iter()
            .find(|(other, ocfg)| ndi_names(ocfg, other).contains(n))
        {
            return Err(format!("NDI name {n} is already used by {other}"));
        }
    }
    Ok(())
}

/// The program's video caps: the channel's size and rate, progressive I420.
fn program_caps(format: &LiveVideoFormat) -> gst::Caps {
    let base = gst::Caps::builder("video/x-raw")
        .field("format", "I420")
        .field("colorimetry", "bt709")
        .field("chroma-site", "mpeg2")
        .build();
    live::lock_caps(&base, Some(format))
}

// ── Lanes (program side) ──────────────────────────────────────────────────────

/// The appsrcs a loaded clip pushes into, in the current program bin. The
/// monitor builds a new bin each time it (re)starts; [`Lanes::attach`] moves
/// the clip over to it.
pub struct Lanes {
    id: String,
    inner: Mutex<LanesInner>,
}

#[derive(Default)]
struct LanesInner {
    video_in: Option<Inputs>,
    audio_in: Option<Inputs>,
    /// Bumped for every set of lanes, to name their elements uniquely.
    serial: u32,
    /// Whether a clip wants lanes, and an audio one.
    want: Option<bool>,
    video: Lane,
    audio: Lane,
    #[cfg(test)]
    pushed: Vec<Pushed>,
}

#[derive(Default)]
struct Lane {
    src: Option<gst_app::AppSrc>,
    chain: Option<Chain>,
    /// Start and end (program time) of the last buffer pushed.
    last: Option<(gst::ClockTime, gst::ClockTime)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Video,
    Audio,
}

/// A buffer as pushed, for tests: its position in the clip (stream time)
/// and its program timestamp.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct Pushed {
    kind: Kind,
    clip: gst::ClockTime,
    program: gst::ClockTime,
    preroll: bool,
}

impl Lanes {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            inner: Mutex::default(),
        }
    }

    /// A new program bin was built: its inputs replace the old bin's (whose
    /// lanes go with it), and a loaded clip gets lanes in it.
    fn attach(&self, video: Inputs, audio: Inputs) {
        let mut inner = self.inner.lock().unwrap();
        inner.video_in = Some(video);
        inner.audio_in = Some(audio);
        inner.video = Lane::default();
        inner.audio = Lane::default();
        if let Some(audio) = inner.want {
            if let Err(e) = self.open_locked(&mut inner, audio) {
                warn!(channel = %self.id, error = %e, "channel lanes failed");
            }
        }
    }

    /// Add lanes for a clip (replacing any).
    fn open(&self, audio: bool) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        self.close_locked(&mut inner);
        inner.want = Some(audio);
        self.open_locked(&mut inner, audio)
    }

    fn open_locked(&self, inner: &mut LanesInner, audio: bool) -> Result<()> {
        // No bin yet: the lanes are made when the monitor builds one.
        let (Some(video_in), Some(audio_in)) = (inner.video_in.clone(), inner.audio_in.clone())
        else {
            return Ok(());
        };
        inner.serial += 1;
        let serial = inner.serial;
        inner.video = self.lane(&video_in, &format!("v{serial}"))?;
        if audio {
            inner.audio = self.lane(&audio_in, &format!("a{serial}"))?;
        }
        Ok(())
    }

    fn lane(&self, inputs: &Inputs, tag: &str) -> Result<Lane> {
        let bin = inputs.bin().context("program bin gone")?;
        let mut chain = inputs.add(tag)?;
        let src = gst_app::AppSrc::builder()
            .name(format!("channel-src-{tag}-{}", self.id))
            .is_live(true)
            .format(gst::Format::Time)
            // Buffers arrive LEAD early; hold that and some, then drop the
            // oldest rather than block the player.
            .max_bytes(0)
            .max_time(Some(gst::ClockTime::from_mseconds(500)))
            // Without its own latency a live appsrc reports a maximum of 0,
            // which the aggregators can't configure.
            .min_latency(0)
            .max_latency(-1)
            .build();
        src.set_property("max-buffers", 0u64);
        src.set_property_from_str("leaky-type", "downstream");
        bin.add(&src).context("add lane")?;
        src.static_pad("src")
            .context("lane src pad")?
            .link(&chain.sink().context("lane chain sink")?)
            .context("link lane")?;
        src.sync_state_with_parent().context("start lane")?;
        chain.prepend(src.clone().upcast());
        Ok(Lane {
            src: Some(src),
            chain: Some(chain),
            last: None,
        })
    }

    /// Remove the clip's lanes: the program goes to black and silence.
    fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.want = None;
        self.close_locked(&mut inner);
    }

    fn close_locked(&self, inner: &mut LanesInner) {
        for (lane, inputs) in [
            (std::mem::take(&mut inner.video), inner.video_in.clone()),
            (std::mem::take(&mut inner.audio), inner.audio_in.clone()),
        ] {
            if let (Some(chain), Some(inputs)) = (lane.chain, inputs) {
                inputs.remove(chain);
            }
        }
    }

    /// Push a player's buffer into its lane. `player_base` is the player's
    /// base time while it plays; `None` for a preroll (a cued or paused
    /// frame), which is shown right away.
    fn push(&self, kind: Kind, sample: &gst::Sample, player_base: Option<gst::ClockTime>) {
        let Some(buffer) = sample.buffer_owned() else {
            return;
        };
        let (src, last) = {
            let inner = self.inner.lock().unwrap();
            let lane = match kind {
                Kind::Video => &inner.video,
                Kind::Audio => &inner.audio,
            };
            let Some(src) = lane.src.clone() else { return };
            (src, lane.last)
        };
        let Some(pts) = program_pts(sample, &src, player_base) else {
            return;
        };
        // Never backwards or repeated: after a seek, a preroll or a loop,
        // buffers stamped before the ones already queued are dropped.
        if last.is_some_and(|(start, end)| {
            pts <= start || pts + gst::ClockTime::from_mseconds(1) < end
        }) {
            return;
        }
        let end = last.map(|(_, end)| end);
        let duration = buffer.duration();
        let mut buffer = buffer;
        {
            let buffer = buffer.make_mut();
            buffer.set_pts(pts);
            buffer.set_dts(gst::ClockTime::NONE);
            // A gap (resume, seek): let the mixer resync rather than wait.
            if end.is_some_and(|end| pts > end + gst::ClockTime::from_mseconds(20)) {
                buffer.set_flags(gst::BufferFlags::DISCONT);
            }
        }
        if let Some(caps) = sample.caps() {
            if src.caps().is_none_or(|c| c.as_ref() != caps) {
                src.set_caps(Some(&caps.to_owned()));
            }
        }
        {
            let mut inner = self.inner.lock().unwrap();
            let lane = match kind {
                Kind::Video => &mut inner.video,
                Kind::Audio => &mut inner.audio,
            };
            // Lanes replaced meanwhile.
            if lane.src.as_ref() != Some(&src) {
                return;
            }
            lane.last = Some((pts, pts + duration.unwrap_or(gst::ClockTime::ZERO)));
            #[cfg(test)]
            if let Some(clip) = sample.buffer().and_then(|b| b.pts()).and_then(|pts| {
                sample
                    .segment()?
                    .downcast_ref::<gst::ClockTime>()?
                    .to_stream_time(pts)
            }) {
                inner.pushed.push(Pushed {
                    kind,
                    clip,
                    program: pts,
                    preroll: player_base.is_none(),
                });
            }
        }
        // Fails only while the program is stopping.
        let _ = src.push_buffer(buffer);
    }
}

/// When `sample` is due in the program's running time, plus [`LEAD`].
fn program_pts(
    sample: &gst::Sample,
    src: &gst_app::AppSrc,
    player_base: Option<gst::ClockTime>,
) -> Option<gst::ClockTime> {
    let Some(player_base) = player_base else {
        // A preroll: now. The program isn't running yet without a time.
        return Some(src.current_running_time()? + LEAD);
    };
    let pts = sample.buffer()?.pts()?;
    let segment = sample.segment()?.downcast_ref::<gst::ClockTime>()?;
    let rt = segment.to_running_time(pts)?;
    (player_base + rt + LEAD).checked_sub(src.base_time()?)
}

// ── Playout (transport) ───────────────────────────────────────────────────────

/// A clip to play, as resolved from the media library.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRequest {
    pub media_id: String,
    pub name: String,
    pub path: String,
    pub in_point: gst::ClockTime,
    pub out_point: Option<gst::ClockTime>,
    pub end: ClipEnd,
}

/// What a channel is doing, for the API.
#[derive(Debug, Clone)]
pub struct PlayoutStatus {
    pub state: TransportState,
    pub clip: Option<ClipRequest>,
    /// Position in the file.
    pub position: Option<gst::ClockTime>,
    pub duration: Option<gst::ClockTime>,
    /// Why the last clip stopped, if it failed.
    pub error: Option<String>,
}

/// A channel's transport: loads clips and plays them into its program.
/// Lives as long as the channel's source, across monitor restarts.
pub struct Playout {
    lanes: Arc<Lanes>,
    /// Serializes transport commands.
    clip: tokio::sync::Mutex<Option<Clip>>,
    shared: Arc<Mutex<Shared>>,
}

/// State the player's bus task updates.
struct Shared {
    state: TransportState,
    clip: Option<ClipRequest>,
    error: Option<String>,
    player: Option<glib::WeakRef<gst::Pipeline>>,
    /// The last position read (or sought to): a player can't answer while
    /// it flushes for a seek.
    position: Option<gst::ClockTime>,
}

struct Clip {
    player: gst::Pipeline,
    bus_task: tokio::task::JoinHandle<()>,
    /// Whether the appsinks' buffers are played (not prerolls).
    playing: Arc<AtomicBool>,
    request: ClipRequest,
    has_audio: bool,
}

impl Drop for Clip {
    fn drop(&mut self) {
        self.bus_task.abort();
        let _ = self.player.set_state(gst::State::Null);
    }
}

impl Playout {
    fn new(id: &str) -> Self {
        Self {
            lanes: Arc::new(Lanes::new(id)),
            clip: tokio::sync::Mutex::new(None),
            shared: Arc::new(Mutex::new(Shared {
                state: TransportState::Idle,
                clip: None,
                error: None,
                player: None,
                position: None,
            })),
        }
    }

    pub fn status(&self) -> PlayoutStatus {
        let mut shared = self.shared.lock().unwrap();
        let player = shared.player.as_ref().and_then(|p| p.upgrade());
        if let Some(position) = player
            .as_ref()
            .and_then(|p| p.query_position::<gst::ClockTime>())
        {
            shared.position = Some(position);
        }
        PlayoutStatus {
            state: shared.state,
            clip: shared.clip.clone(),
            position: player.as_ref().and(shared.position),
            duration: player.and_then(|p| p.query_duration::<gst::ClockTime>()),
            error: shared.error.clone(),
        }
    }

    /// Load `request` and cue it: paused at its in point, showing its first
    /// frame. Replaces any clip.
    pub async fn load(&self, request: ClipRequest) -> Result<()> {
        let mut current = self.clip.lock().await;
        if let Some(old) = current.take() {
            drop(old);
            self.lanes.close();
        }
        self.set(TransportState::Idle, None, None);
        match self.cue(request.clone()).await {
            Ok(clip) => {
                *current = Some(clip);
                self.set(TransportState::Cued, Some(request), None);
                Ok(())
            }
            Err(e) => {
                self.lanes.close();
                self.set(TransportState::Idle, None, Some(e.to_string()));
                Err(e)
            }
        }
    }

    async fn cue(&self, request: ClipRequest) -> Result<Clip> {
        let playing = Arc::new(AtomicBool::new(false));
        let (player, streams) = build_player(&request, &self.lanes, &playing)?;
        let bus = player.bus().context("player has no bus")?;
        let mut messages = bus.stream();
        let mut guard = PlayerGuard(Some(player.clone()));

        player
            .set_state(gst::State::Paused)
            .map_err(|_| anyhow!("can't open {}", request.path))?;
        wait_async_done(&mut messages, &player, &request).await?;
        if !streams.video.load(Ordering::SeqCst) {
            bail!("{} has no video", request.path);
        }
        let has_audio = streams.audio.load(Ordering::SeqCst);
        // Lanes before the seek's preroll, so the cued frame shows.
        self.lanes.open(has_audio)?;
        seek(&player, &request, gst::SeekFlags::FLUSH)
            .with_context(|| format!("can't seek {}", request.path))?;
        wait_async_done(&mut messages, &player, &request).await?;

        self.shared.lock().unwrap().player = Some(player.downgrade());
        let bus_task = tokio::spawn(watch_player(
            messages,
            player.downgrade(),
            request.clone(),
            self.lanes.clone(),
            self.shared.clone(),
            playing.clone(),
        ));
        guard.0 = None;
        Ok(Clip {
            player,
            bus_task,
            playing,
            request,
            has_audio,
        })
    }

    pub async fn play(&self) -> Result<()> {
        let current = self.clip.lock().await;
        let clip = current.as_ref().context("no clip loaded")?;
        let state = self.shared.lock().unwrap().state;
        if state == TransportState::Ended {
            // Again from the in point; a black end removed the lanes.
            self.lanes.open(clip.has_audio)?;
            seek(&clip.player, &clip.request, gst::SeekFlags::FLUSH)?;
        }
        clip.playing.store(true, Ordering::SeqCst);
        clip.player
            .set_state(gst::State::Playing)
            .map_err(|_| anyhow!("can't play {}", clip.request.path))?;
        self.shared.lock().unwrap().state = TransportState::Playing;
        Ok(())
    }

    pub async fn pause(&self) -> Result<()> {
        let current = self.clip.lock().await;
        let clip = current.as_ref().context("no clip loaded")?;
        if self.shared.lock().unwrap().state != TransportState::Playing {
            return Ok(());
        }
        clip.playing.store(false, Ordering::SeqCst);
        clip.player
            .set_state(gst::State::Paused)
            .map_err(|_| anyhow!("can't pause {}", clip.request.path))?;
        self.shared.lock().unwrap().state = TransportState::Paused;
        Ok(())
    }

    /// Unload the clip: black and silence.
    pub async fn stop(&self) {
        let mut current = self.clip.lock().await;
        current.take();
        self.lanes.close();
        let mut shared = self.shared.lock().unwrap();
        shared.state = TransportState::Idle;
        shared.clip = None;
        shared.player = None;
    }

    /// Go to `position` in the file, kept within the clip's in and out
    /// points. Paused or cued, the frame there is shown.
    pub async fn seek(&self, position: gst::ClockTime) -> Result<()> {
        let current = self.clip.lock().await;
        let clip = current.as_ref().context("no clip loaded")?;
        let mut request = clip.request.clone();
        let position = position.max(request.in_point);
        request.in_point = match request.out_point {
            Some(out) => position.min(out),
            None => position,
        };
        if self.shared.lock().unwrap().state == TransportState::Ended {
            self.lanes.open(clip.has_audio)?;
            self.shared.lock().unwrap().state = TransportState::Paused;
        }
        seek(&clip.player, &request, gst::SeekFlags::FLUSH)
            .with_context(|| format!("can't seek {}", request.path))?;
        self.shared.lock().unwrap().position = Some(request.in_point);
        Ok(())
    }

    fn set(&self, state: TransportState, clip: Option<ClipRequest>, error: Option<String>) {
        let mut shared = self.shared.lock().unwrap();
        shared.state = state;
        shared.clip = clip;
        shared.error = error;
        if state == TransportState::Idle {
            shared.player = None;
            shared.position = None;
        }
    }
}

/// Sets a player to NULL on drop unless forgotten: a failed cue doesn't
/// leave it running.
struct PlayerGuard(Option<gst::Pipeline>);

impl Drop for PlayerGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = p.set_state(gst::State::Null);
        }
    }
}

/// Wait for a state change to finish, or the player to fail. A looping
/// clip's segment can end meanwhile (the demuxer runs ahead of playback
/// into the decoder's queues, even paused), so it's looped here too.
async fn wait_async_done(
    messages: &mut gst::bus::BusStream,
    player: &gst::Pipeline,
    clip: &ClipRequest,
) -> Result<()> {
    let path = &clip.path;
    let wait = async {
        while let Some(msg) = messages.next().await {
            match msg.view() {
                gst::MessageView::AsyncDone(_) => return Ok(()),
                gst::MessageView::SegmentDone(_) => {
                    seek(player, clip, gst::SeekFlags::empty())
                        .with_context(|| format!("{path}: can't loop"))?;
                }
                gst::MessageView::Error(err) => bail!("{path}: {}", err.error()),
                _ => {}
            }
        }
        bail!("{path}: player stopped")
    };
    tokio::time::timeout(LOAD_TIMEOUT, wait)
        .await
        .map_err(|_| anyhow!("{path}: timed out loading"))?
}

/// A seek to the clip's in point, ending at its out point. A looping clip's
/// seeks are segment seeks: the segment's end comes as `SEGMENT_DONE`, and
/// the next seek continues from it without a gap.
fn seek(player: &gst::Pipeline, clip: &ClipRequest, flags: gst::SeekFlags) -> Result<()> {
    let mut flags = flags | gst::SeekFlags::ACCURATE;
    if clip.end == ClipEnd::Loop {
        flags |= gst::SeekFlags::SEGMENT;
    }
    let (stop_type, stop) = match clip.out_point {
        Some(out) => (gst::SeekType::Set, Some(out)),
        None => (gst::SeekType::None, gst::ClockTime::NONE),
    };
    player.seek(
        1.0,
        flags,
        gst::SeekType::Set,
        Some(clip.in_point),
        stop_type,
        stop,
    )?;
    Ok(())
}

/// Loop the clip, end it at its out point, and stop it on errors. Runs until
/// the clip is unloaded.
async fn watch_player(
    mut messages: gst::bus::BusStream,
    player: glib::WeakRef<gst::Pipeline>,
    clip: ClipRequest,
    lanes: Arc<Lanes>,
    shared: Arc<Mutex<Shared>>,
    playing: Arc<AtomicBool>,
) {
    while let Some(msg) = messages.next().await {
        match msg.view() {
            gst::MessageView::SegmentDone(_) => {
                let Some(p) = player.upgrade() else { return };
                if let Err(e) = seek(&p, &clip, gst::SeekFlags::empty()) {
                    fail(&clip, &format!("can't loop: {e}"), &lanes, &shared, &p);
                    return;
                }
            }
            gst::MessageView::Eos(_) => {
                playing.store(false, Ordering::SeqCst);
                if let Some(p) = player.upgrade() {
                    let _ = p.set_state(gst::State::Paused);
                }
                if clip.end == ClipEnd::Black {
                    lanes.close();
                }
                shared.lock().unwrap().state = TransportState::Ended;
            }
            gst::MessageView::Error(err) => {
                let Some(p) = player.upgrade() else { return };
                fail(&clip, &err.error().to_string(), &lanes, &shared, &p);
                return;
            }
            _ => {}
        }
    }
}

/// A clip failed: stop it and go to black, keeping the error to show.
fn fail(
    clip: &ClipRequest,
    error: &str,
    lanes: &Lanes,
    shared: &Mutex<Shared>,
    player: &gst::Pipeline,
) {
    warn!(path = %clip.path, error = %error, "clip player error");
    let _ = player.set_state(gst::State::Null);
    lanes.close();
    let mut shared = shared.lock().unwrap();
    shared.state = TransportState::Idle;
    shared.clip = None;
    shared.player = None;
    shared.error = Some(format!("{}: {error}", clip.name));
}

// ── Player ────────────────────────────────────────────────────────────────────

/// uridecodebin ─┬→ queue → videoconvert → appsink ⇢ video lane
///               ├→ queue → audioconvert → audioresample → appsink ⇢ audio lane
///               └→ fakesink (any other stream)
///
/// [`Streams`] says which branches the file feeds, once it has prerolled.
fn build_player(
    clip: &ClipRequest,
    lanes: &Arc<Lanes>,
    playing: &Arc<AtomicBool>,
) -> Result<(gst::Pipeline, Arc<Streams>)> {
    let player = gst::Pipeline::with_name(&format!("channel-player-{}", lanes.id));
    // The program runs on the system clock; timestamps map between the two.
    player.use_clock(Some(&gst::SystemClock::obtain()));
    let decode = make_el("uridecodebin", &format!("channel-decode-{}", lanes.id))?;
    decode.set_property(
        "uri",
        glib::filename_to_uri(&clip.path, None)
            .with_context(|| format!("make a URI for {}", clip.path))?
            .to_string(),
    );
    player.add(&decode).context("add uridecodebin")?;

    let video = add_branch(
        &player,
        Kind::Video,
        &["queue", "videoconvert"],
        lanes,
        playing,
    )?;
    let audio = add_branch(
        &player,
        Kind::Audio,
        &["queue", "audioconvert", "audioresample"],
        lanes,
        playing,
    )?;

    let streams = Arc::new(Streams::default());
    let streams_ref = streams.clone();
    let (video, audio) = (video.downgrade(), audio.downgrade());
    let branches = (video.clone(), audio.clone());
    let player_weak = player.downgrade();
    decode.connect_pad_added(move |_, pad| {
        let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
        let kind = caps
            .structure(0)
            .map(|s| s.name().as_str())
            .unwrap_or_default();
        let branch = if kind.starts_with("video/") {
            video.upgrade()
        } else if kind.starts_with("audio/") {
            audio.upgrade()
        } else {
            None
        };
        let sink = match branch
            .and_then(|b| b.static_pad("sink"))
            .filter(|p| !p.is_linked())
        {
            Some(sink) => {
                let found = if kind.starts_with("audio/") {
                    &streams_ref.audio
                } else {
                    &streams_ref.video
                };
                found.store(true, Ordering::SeqCst);
                sink
            }
            // Drain any other stream, so the demuxer doesn't stop on it.
            None => {
                let Some(player) = player_weak.upgrade() else {
                    return;
                };
                let Ok(fake) = gst::ElementFactory::make("fakesink")
                    .property("sync", true)
                    .build()
                else {
                    return;
                };
                if player.add(&fake).is_err() || fake.sync_state_with_parent().is_err() {
                    return;
                }
                let Some(sink) = fake.static_pad("sink") else {
                    return;
                };
                debug!(pad = %pad.name(), kind, "clip stream not used");
                sink
            }
        };
        if let Err(e) = pad.link(&sink) {
            warn!(pad = %pad.name(), error = ?e, "clip pad link failed");
        }
    });

    // A file without audio (or video) leaves that branch unlinked; take it
    // out so the player can preroll without it.
    let player_weak = player.downgrade();
    decode.connect_no_more_pads(move |_| {
        let Some(player) = player_weak.upgrade() else {
            return;
        };
        for branch in [&branches.0, &branches.1] {
            if let Some(first) = branch
                .upgrade()
                .filter(|b| b.static_pad("sink").is_some_and(|p| !p.is_linked()))
            {
                remove_branch(&player, &first);
            }
        }
    });

    Ok((player, streams))
}

/// Which of a player's branches its file feeds.
#[derive(Default)]
struct Streams {
    video: AtomicBool,
    audio: AtomicBool,
}

/// `elements` → appsink, pushing into `kind`'s lane. Returns the first
/// element, for the decoder to link to.
fn add_branch(
    player: &gst::Pipeline,
    kind: Kind,
    elements: &[&str],
    lanes: &Arc<Lanes>,
    playing: &Arc<AtomicBool>,
) -> Result<gst::Element> {
    let tag = match kind {
        Kind::Video => "video",
        Kind::Audio => "audio",
    };
    let id = player.name();
    let id = id.trim_start_matches("channel-player-");
    let mut chain = elements
        .iter()
        .map(|factory| make_el(factory, &format!("{id}-{factory}-{tag}")))
        .collect::<Result<Vec<_>>>()?;
    let sink = gst_app::AppSink::builder()
        .name(format!("{id}-sink-{tag}"))
        .caps(&gst::Caps::new_empty_simple(format!("{tag}/x-raw")))
        // Paced by the clock: this is what plays the clip in real time.
        .sync(true)
        .max_buffers(2)
        .build();
    let player_weak = player.downgrade();
    let (on_sample, on_preroll) = (lanes.clone(), lanes.clone());
    let (playing_s, playing_p) = (playing.clone(), playing.clone());
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                if playing_s.load(Ordering::SeqCst) {
                    let base = player_weak.upgrade().and_then(|p| p.base_time());
                    if base.is_some() {
                        on_sample.push(kind, &sample, base);
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .new_preroll(move |sink| {
                let sample = sink.pull_preroll().map_err(|_| gst::FlowError::Eos)?;
                // Cued or paused: show the frame now. (While playing, a
                // preroll is followed by the same buffer as a sample.)
                if kind == Kind::Video && !playing_p.load(Ordering::SeqCst) {
                    on_preroll.push(kind, &sample, None);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    chain.push(sink.upcast());
    player
        .add_many(&chain)
        .with_context(|| format!("add {tag} branch"))?;
    gst::Element::link_many(&chain).with_context(|| format!("link {tag} branch"))?;
    Ok(chain.swap_remove(0))
}

/// Remove the branch starting at `first` from `player`.
fn remove_branch(player: &gst::Pipeline, first: &gst::Element) {
    let mut el = Some(first.clone());
    while let Some(e) = el {
        el = e
            .static_pad("src")
            .and_then(|p| p.peer())
            .and_then(|p| p.parent_element());
        let _ = e.set_state(gst::State::Null);
        let _ = player.remove(&e);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use super::*;
    use crate::api::types::MonitorSettingsDto;
    use crate::pipeline::monitor::MonitorPipeline;

    const W: u32 = 1920;
    const H: u32 = 1080;

    /// A 5 s 720p30 clip: colour bars and a tone.
    fn make_clip(path: &std::path::Path) {
        let p = gst::parse::launch(&format!(
            "videotestsrc num-buffers=150 pattern=smpte ! video/x-raw,width=1280,height=720,framerate=30/1 \
             ! x264enc key-int-max=30 ! mux. \
             audiotestsrc num-buffers=235 samplesperbuffer=1024 ! audio/x-raw,rate=48000,channels=2 \
             ! avenc_aac ! aacparse ! mux. \
             mp4mux name=mux ! filesink location={}",
            path.display()
        ))
        .unwrap();
        p.set_state(gst::State::Playing).unwrap();
        let bus = p.bus().unwrap();
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(30),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("clip written");
        assert_eq!(msg.type_(), gst::MessageType::Eos, "{msg:?}");
        p.set_state(gst::State::Null).unwrap();
    }

    /// What the program's video producer delivers.
    #[derive(Default)]
    struct Program {
        /// Mean luma of the latest frame.
        luma: Mutex<f64>,
        last: Mutex<Option<Instant>>,
        stalls: Mutex<Vec<Duration>>,
    }

    fn watch_program(mon: &MonitorPipeline) -> Arc<Program> {
        let program = Arc::new(Program::default());
        let p = program.clone();
        mon.video.appsink().static_pad("sink").unwrap().add_probe(
            gst::PadProbeType::BUFFER,
            move |_, info| {
                let now = Instant::now();
                if let Some(prev) = p.last.lock().unwrap().replace(now) {
                    if now - prev > Duration::from_millis(150) {
                        p.stalls.lock().unwrap().push(now - prev);
                    }
                }
                if let Some(gst::PadProbeData::Buffer(b)) = &info.data {
                    let map = b.map_readable().unwrap();
                    let y = &map[..(W * H) as usize];
                    let sum: u64 = y.iter().step_by(97).map(|&v| v as u64).sum();
                    *p.luma.lock().unwrap() = sum as f64 / y.iter().step_by(97).count() as f64;
                }
                gst::PadProbeReturn::Ok
            },
        );
        program
    }

    fn clip(path: &std::path::Path, in_s: f64, out_s: Option<f64>, end: ClipEnd) -> ClipRequest {
        let t = |s: f64| gst::ClockTime::from_nseconds((s * 1e9) as u64);
        ClipRequest {
            media_id: "m".into(),
            name: "clip".into(),
            path: path.to_string_lossy().into(),
            in_point: t(in_s),
            out_point: out_s.map(t),
            end,
        }
    }

    fn take_pushed(playout: &Playout) -> Vec<Pushed> {
        std::mem::take(&mut playout.lanes.inner.lock().unwrap().pushed)
    }

    /// Distinct program − clip offsets of played (not preroll) buffers.
    fn offsets(pushed: &[Pushed], kind: Kind) -> BTreeSet<i128> {
        pushed
            .iter()
            .filter(|p| p.kind == kind && !p.preroll)
            .map(|p| p.program.nseconds() as i128 - p.clip.nseconds() as i128)
            .collect()
    }

    /// The whole transport on a running program: cue, play, pause, resume,
    /// stop, loop and end, checking what the program shows and that the
    /// producers recordings read from never stall.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~15 s); run with --ignored"]
    async fn transport_on_a_running_program() {
        gst::init().unwrap();
        let dir = std::env::temp_dir().join(format!("cr-channel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        make_clip(&file);

        let src = ChannelSource::new(
            "c".into(),
            "c".into(),
            ChannelConfig {
                format: LiveVideoFormat {
                    width: W,
                    height: H,
                    fps_num: 30,
                    fps_den: 1,
                },
                audio_channels: 2,
                outputs: vec![],
            },
        );
        let playout = src.playout().unwrap().clone();
        let mon = MonitorPipeline::new(&src, &MonitorSettingsDto::default()).unwrap();
        let program = watch_program(&mon);
        let luma = || *program.luma.lock().unwrap();
        let pos = || playout.status().position.unwrap().mseconds();
        let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));
        // Black is Y=16; bars average far above.
        let black = |l: f64| (l - 16.0).abs() < 2.0;

        sleep(1000).await;
        assert!(black(luma()), "idle is black: {}", luma());
        assert_eq!(playout.status().state, TransportState::Idle);

        // ── Cue: the first frame shows, and holds ────────────────────────────
        playout
            .load(clip(&file, 0.0, None, ClipEnd::Hold))
            .await
            .unwrap();
        assert_eq!(playout.status().state, TransportState::Cued);
        sleep(500).await;
        assert!(!black(luma()), "cued frame shows: {}", luma());
        assert!(pos() < 40, "cued at the start: {}", pos());

        // ── Play, pause, resume ──────────────────────────────────────────────
        playout.play().await.unwrap();
        sleep(1500).await;
        let p1 = pos();
        assert!((1300..1700).contains(&p1), "plays in real time: {p1}");
        playout.pause().await.unwrap();
        let paused_at = pos();
        sleep(1000).await;
        assert_eq!(playout.status().state, TransportState::Paused);
        assert!(pos().abs_diff(paused_at) < 40, "pause holds position");
        assert!(!black(luma()), "pause holds the frame");
        playout.play().await.unwrap();
        sleep(1000).await;
        let pushed = take_pushed(&playout);
        let (v, a) = (offsets(&pushed, Kind::Video), offsets(&pushed, Kind::Audio));
        println!("video offsets {v:?}\naudio offsets {a:?}");
        assert_eq!(v.len(), 2, "one offset per play stretch: {v:?}");
        assert_eq!(a.len(), 2, "one offset per play stretch: {a:?}");
        // Offsets are in file timestamps, which can differ between streams
        // (an edit list): what must hold is that a pause moves both alike.
        let drift: Vec<i128> = v.iter().zip(&a).map(|(v, a)| v - a).collect();
        assert_eq!(drift[0], drift[1], "A/V drift across the pause");

        // ── Stop: back to black ──────────────────────────────────────────────
        playout.stop().await;
        sleep(500).await;
        assert!(black(luma()), "stop goes to black: {}", luma());
        assert_eq!(playout.status().state, TransportState::Idle);

        // ── Loop 1 s..2 s: accurate and gapless ──────────────────────────────
        playout
            .load(clip(&file, 1.0, Some(2.0), ClipEnd::Loop))
            .await
            .unwrap();
        take_pushed(&playout);
        playout.play().await.unwrap();
        sleep(3500).await;
        let pushed: Vec<_> = take_pushed(&playout)
            .into_iter()
            .filter(|p| p.kind == Kind::Video && !p.preroll)
            .collect();
        let first = pushed.first().unwrap().clip.mseconds();
        assert!(
            (1000..1034).contains(&first),
            "starts at the in point: {first}"
        );
        let loops = pushed.windows(2).filter(|w| w[1].clip < w[0].clip).count();
        assert!(loops >= 2, "looped: {loops}");
        for w in pushed.windows(2) {
            let gap = w[1].program.saturating_sub(w[0].program).mseconds();
            assert!(
                (30..=40).contains(&gap),
                "gap {gap} ms at clip {}",
                w[1].clip
            );
            assert!(
                (1000..2000).contains(&w[1].clip.mseconds()),
                "within in/out: {}",
                w[1].clip
            );
        }

        // ── End to black ─────────────────────────────────────────────────────
        playout
            .load(clip(&file, 4.0, None, ClipEnd::Black))
            .await
            .unwrap();
        playout.play().await.unwrap();
        sleep(2000).await;
        assert_eq!(playout.status().state, TransportState::Ended);
        assert!(black(luma()), "ends in black: {}", luma());

        // Replay from the in point after the end.
        playout.play().await.unwrap();
        sleep(500).await;
        assert_eq!(playout.status().state, TransportState::Playing);
        assert!(!black(luma()), "replays: {}", luma());

        mon.stop().unwrap();
        assert_eq!(*program.stalls.lock().unwrap(), vec![], "program stalls");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
