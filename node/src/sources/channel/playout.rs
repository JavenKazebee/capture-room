//! Playout: a channel's transport.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use gstreamer::{self as gst, glib, prelude::*};
use tracing::warn;

use super::lanes::{Lanes, PlayedProgress};
use super::player::{build_player, Marks};
use super::{LOAD_TIMEOUT, STEP_TIMEOUT};
use crate::api::types::{ClipEnd, TransportState};
use crate::pipeline::replay::ReplaySpan;

/// A clip to play, as resolved from the media library.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRequest {
    pub media_id: String,
    pub name: String,
    pub path: String,
    pub in_point: gst::ClockTime,
    pub out_point: Option<gst::ClockTime>,
    pub end: ClipEnd,
    /// Speed: 1 is normal, negative plays from the out point back to the
    /// in point. Audio is only heard at 1.
    pub rate: f64,
    /// Part of a replay buffer, played instead of `path` (its folder).
    pub replay: Option<ReplayClip>,
}

/// A replay buffer's span, as a clip's source: positions in the clip count
/// from the start of its first file.
#[derive(Debug, Clone)]
pub struct ReplayClip {
    pub source_id: String,
    /// The saved clip it is, if it isn't straight from the buffer.
    pub clip_id: Option<String>,
    pub span: ReplaySpan,
}

impl PartialEq for ReplayClip {
    fn eq(&self, other: &Self) -> bool {
        self.source_id == other.source_id
            && self.clip_id == other.clip_id
            && self.span.files == other.span.files
            && self.span.base == other.span.base
    }
}

/// An entry in a channel's playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistItem {
    pub id: String,
    pub clip: ClipRequest,
}

/// What a channel is doing, for the API.
#[derive(Debug, Clone)]
pub struct PlayoutStatus {
    pub state: TransportState,
    pub clip: Option<ClipRequest>,
    /// The playlist item on air.
    pub item_id: Option<String>,
    /// The item that plays next, and whether it's cued in the background.
    pub next_id: Option<String>,
    pub next_ready: bool,
    /// Position in the file.
    pub position: Option<gst::ClockTime>,
    pub duration: Option<gst::ClockTime>,
    /// Why the last clip stopped, if it failed.
    pub error: Option<String>,
    pub playlist_rev: u64,
}

/// A channel's transport: loads clips and plays them into its program, and
/// runs its playlist. Lives as long as the channel, across monitor restarts
/// and config changes (see [`InputSource::adopt`]).
///
/// The *deck* holds the clip on air and the playlist's next item, cued in
/// the background (in *standby*: prerolled, pushing nothing), so it starts
/// at once: also while the channel is idle, when it's the item Play starts. A clip that ends into the next item hands over to it at the
/// frame after its last one: the next player's base time is set so its
/// first buffer lands there, on the same lanes.
pub struct Playout {
    pub(super) lanes: Arc<Lanes>,
    /// Serializes transport commands.
    deck: tokio::sync::Mutex<Deck>,
    shared: Arc<Mutex<Shared>>,
    playlist: Mutex<Playlist>,
    /// Numbers the clips loaded, so a player's messages find their clip.
    serials: AtomicU64,
}

#[derive(Default)]
struct Deck {
    current: Option<Clip>,
    /// The next item, cued in standby.
    next: Option<Clip>,
    /// The item being cued for `next`, meanwhile.
    preloading: Option<PlaylistItem>,
    /// Where the item on air (or the last one) was in the playlist, for
    /// finding the next one if it's removed while it plays, and what's next
    /// once the channel is idle.
    position: Option<usize>,
}

#[derive(Default)]
struct Playlist {
    items: Vec<PlaylistItem>,
    loop_playlist: bool,
    rev: u64,
    /// Read from the database (by the API) yet.
    loaded: bool,
}

/// State the player's bus task updates.
struct Shared {
    state: TransportState,
    clip: Option<ClipRequest>,
    item_id: Option<String>,
    next_id: Option<String>,
    next_ready: bool,
    error: Option<String>,
    player: Option<glib::WeakRef<gst::Pipeline>>,
    /// Where the player on air is showing.
    marks: Option<Arc<Marks>>,
    /// The last position read (or sought to): a player can't answer while
    /// it flushes for a seek.
    position: Option<gst::ClockTime>,
}

/// What a clip's appsinks do with their buffers.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ClipMode {
    /// Standby: nothing.
    Off,
    /// Prerolls (a cued or paused frame) are shown.
    Cued,
    /// Buffers are played.
    Playing,
}

/// A clip's [`ClipMode`], shared with its appsinks.
pub(super) struct ModeCell(AtomicU8);

impl ModeCell {
    fn new(mode: ClipMode) -> Self {
        Self(AtomicU8::new(mode as u8))
    }

    pub(super) fn load(&self) -> ClipMode {
        match self.0.load(Ordering::SeqCst) {
            0 => ClipMode::Off,
            1 => ClipMode::Cued,
            _ => ClipMode::Playing,
        }
    }

    fn store(&self, mode: ClipMode) {
        self.0.store(mode as u8, Ordering::SeqCst);
    }
}

struct Clip {
    player: gst::Pipeline,
    bus_task: tokio::task::JoinHandle<()>,
    serial: u64,
    mode: Arc<ModeCell>,
    marks: Arc<Marks>,
    request: ClipRequest,
    item: Option<String>,
    has_audio: bool,
    /// The rate the player was last sought at: the request's, or ±1 after
    /// stepping.
    rate: f64,
}

impl Clip {
    fn is(&self, item: &PlaylistItem) -> bool {
        self.item.as_ref() == Some(&item.id) && self.request == item.clip
    }

    /// Set what the appsinks do, then put the player in `state` (playing or
    /// paused).
    fn run(&self, mode: ClipMode, state: gst::State) -> Result<()> {
        self.mode.store(mode);
        let verb = if state == gst::State::Playing {
            "play"
        } else {
            "pause"
        };
        self.player
            .set_state(state)
            .map_err(|_| anyhow!("can't {verb} {}", self.request.path))?;
        Ok(())
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        self.bus_task.abort();
        let _ = self.player.set_state(gst::State::Null);
    }
}

impl Playout {
    pub(super) fn new(id: &str) -> Self {
        Self {
            lanes: Arc::new(Lanes::new(id)),
            deck: tokio::sync::Mutex::default(),
            shared: Arc::new(Mutex::new(Shared {
                state: TransportState::Idle,
                clip: None,
                item_id: None,
                next_id: None,
                next_ready: false,
                error: None,
                player: None,
                marks: None,
                position: None,
            })),
            playlist: Mutex::default(),
            serials: AtomicU64::new(0),
        }
    }

    /// How the clips played so far have reached the program: frames that
    /// arrived, and those too late to show. For the benchmark.
    pub fn played(&self) -> PlayedProgress {
        self.lanes.played()
    }

    pub fn status(&self) -> PlayoutStatus {
        let rev = self.playlist.lock().unwrap().rev;
        let mut shared = self.shared.lock().unwrap();
        let player = shared.player.as_ref().and_then(|p| p.upgrade());
        // The frame showing; a player's own position is off after a step
        // and backwards.
        if let Some(position) = shared
            .marks
            .as_ref()
            .and_then(|m| m.shown())
            .or_else(|| player.as_ref()?.query_position::<gst::ClockTime>())
        {
            shared.position = Some(position);
        }
        PlayoutStatus {
            state: shared.state,
            clip: shared.clip.clone(),
            item_id: shared.item_id.clone(),
            next_id: shared.next_id.clone(),
            next_ready: shared.next_ready,
            position: player.as_ref().and(shared.position),
            duration: player.and_then(|p| p.query_duration::<gst::ClockTime>()),
            error: shared.error.clone(),
            playlist_rev: rev,
        }
    }

    // ── Playlist ──────────────────────────────────────────────────────────────

    /// Whether the playlist has been read from the database.
    pub fn playlist_loaded(&self) -> bool {
        self.playlist.lock().unwrap().loaded
    }

    /// The playlist's items, whether it loops, and its revision.
    pub fn playlist(&self) -> (Vec<PlaylistItem>, bool, u64) {
        let p = self.playlist.lock().unwrap();
        (p.items.clone(), p.loop_playlist, p.rev)
    }

    /// Replace the playlist. The clip on air plays on (as it was cued); the
    /// next item is cued again if it changed.
    pub async fn set_playlist(self: &Arc<Self>, items: Vec<PlaylistItem>, loop_playlist: bool) {
        let mut deck = self.deck.lock().await;
        {
            let mut p = self.playlist.lock().unwrap();
            // What the clip on air does at its out point follows its item,
            // so "stop after this one" works while it plays. Not into or out
            // of a loop, which its seeks set up.
            if let Some(clip) = deck.current.as_mut() {
                let edited = items.iter().find(|i| clip.item.as_ref() == Some(&i.id));
                if let Some(item) = edited {
                    let mut was = item.clip.clone();
                    was.end = clip.request.end;
                    let loops = [was.end, item.clip.end].contains(&ClipEnd::Loop);
                    if was == clip.request && !loops {
                        clip.request.end = item.clip.end;
                        let mut shared = self.shared.lock().unwrap();
                        if let Some(c) = shared.clip.as_mut() {
                            c.end = item.clip.end;
                        }
                    }
                }
            }
            p.items = items;
            p.loop_playlist = loop_playlist;
            p.rev += 1;
            p.loaded = true;
        }
        self.refresh_next(&mut deck);
    }

    /// The item after the one on air, if there's one on air from the
    /// playlist. Idle, the item after the last one played, or the first.
    fn following(&self, deck: &mut Deck) -> Option<PlaylistItem> {
        let p = self.playlist.lock().unwrap();
        let index = match deck.current.as_ref() {
            None => {
                let after = deck.position.map_or(0, |i| i + 1);
                return p.items.get(after).or(p.items.first()).cloned();
            }
            Some(clip) => {
                let id = clip.item.as_ref()?;
                match p.items.iter().position(|item| &item.id == id) {
                    Some(i) => {
                        deck.position = Some(i);
                        i + 1
                    }
                    // Removed: the item that took its place.
                    None => deck.position?,
                }
            }
        };
        p.items
            .get(index)
            .or_else(|| p.loop_playlist.then(|| p.items.first()).flatten())
            .cloned()
    }

    /// Cue the next item in standby, unless it already is.
    fn refresh_next(self: &Arc<Self>, deck: &mut Deck) {
        let want = self.following(deck);
        if deck
            .next
            .as_ref()
            .is_some_and(|c| want.as_ref().is_none_or(|w| !c.is(w)))
        {
            deck.next = None;
        }
        {
            let mut shared = self.shared.lock().unwrap();
            shared.next_id = want.as_ref().map(|w| w.id.clone());
            shared.next_ready = deck.next.is_some();
        }
        let Some(want) = want else { return };
        if deck.next.is_some() || deck.preloading.as_ref() == Some(&want) {
            return;
        }
        deck.preloading = Some(want.clone());
        let this = self.clone();
        tokio::spawn(async move {
            let cued = this.cue(&want.clip, Some(want.id.clone()), false).await;
            let mut deck = this.deck.lock().await;
            if deck.preloading.as_ref() != Some(&want) {
                return;
            }
            deck.preloading = None;
            match cued {
                Ok(clip) => {
                    // Still the one wanted?
                    if this.following(&mut deck) == Some(want) {
                        deck.next = Some(clip);
                        this.shared.lock().unwrap().next_ready = true;
                    } else {
                        drop(deck);
                        this.refresh_next(&mut *this.deck.lock().await);
                    }
                }
                Err(e) => {
                    warn!(path = %want.clip.path, error = %e, "can't cue the next item");
                    this.shared.lock().unwrap().error = Some(format!(
                        "can't cue the next item, {}: {e:#}",
                        want.clip.name
                    ));
                }
            }
        });
    }

    // ── Transport ─────────────────────────────────────────────────────────────

    fn item(&self, item_id: &str) -> Result<PlaylistItem> {
        self.playlist
            .lock()
            .unwrap()
            .items
            .iter()
            .find(|i| i.id == item_id)
            .cloned()
            .context("no such playlist item")
    }

    /// Cue a playlist item: paused at its in point, showing its first frame.
    /// What's on air stays until it's ready.
    pub async fn cue_item(self: &Arc<Self>, item_id: &str) -> Result<()> {
        let item = self.item(item_id)?;
        let mut deck = self.deck.lock().await;
        self.take(&mut deck, item, false).await
    }

    /// Play a playlist item now, following straight on from what's on air.
    pub async fn play_item(self: &Arc<Self>, item_id: &str) -> Result<()> {
        let item = self.item(item_id)?;
        let mut deck = self.deck.lock().await;
        self.take(&mut deck, item, true).await
    }

    /// Load `request` (not from the playlist) and cue it, or play it.
    /// Replaces any clip.
    pub async fn load(self: &Arc<Self>, request: ClipRequest, play: bool) -> Result<()> {
        self.load_clip(request, None).await?;
        if play {
            self.play().await?;
        }
        Ok(())
    }

    async fn load_clip(self: &Arc<Self>, request: ClipRequest, item: Option<String>) -> Result<()> {
        let mut deck = self.deck.lock().await;
        if deck.current.take().is_some() {
            self.lanes.close();
        }
        self.set(TransportState::Idle, None, None, None);
        // The standby clip, if it's this one.
        let standby = deck
            .next
            .take()
            .filter(|c| c.item == item && c.request == request);
        let cued = match standby {
            Some(clip) => self.put_on_air(&mut deck, clip, false),
            None => match self.cue(&request, item.clone(), true).await {
                Ok(clip) => {
                    deck.current = Some(clip);
                    self.set(TransportState::Cued, Some(request), item, None);
                    Ok(())
                }
                Err(e) => Err(e),
            },
        };
        if let Err(e) = cued {
            deck.current = None;
            self.lanes.close();
            self.set(TransportState::Idle, None, None, Some(format!("{e:#}")));
            self.refresh_next(&mut deck);
            return Err(e);
        }
        self.refresh_next(&mut deck);
        Ok(())
    }

    /// Open and preroll a clip at its in point. On air, its lanes are opened
    /// and its first frame shows; otherwise it's in standby.
    async fn cue(
        self: &Arc<Self>,
        request: &ClipRequest,
        item: Option<String>,
        on_air: bool,
    ) -> Result<Clip> {
        let mode = Arc::new(ModeCell::new(if on_air {
            ClipMode::Cued
        } else {
            ClipMode::Off
        }));
        let marks = Arc::new(Marks::default());
        let (player, streams) = build_player(request, &self.lanes, &mode, &marks)?;
        let bus = player.bus().context("player has no bus")?;
        let mut messages = bus.stream();
        let mut guard = PlayerGuard(Some(player.clone()));

        player
            .set_state(gst::State::Paused)
            .map_err(|_| anyhow!("can't open {}", request.path))?;
        wait_async_done(&mut messages, &player, request).await?;
        if !streams.video.load(Ordering::SeqCst) {
            bail!("{} has no video", request.path);
        }
        let has_audio = streams.audio.load(Ordering::SeqCst);
        if on_air {
            // Lanes before the seek's preroll, so the cued frame shows.
            self.lanes.open(has_audio)?;
        }
        seek(&player, request, gst::SeekFlags::FLUSH)
            .with_context(|| format!("can't seek {}", request.path))?;
        wait_async_done(&mut messages, &player, request).await?;

        if on_air {
            let mut shared = self.shared.lock().unwrap();
            shared.player = Some(player.downgrade());
            shared.marks = Some(marks.clone());
        }
        let serial = self.serials.fetch_add(1, Ordering::SeqCst);
        let bus_task = tokio::spawn(watch_player(
            messages,
            player.downgrade(),
            request.clone(),
            Arc::downgrade(self),
            serial,
        ));
        guard.0 = None;
        Ok(Clip {
            player,
            bus_task,
            serial,
            mode,
            marks,
            request: request.clone(),
            item,
            has_audio,
            rate: request.rate,
        })
    }

    /// Put a standby clip on air in place of the current one: playing, from
    /// the frame after the current one's last, or cued.
    fn put_on_air(&self, deck: &mut Deck, clip: Clip, play: bool) -> Result<()> {
        if let Some(old) = &deck.current {
            old.mode.store(ClipMode::Off);
        }
        let playing = self.shared.lock().unwrap().state == TransportState::Playing;
        if play || !playing {
            self.lanes.ensure(clip.has_audio)?;
        } else {
            // Cued over a clip playing: new lanes, or its frame would land
            // behind what's queued and be dropped.
            self.lanes.open(clip.has_audio)?;
        }
        if play {
            self.start(&clip)?;
        } else {
            clip.mode.store(ClipMode::Cued);
            // Preroll again, now that it shows.
            seek(&clip.player, &clip.request, gst::SeekFlags::FLUSH)
                .with_context(|| format!("can't seek {}", clip.request.path))?;
        }
        let state = if play {
            TransportState::Playing
        } else {
            TransportState::Cued
        };
        self.set(state, Some(clip.request.clone()), clip.item.clone(), None);
        {
            let mut shared = self.shared.lock().unwrap();
            shared.player = Some(clip.player.downgrade());
            shared.marks = Some(clip.marks.clone());
        }
        deck.current = Some(clip);
        Ok(())
    }

    /// Play a prerolled clip from its first frame, following straight on
    /// from what the program has queued.
    fn start(&self, clip: &Clip) -> Result<()> {
        let rt = gst::ClockTime::from_nseconds(clip.marks.first_rt.load(Ordering::SeqCst));
        let base = self.lanes.follow_on_base(rt);
        if let Some(base) = base {
            // A fixed base time: the pipeline mustn't pick its own.
            clip.player.set_start_time(gst::ClockTime::NONE);
            clip.player.set_base_time(base);
        }
        clip.run(ClipMode::Playing, gst::State::Playing)?;
        if base.is_some() {
            // Pausing and resuming move the base time again from here.
            clip.player.set_start_time(gst::ClockTime::ZERO);
        }
        Ok(())
    }

    /// Play the clip on air; idle, the next item.
    pub async fn play(self: &Arc<Self>) -> Result<()> {
        let mut deck = self.deck.lock().await;
        let Some(clip) = deck.current.as_mut() else {
            let item = self.following(&mut deck).context("the playlist is empty")?;
            return self.take(&mut deck, item, true).await;
        };
        let mut state = self.shared.lock().unwrap().state;
        if clip.rate != clip.request.rate && state != TransportState::Ended {
            // Stepped: back to the clip's speed, from the frame shown.
            let at = self.position_of(clip);
            clip.rate = clip.request.rate;
            seek_from(&clip.player, &clip.request, at, clip.rate).await?;
            if state == TransportState::Cued {
                state = TransportState::Paused;
            }
        }
        let clip = &*clip;
        match state {
            TransportState::Playing => return Ok(()),
            TransportState::Ended => {
                // Again from the in point; a black end removed the lanes.
                self.lanes.open(clip.has_audio)?;
                seek(&clip.player, &clip.request, gst::SeekFlags::FLUSH)?;
                clip.run(ClipMode::Playing, gst::State::Playing)?;
            }
            TransportState::Cued => self.start(clip)?,
            _ => clip.run(ClipMode::Playing, gst::State::Playing)?,
        }
        self.shared.lock().unwrap().state = TransportState::Playing;
        Ok(())
    }

    pub async fn pause(&self) -> Result<()> {
        let deck = self.deck.lock().await;
        let clip = deck.current.as_ref().context("no clip loaded")?;
        if self.shared.lock().unwrap().state != TransportState::Playing {
            return Ok(());
        }
        clip.run(ClipMode::Cued, gst::State::Paused)?;
        self.shared.lock().unwrap().state = TransportState::Paused;
        Ok(())
    }

    /// Play the next playlist item now.
    pub async fn next(self: &Arc<Self>) -> Result<()> {
        let mut deck = self.deck.lock().await;
        let item = self
            .following(&mut deck)
            .context("nothing next in the playlist")?;
        self.take(&mut deck, item, true).await
    }

    /// Put `item` on air from standby (cueing it first if it isn't there).
    async fn take(self: &Arc<Self>, deck: &mut Deck, item: PlaylistItem, play: bool) -> Result<()> {
        let clip = match deck.next.take().filter(|c| c.is(&item)) {
            Some(clip) => clip,
            None => match self.cue(&item.clip, Some(item.id.clone()), false).await {
                Ok(clip) => clip,
                Err(e) => {
                    self.shared.lock().unwrap().error = Some(format!("{}: {e:#}", item.clip.name));
                    return Err(e);
                }
            },
        };
        let result = self.put_on_air(deck, clip, play);
        if let Err(e) = &result {
            deck.current = None;
            self.lanes.close();
            self.set(TransportState::Idle, None, None, Some(format!("{e:#}")));
        }
        self.refresh_next(deck);
        result
    }

    /// Unload the clip: black and silence.
    pub async fn stop(self: &Arc<Self>) {
        let mut deck = self.deck.lock().await;
        deck.current = None;
        self.lanes.close();
        self.set(TransportState::Idle, None, None, None);
        self.refresh_next(&mut deck);
    }

    /// Go to `position` in the file, kept within the clip's in and out
    /// points. Paused or cued, the frame there is shown.
    pub async fn seek(&self, position: gst::ClockTime) -> Result<()> {
        let mut deck = self.deck.lock().await;
        let clip = deck.current.as_mut().context("no clip loaded")?;
        self.reopen_if_ended(clip)?;
        let at = seek_from(&clip.player, &clip.request, position, clip.rate).await?;
        self.shared.lock().unwrap().position = Some(at);
        Ok(())
    }

    /// Play the clip on air at `rate` from where it is (1 normal, 0.5 half
    /// speed, negative backwards). Only replays change speed.
    pub async fn set_rate(&self, rate: f64) -> Result<()> {
        if !rate.is_finite() || rate == 0.0 || rate.abs() > 4.0 {
            bail!("the speed must be between -4 and 4, and not 0");
        }
        let mut deck = self.deck.lock().await;
        let clip = deck.current.as_mut().context("no clip loaded")?;
        if clip.request.replay.is_none() && rate != 1.0 {
            bail!("only a replay plays at other speeds");
        }
        let at = self.position_of(clip);
        self.reopen_if_ended(clip)?;
        clip.request.rate = rate;
        clip.rate = rate;
        seek_from(&clip.player, &clip.request, at, rate).await?;
        let mut shared = self.shared.lock().unwrap();
        if let Some(c) = shared.clip.as_mut() {
            c.rate = rate;
        }
        Ok(())
    }

    /// Show the frame `frames` away (negative: earlier), pausing first.
    pub async fn step(&self, frames: i32) -> Result<()> {
        if frames == 0 {
            return Ok(());
        }
        let mut deck = self.deck.lock().await;
        let clip = deck.current.as_mut().context("no clip loaded")?;
        let state = self.shared.lock().unwrap().state;
        if state == TransportState::Playing {
            clip.run(ClipMode::Cued, gst::State::Paused)?;
        }
        self.reopen_if_ended(clip)?;
        // A step goes the way the player plays: turn it round first.
        let forward = frames > 0;
        if (clip.rate > 0.0) != forward {
            let at = self.position_of(clip);
            clip.rate = if forward { 1.0 } else { -1.0 };
            seek_from(&clip.player, &clip.request, at, clip.rate).await?;
        }
        let before = clip.marks.shown();
        clip.player.send_event(gst::event::Step::new(
            gst::format::Buffers::from_u64(u64::from(frames.unsigned_abs())),
            1.0,
            true,
            false,
        ));
        // Done once the frame stepped to shows (a step leaves the player's
        // state alone, so there's no state change to wait for). None comes
        // at either end of the clip.
        let until = tokio::time::Instant::now() + STEP_TIMEOUT;
        while clip.marks.shown() == before && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let mut shared = self.shared.lock().unwrap();
        shared.state = TransportState::Paused;
        shared.position = None;
        Ok(())
    }

    /// Where the clip on air is, as last known.
    fn position_of(&self, clip: &Clip) -> gst::ClockTime {
        clip.marks
            .shown()
            .or_else(|| clip.player.query_position::<gst::ClockTime>())
            .or(self.shared.lock().unwrap().position)
            .unwrap_or(clip.request.in_point)
    }

    /// An ended clip is shown paused again, its lanes back if it went to
    /// black.
    fn reopen_if_ended(&self, clip: &Clip) -> Result<()> {
        let mut shared = self.shared.lock().unwrap();
        if shared.state == TransportState::Ended {
            self.lanes.open(clip.has_audio)?;
            clip.mode.store(ClipMode::Cued);
            shared.state = TransportState::Paused;
        }
        Ok(())
    }

    /// A clip's player reached its out point.
    async fn ended(self: Arc<Self>, serial: u64) {
        let mut deck = self.deck.lock().await;
        let Some(end) = deck
            .current
            .as_ref()
            .filter(|c| c.serial == serial)
            .map(|c| c.request.end)
        else {
            return;
        };
        if end == ClipEnd::Next {
            if let Some(item) = self.following(&mut deck) {
                if self.take(&mut deck, item, true).await.is_ok() {
                    return;
                }
            }
        }
        // Hold (or go to black) on the last frame; also when the next item
        // couldn't be taken.
        let Some(clip) = deck.current.as_ref().filter(|c| c.serial == serial) else {
            return;
        };
        let _ = clip.run(ClipMode::Cued, gst::State::Paused);
        if end == ClipEnd::Black {
            self.lanes.close();
        }
        self.shared.lock().unwrap().state = TransportState::Ended;
    }

    /// A clip's player failed: on air, stop it and go to black, keeping the
    /// error to show; in standby, drop it.
    async fn failed(self: Arc<Self>, serial: u64, error: String) {
        let mut deck = self.deck.lock().await;
        if let Some(clip) = deck.next.take_if(|c| c.serial == serial) {
            warn!(path = %clip.request.path, error = %error, "next item's player error");
            let mut shared = self.shared.lock().unwrap();
            shared.next_ready = false;
            shared.error = Some(format!(
                "can't cue the next item, {}: {error}",
                clip.request.name
            ));
            return;
        }
        let Some(clip) = deck.current.take_if(|c| c.serial == serial) else {
            return;
        };
        warn!(path = %clip.request.path, error = %error, "clip player error");
        let message = format!("{}: {error}", clip.request.name);
        drop(clip);
        self.lanes.close();
        self.set(TransportState::Idle, None, None, Some(message));
        self.refresh_next(&mut deck);
    }

    fn set(
        &self,
        state: TransportState,
        clip: Option<ClipRequest>,
        item: Option<String>,
        error: Option<String>,
    ) {
        let mut shared = self.shared.lock().unwrap();
        shared.state = state;
        shared.clip = clip;
        shared.item_id = item;
        shared.error = error;
        shared.position = None;
        if state == TransportState::Idle {
            shared.player = None;
            shared.marks = None;
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
/// At a negative rate, it plays from the out point back to the in point.
fn seek(player: &gst::Pipeline, clip: &ClipRequest, flags: gst::SeekFlags) -> Result<()> {
    seek_span(
        player,
        clip,
        clip.in_point,
        clip.out_point,
        clip.rate,
        flags,
    )
}

fn seek_span(
    player: &gst::Pipeline,
    clip: &ClipRequest,
    start: gst::ClockTime,
    stop: Option<gst::ClockTime>,
    rate: f64,
    flags: gst::SeekFlags,
) -> Result<()> {
    let mut flags = flags | gst::SeekFlags::ACCURATE;
    if clip.end == ClipEnd::Loop {
        flags |= gst::SeekFlags::SEGMENT;
    }
    let (stop_type, stop) = match stop {
        Some(out) => (gst::SeekType::Set, Some(out)),
        None => (gst::SeekType::None, gst::ClockTime::NONE),
    };
    player.seek(
        rate,
        flags,
        gst::SeekType::Set,
        Some(start),
        stop_type,
        stop,
    )?;
    Ok(())
}

/// Go to `position` (kept within the clip) at `rate`, going on from there
/// towards the out point, or the in point backwards; wait for the frame
/// there. Returns the position.
async fn seek_from(
    player: &gst::Pipeline,
    clip: &ClipRequest,
    position: gst::ClockTime,
    rate: f64,
) -> Result<gst::ClockTime> {
    let mut at = position.max(clip.in_point);
    if let Some(out) = clip.out_point {
        at = at.min(out);
    }
    let (start, stop) = if rate > 0.0 {
        (at, clip.out_point)
    } else {
        // Backwards from the frame at `at`, which starts there: a stop at
        // `at` would leave it out.
        let stop = at + gst::ClockTime::from_nseconds(1);
        (
            clip.in_point,
            Some(clip.out_point.map_or(stop, |out| stop.min(out))),
        )
    };
    seek_span(player, clip, start, stop, rate, gst::SeekFlags::FLUSH)
        .with_context(|| format!("can't seek {}", clip.path))?;
    settle(player).await?;
    Ok(at)
}

/// Wait for a player's state change (a seek's preroll) to finish.
async fn settle(player: &gst::Pipeline) -> Result<()> {
    let p = player.clone();
    let (result, _, _) =
        tokio::task::spawn_blocking(move || p.state(gst::ClockTime::from_seconds(5))).await?;
    result.map_err(|_| anyhow!("the player failed"))?;
    Ok(())
}

// Plain functions, not inline spawns: ending a clip can cue the next one,
// which starts its own watcher, and an async fn can't spawn itself.
fn spawn_ended(playout: Arc<Playout>, serial: u64) {
    tokio::spawn(playout.ended(serial));
}

fn spawn_failed(playout: Arc<Playout>, serial: u64, error: String) {
    tokio::spawn(playout.failed(serial, error));
}

/// Loop the clip, and tell the transport when it ends or fails. Runs until
/// the clip is unloaded.
async fn watch_player(
    mut messages: gst::bus::BusStream,
    player: glib::WeakRef<gst::Pipeline>,
    clip: ClipRequest,
    playout: std::sync::Weak<Playout>,
    serial: u64,
) {
    while let Some(msg) = messages.next().await {
        // Handled in their own tasks: dropping the clip aborts this one.
        match msg.view() {
            gst::MessageView::SegmentDone(_) => {
                let Some(p) = player.upgrade() else { return };
                if let Err(e) = seek(&p, &clip, gst::SeekFlags::empty()) {
                    if let Some(playout) = playout.upgrade() {
                        spawn_failed(playout, serial, format!("can't loop: {e}"));
                    }
                    return;
                }
            }
            gst::MessageView::Eos(_) => {
                let Some(playout) = playout.upgrade() else {
                    return;
                };
                spawn_ended(playout, serial);
            }
            gst::MessageView::Error(err) => {
                if let Some(playout) = playout.upgrade() {
                    spawn_failed(playout, serial, err.error().to_string());
                }
                return;
            }
            _ => {}
        }
    }
}
