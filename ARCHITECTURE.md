# Capture Room — Architecture

A multi-feed video capture and recording platform for live broadcast environments, built to run on a single machine or a cluster of up to ~20 nodes managed from a single web UI.

---

## Design Principles

- **One binary, every instance is a node.** A node exposes everything its own machine has: sources, storage volumes, recordings, thumbnails, monitor settings. It knows nothing about other nodes.
- **Controller is a toggle, not a mode.** Any node can be promoted to controller (live, no restart). A controller keeps every node capability and additionally discovers other nodes, forwards commands to them, and merges their event streams.
- **Controllers send plain commands.** Settings changes and start/stop. A start command carries the full output settings inline, so nodes keep no preset store and nothing needs syncing.
- **One addressing scheme.** The UI always talks to `/api/v1/nodes/{node_id}/…`; on a non-controller that list contains only the node itself.
- **Input sources are pluggable from day one.** NDI and Decklink are the first implementations of a formal trait; adding a new source type is additive, not a refactor.
- **Outputs mirror inputs.** Playback is capture in reverse: an `OutputSink` trait mirrors `InputSource`, and each output type (NDI, SDI, SRT, …) is the counterpart of a source type. Adding one is additive too.
- **Types flow from Rust outward.** API types are defined once as Rust structs and exported to TypeScript via `ts-rs`.

## Non-goals

Recording, playback and replay pull toward neighbouring products. Capture Room is not:

- a vision mixer / switcher (no transitions or keying between live sources)
- a graphics system (it can capture or play out graphics, not author them)
- a media asset manager or editor (it hands files to one; it doesn't catalog or cut them)
- an automation system (the scheduler and playlists run Capture Room's own channels only)

---

## System Components

```
Browser (Vue 3 UI)
      │  HTTP / WebSocket (:7700)
      ▼
┌──────────────────────────────────────────────┐
│  capture-room (controller enabled)           │
│  ─ Node API  /api/v1/node/…   (local only)   │
│  ─ /api/v1/nodes/{id}/…  → in-process (self) │
│                          → HTTP (peers)      │
│  ─ Node registry, mDNS browse, health polls  │
│  ─ Presets                                   │
│  ─ /ws: own events + relayed peer events     │
└──────────────┬───────────────────────────────┘
               │  HTTP /api/v1/node/… + WS /api/v1/node/ws
               ▼
┌─────────────────────────┐     ┌─────────────────────────┐
│  capture-room (node)     │     │  capture-room (node)     │  ...
│  ─ Sources + pipelines   │     │  ─ Sources + pipelines   │
│  ─ Recording sessions    │     │  ─ Recording sessions    │
│  ─ Storage volumes       │     │  ─ Storage volumes       │
│  ─ Thumbnails / meters   │     │  ─ Thumbnails / meters   │
│  ─ Local SQLite          │     │  ─ Local SQLite          │
└─────────────────────────┘     └─────────────────────────┘
```

Every instance serves the UI. Pointed at a plain node, the UI shows that node; pointed at a controller, it shows every node the controller knows.

---

## The Rust Binary

**Language:** Rust  
**HTTP/WS:** Axum + `tower-http`  
**Media pipeline:** GStreamer via `gstreamer-rs`  
**Database:** SQLite via `sqlx`  
**UI embedding:** `rust-embed` (release builds) / served from `ui/dist/` (debug builds)  
**Type export:** `ts-rs` — derives TypeScript types from Rust structs  

```
capture-room                  # node
capture-room --controller     # node + controller (persisted; later runs don't need the flag)
```

The controller can also be toggled at runtime with `PUT /api/v1/controller`. The setting is stored as `controller_enabled` in `node_config`.

On first run with no config, generates a UUID and starts mDNS announcement.

---

## Input Plugin System

Every input source implements a single trait. Adding a new source type (RTSP, SRT, HDMI capture card, etc.) means writing a new struct — no changes to the pipeline or recording layer.

```rust
pub trait InputSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    /// Format, if known up front (NDI negotiates at runtime → None).
    fn capabilities(&self) -> Option<SourceCapabilitiesDto>;
    /// Identifies the config the bin was built from; a rescan rebuilds the
    /// source (and restarts its monitor) only when this changes or the
    /// monitor has failed or never started.
    fn fingerprint(&self) -> String;
    fn gst_src_element(&self) -> gst::Element;
    fn timecode(&self) -> Option<String>; // "HH:MM:SS:FF"
}
```

Initial implementations:
- `TestSource` — ✅ implemented — `videotestsrc` + `audiotestsrc`, the reference pattern for all sources (`gst::Bin` with `"video"` / `"audio"` ghost pads)
- `NdiSource` — ✅ implemented — built on the `gst-plugin-ndi` GStreamer elements (`ndisrc` + `ndisrcdemux`), not the raw NDI SDK FFI. Discovery via a persistent `GstDeviceMonitor`. Follows the same bin/ghost-pad contract as `TestSource`. A node connects to every sender it finds, or, set to `chosen` (`PUT /ndi`; Setup › Sources › NDI senders), only to the chosen ones by NDI name (`ndi_subscriptions`) plus any a recording or replay buffer uses, so a node that only needs some feeds doesn't pull them all. It never connects to its own NDI outputs (`HOST (name)`, matched to the machine's host name and its channels' NDI names). It receives as `capture-room (<node name>)`, so senders and NDI Access Manager can tell nodes apart.
- `FileSource` — ✅ implemented — a media file on the node played in a loop as a live feed (real footage for the benchmark, demos without hardware). The file plays in its own *player* pipeline (`uridecodebin` → appsinks, synced to the clock) that loops with segment seeks, so loops are gapless unless the file's streams differ in length (the first frame after such a gap is marked `DISCONT`, so frame counters can tell a loop from a feed falling behind). Its appsinks push into live `appsrc`s in the source bin, which restamp every buffer with the monitor's running time, so the monitor sees one continuous live feed. The player starts when the monitor first wants data and stops when the bin is disposed; its errors are posted on the monitor. A file without audio plays stereo silence; one without video is refused. The file is probed (`Discoverer`) when the source is saved, which rejects anything undecodable and gives the source its capabilities up front.

- `StreamSource` — ✅ implemented — one type for every network stream, chosen by URL scheme: RTSP, SRT (caller, or listener for OBS-style pushes), RTMP pull, HLS/HTTP and UDP MPEG-TS. `uridecodebin3` builds the protocol element and decodes; the source sets that element's properties (jitter buffer `latency_ms`, RTSP transport). Saving checks the scheme is available on the node and that no other configured source binds the same listening port.
- `DeviceSource` — ✅ implemented — a local capture device (HDMI/USB capture card, webcam), configured from a picker rather than auto-discovered. A persistent `GstDeviceMonitor` (`Video/Source`, `Audio/Source`, `Source/Monitor`), like `NdiMonitor`, serves `/devices`; each platform's provider makes the element (`v4l2src` / `pulsesrc` on Linux, `avfvideosrc` on macOS, `mfvideosrc` / `wasapi2src` on Windows), so the only platform code is the stable device key (v4l2 path, AVFoundation unique id, Windows device path, display name as a last resort). Devices without raw video in the requested format are captured as MJPEG and decoded. On Linux, PipeWire's devices are skipped: `pipewiresrc` fails to open its target on some systems (PipeWire 1.6 / GStreamer 1.28), and the v4l2 and PulseAudio providers list the same devices. ALSA's raw devices are skipped too (they'd fight the sound server for the card). An unplugged device keeps the source, shown by its saved name.
- `WhipSource` — ✅ implemented — WebRTC ingest from OBS 30+ and browsers: `whipserversrc` (gst-plugins-rs, statically linked) serves `http://<node>:<port>/whip/endpoint`. Black and silence until a publisher connects. Each publisher session gets fresh input chains, and the element restarts when its pads go (it won't serve a second session's media otherwise). Ports are unique across WHIP and stream listeners.
- `DecklinkSource` — 🟡 built, untested on hardware — an SDI (or HDMI) input on a Blackmagic Decklink card through gst-plugins-bad's `decklinkvideosrc` + `decklinkaudiosrc` (the Desktop Video driver supplies `DeckLinkAPI`; the macOS bundle already ships the plugin). Sub-devices come from the same device monitor as `DeviceSource` (Decklink's are kept out of `/devices` and served by `GET /decklink`, grouped by card), keyed by the driver's `persistent-id` so a source finds the same connector after a restart. Video and embedded audio (2, 8 or 16 channels) are in one bin, as the card requires. Mode is detected from the signal by default, or pinned. It goes through `live::build_bin` as a direct source, never restarted: the card keeps sending frames without a signal, flagged as gaps, which are dropped so the link reads Connecting / Reconnecting and the compositor shows black. Each frame's RP188 timecode is shown on the source (not yet written to recordings: the compositor drops the meta). A card's **profile** (how its connectors split into sub-devices, half or full duplex) is a node setting per card (`PUT /decklink/{card}/profile`, `node_config` key `decklink_profiles`), set on every video element opened on the card; it's in the source's fingerprint, so a change rebuilds the card's sources, and the card's outputs are restarted. Saving checks a sub-device is captured and played out at most once each, and not both on a half-duplex profile.
- Linux screen capture — ⬜ not started — X11 `ximagesrc`, or a Wayland xdg-desktop-portal ScreenCast session into `pipewiresrc`, offered as entries in the device list. On macOS and Windows screens already appear as devices.

**Configured vs discovered sources.** Test, file, stream, device and WHIP sources are *configured*: stored in `configured_sources` as a JSON config tagged with its `type` (`SourceConfig` in `api/types.rs`), and served from `/configured-sources`. A new configured type adds a `SourceConfig` variant and a case in `sources::configured`. NDI sources are *discovered* and not stored.

**Live sources (`sources/live.rs`).** Stream, device and WHIP sources share one bin built around `fallbacksrc` (gst-plugins-rs `fallbackswitch`, statically linked). A dropout never stops the monitor or its recordings: the feed plays black and silence while the source retries forever, and recordings keep going (NDI keeps stop-on-failure).

```
source → [fallbacksrc] → input chain ─┐
            black (direct sources only) ┴→ compositor → locked caps → video
source → [fallbacksrc] → input chain ─┐
                        silence (always) ┴→ audiomixer → 48 kHz F32 → audio
```

- **Fixed output format.** Fallback frames have their own format (320x240, 44.1 kHz mono), so video is deinterlaced and locked to the source's first live size, rate, pixel format and colorimetry (or a configured `format`), and audio to 48 kHz at a fixed channel count. A recording never sees the format change.
- **Clocked aggregators.** The compositor and mixer run on the clock: when the source is late or gone they repeat the last frame or play silence instead of leaving a gap, so a recording's 500 ms input queue never gets a catch-up burst to drop.
- **Fixed latency.** `fallbacksrc`'s `min-latency` is set; otherwise latency changes on every switch and the re-queries stall the pipeline.
- **Direct mode for WHIP.** `fallbacksrc`'s custom-source wrapper aborts the process on assertion failures with `webrtcsrc`, so WHIP's element sits in the bin directly, with its own black source under the compositor. The gst-plugins-rs WebRTC elements block on their own tokio runtime during state changes, so monitor state changes run on a non-tokio thread.
- **Link state.** A pad probe counts real frames; `LinkState` (`live`, `connecting`, `waiting` for a listener or WHIP publisher, `reconnecting`) is on `SourceDto` and the 1 Hz `feed.status` event, and the UI shows it on Sources rows and Record tiles (reconnecting is a warning, not tally red).

**Audio pairing.** A live source's config carries an `AudioPlan`: `source` (its own audio, mixed to N channels; silence if it has none), `device` (an audio device on the node such as an interface or Dante Virtual Soundcard, with 1-based input channels in output order), or `silence`. A device plan adds a second `fallbacksrc` around the audio device. Audio-only devices are never sources by themselves. Monitor pipelines are pinned to the node clock (see Clock sync), so an audio device inside a bin never becomes the clock that recordings slave to.

**Element availability per type.** `plugins::REQUIRED` (core encoders and the like) still fails startup. Stream, device and WHIP list the elements they need instead: a missing one makes that type (or stream protocol) unavailable without stopping the node. `NodeStatus.source_types` reports `{ source_type, missing, protocols }`; the UI greys out unavailable types and saving one returns 400.

---

## GStreamer Pipeline

One always-on **monitor pipeline** per connected source (thumbnail, audio meter), plus
one **recording pipeline per output leg** while recording. The monitor hands raw video
and audio to recordings through `StreamProducer`s (`gstreamer-utils`: an `appsink` that
fans buffers out to consumer `appsrc`s in other pipelines).

```
Monitor pipeline (pipeline/monitor.rs)
[InputSource gst src element]
    ├─► [vtee]
    │      ├─► [queue] → [videorate] → [caps fps] → [videoscale] → [caps res]
    │      │             → [videoconvert] → [jpegenc] → ThumbnailStore → GET /api/v1/node/thumbnails/{id}
    │      └─► [queue] → [appsink] = video StreamProducer
    └─► [atee]
           ├─► [queue] → [audioconvert] → [level] → AudioMeter → WS audio.levels
           └─► [queue] → [appsink] = audio StreamProducer

Recording pipeline, one per output leg (pipeline/recording.rs)
[appsrc video] → [deinterlace]? → [videorate → caps]? → [videoscale → caps]? → [videoconvert]
               → [caps input format]? → [encoder] → [caps prores variant]? → [h264parse|h265parse]? ─┐
[appsrc audio] → [queue 10 s] → [audioconvert (mix-matrix)?] → [audioresample] → [caps channels/S24LE]?
               → [capssetter]? → [aac|opus encoder]? ─┴─► [muxer] → [filesink]
```

Why separate pipelines: a tee passes a branch's flow error back to the source, so a leg
that failed (a codec the container rejects at runtime, a full disk) used to stop the
whole monitor and every other leg. A producer only logs consumer errors, so a failing leg
now fails alone while the session's other legs keep recording.

- **Backpressure:** each consumer `appsrc` holds up to 500 ms and drops the oldest
  buffers when full, so a slow encoder drops frames in its own leg only. The 10 s audio
  queue lets the muxer wait for the video encoder's first frames without dropping audio.
- **Timing:** recording pipelines share the monitor's clock and base time, and the
  producer forwards segments, so audio and video stay aligned. `matroskamux` gets
  `offset-to-zero` (qtmux/mp4mux start at zero on their own).
- **Start:** every leg is built (NULL, no file opened) before any is started, so config
  errors touch no files; if a leg fails to start, the attempt's legs are rolled back and
  their partial files deleted.
- **Stop:** disconnect from the producers, `end_of_stream()` on both appsrcs, wait for
  the leg's own EOS (or error) with a timeout, then NULL. An empty file is removed.
- **Leg failure:** a leg reports its first error the moment it happens (a bus sync
  handler, so the error still reaches `stop`). The error is added to the session's
  `error_message` and sent as `recording.leg_failed`; the session stays active on its
  remaining legs. Once every leg has failed nothing is being recorded, so the session
  is stopped and ends as `error`.
- **Source failure:** a monitor that posts an error (e.g. an NDI sender dropped out)
  shows it on the source. A background check every 5 s rescans while any source lacks
  a healthy monitor (failed, or failed to start): the source is rebuilt with a fresh monitor (or removed, if it has left the
  network), and its recordings are stopped with `source failed: …` so their files are
  finalized. Recording does not restart automatically.
- **Chroma:** H.264/H.265 legs encode the preset's chroma subsampling (default 4:2:0),
  VP9 is pinned to 4:2:0 (Profile 0), and VideoToolbox ProRes gets 4:2:2 (4:4:4 for
  4444); otherwise the encoder would follow the source's format (e.g. 4:4:4 from test
  patterns).
- **Encoder choice:** each codec lists encoders best first (`RecordingProfile::encoders`):
  VideoToolbox (`vtenc_h264`, `vtenc_h265`, `vtenc_prores`) where the platform has it,
  then software (`x264enc`, `x265enc`, `avenc_prores_ks`). A leg is built with the first
  that builds; the choice is logged. VideoToolbox H.264/H.265 take 4:2:0 only, so 4:2:2
  and 4:4:4 go straight to software. The plain `vtenc_*` elements fall back to
  VideoToolbox's own software encoder when the hardware is busy.
- **Encoder defaults:** average bitrate (x264's capped ABR with a 1 s buffer); a blank
  bitrate is Auto, scaled by pixels per second (≈12 Mbps for H.264 at 1080p30, 60% of
  that for H.265/VP9) from the output's format, else the source's negotiated format.
  A keyframe every 2 s. x264/x265 at `speed-preset=veryfast`, no `tune` (x265's GStreamer
  default `ssim` is cleared); VP9 realtime (`deadline=1`, `cpu-used=8`, `row-mt`).
- **Audio:** 24-bit PCM beside ProRes and uncompressed video; AAC 256 kbps in MOV/MP4;
  Opus 160 kbps in MKV. PCM goes through `audiorate` (from the first sample), as replay's
  does: qtmux fails a PCM stream that jumps ("timestamps are drifting"), so a looping
  file source stopped every PCM leg at its first loop. AAC and Opus keep the gap in their
  timestamps (an AAC MOV past three loops stayed in sync), so they go without.
- **Crash safety:** a crash or power cut leaves every file playable, without a recovery
  step. MOV/MP4 with compressed audio are written as 2 s fragments
  (`fragment-mode=first-moov-then-finalise`) and rewritten as an ordinary file when the
  leg finishes; a crash loses at most the fragment being written. PCM fragments can't be
  read back (tested with ffmpeg and qtdemux), so MOV with PCM reserves the index at the
  front instead (`reserved-max-duration` = the longest a file runs, rewritten every 10 s),
  costing two copies of 550 B/s per track (~32 MB for 4 h). If that space fills, qtmux
  stops the leg with an error rather than falling back (tested), so these legs always
  record through `splitmuxsink` and start a new file at `PCM_MAX_FILE` (4 h) or their
  split time, whichever is shorter; a leg that only rolls over keeps a plain first file
  name and numbers the rest (`name_002.mov`…). 550 B/s is a worst case: 1080p30 ProRes +
  PCM used about an eighth. splitmuxsink's `use-robust-muxing` was tried as the safety
  net instead and didn't work (it split every second, or still overflowed). MKV is
  readable as is.
  `moov-recovery-file` + `qtmoovrecover` was tried first and rejected: files it rebuilt
  from H.264 + AAC didn't decode.
- **Splitting:** an output with a split time and/or size records through `splitmuxsink`
  (given the leg's configured muxer), which starts each file at a keyframe — on a
  keyframe request at the split time, or the next 2 s keyframe for a size limit. Files are
  named from the path template with `{segment}` (001, 002…), added before the extension
  when the template doesn't place it. Each new file is reported (`OnLegFile`), saved to
  the session's `files` straight away so the list survives a crash, and sent live in
  `recording.stats`. `vtenc_prores` marks every frame a delta unit, so its output has the
  flag cleared: otherwise the keyframe table lists almost no frames and splitmuxsink
  never finds one to split at.
- **Interlacing:** non-ProRes legs pass through `deinterlace` (`mode=auto`: progressive
  video passes untouched). ProRes keeps interlacing, which it stores natively.
- **Advanced settings:** each output carries `OutputAdvanced` (stored as JSON in
  `preset_outputs.advanced`; every field defaults to the behaviour above): encoder
  choice (Auto / Hardware / Software, filtering the candidates), rate control (average,
  constant — x264 `nal-hrd=cbr`, x265 `strict-cbr`, VP9/VideoToolbox `cbr` — or
  constant quality 1–100, mapped to x264/x265 CRF 41→11, VP9 `cq-level` 63→13 and
  VideoToolbox `quality`), x264/x265 speed preset, keyframe seconds, deinterlace,
  audio codec (Auto / PCM / AAC / Opus) and bitrate, and channels (all, stereo mix, or
  picked source channels via `audioconvert`'s `mix-matrix`; plain numbered channels mix
  odd-left/even-right). Impossible combinations are rejected at save
  (`RecordingProfile::check_advanced`); the editor mirrors the checks in `lib/advanced.ts`.
  VideoToolbox's average bitrate runs about 1.5× over on grainy footage (measured);
  its CBR holds the rate.
- **Multichannel PCM in MOV:** `qtmux` takes PCM beyond stereo only without a channel
  layout, and `audioconvert` won't drop one, so such legs relabel the channels with
  `capssetter` (same samples); otherwise they'd be silently downmixed to stereo.
- **Node encoders:** `/status` and `NodeDto` list the encoder elements a node has
  (`available_encoders`), so the editor can say what Auto resolves to on each node.
- **Dropped frames:** each leg's appsrc counts what it drops; active sessions report the
  counts once a second (`recording.stats`) and the final counts are stored with the
  session (`dropped_frames`, ordered like `output_paths`).

Legs with identical profiles currently each run their own encoder.

Supported encoder targets:
- **Ingest:** ProRes (4444, 422 HQ, 422, LT, Proxy), uncompressed
- **Delivery/proxy:** H.264, H.265/HEVC, VP9
- **Containers:** MOV, MP4, MKV. VP9 records to MKV only; ProRes and uncompressed to MOV
  or MKV (`incompatible()` in `pipeline/profile.rs`; the preset editor offers the same
  choices). MXF is not supported yet: `mxfmux` rejected every codec as wired.

Codecs and containers are closed enums (`VideoCodec`, `Container` in `api/types.rs`), so the API rejects anything else and the UI's choices are generated from Rust. A redundant copy is just another leg with the same profile and a different path.

---

## Thumbnails

The monitor pipeline's thumbnail branch generates JPEG frames at a configurable rate (default **1 fps**, 1–10) regardless of source framerate. Rate and size are node-wide monitor settings (`PUT /api/v1/node/settings`, edited on the Settings page), applied live without restarting pipelines, and have no effect on encoded output.

The latest JPEG is held in memory and served from `GET /api/v1/node/thumbnails/{source_id}`. The periodic emitter sends `thumbnail.updated` at the configured rate and the UI re-fetches the image. The 10 fps ceiling comes from the emitter's 100 ms tick.

### Planned: subscription WebSocket

Request-per-frame doesn't scale past ~10–15 fps with more than a handful of feeds:
browsers allow 6 HTTP/1.1 connections per host, and every peer frame is a round trip
through the controller (which forwards and buffers it). The replacement is a dedicated
thumbnail WebSocket:

- The client sends a subscription: which sources are on screen and at what fps. Frames
  are pushed as binary messages (source key + JPEG), driven by the appsink callback
  rather than the emitter tick. Unsubscribed or off-screen feeds cost nothing.
- On a controller, subscriptions are relayed to each peer for only the sources someone
  is watching; peer frames are forwarded as they arrive.
- Makes ~25–30 fps practical (source-rate thumbnails, 29.97 included) and removes
  `thumbnail.updated`, the UI's `thumbnailSeqs` cache-busting, and the thumbnail part of
  the periodic emitter. `GET /thumbnails/{id}` stays for one-off snapshots.
- Not MJPEG (`multipart/x-mixed-replace`): each open stream holds one of the browser's 6
  connections, so 7+ feeds would stall the page.
- Possible later complement: a click-to-watch full-size monitor for one feed using real
  video (WebRTC, or H.264 over this WebSocket decoded with WebCodecs), with audio.

---

## Timecode

**Time-of-day timecode from the shared clock** is built: every frame recorded to MOV
carries its capture time as timecode, so recordings from every node in a clock domain
line up by timecode in an NLE (multicam sync in Resolve or Premiere), and every session
records when its first frame was captured.

- **Capture time.** A frame's capture time is its running time on the monitor plus the
  monitor's base time: its time on the node clock, which nodes in a domain share.
- **To wall-clock time.** The shared clock isn't wall time (a controller serves its
  monotonic clock; PTP is TAI), so each domain has a mapping, `utc_offset_us` (UTC minus
  clock time) on `ClockStatusDto`: a controller sends its own (from its wall clock) with
  every claim, so all its nodes map times identically; PTP is TAI − 37 s; a node on its
  own (or a source still on a previous clock) uses its own wall clock.
- **Timecode.** A probe on a MOV leg's encoder output stamps each frame with a
  `VideoTimeCodeMeta`: its capture time mapped to UTC, as time of day in the node's time
  zone, counted at the output's frame rate (drop-frame at 29.97 and 59.94). qtmux turns
  the first frame's into the file's `tmcd` track (each file's, when a leg splits); a
  frame that already has timecode keeps it. MP4 (`mp4mux` writes no timecode track) and
  Matroska (no timecode track) get none. Tested: a recording's file reads back the
  timecode of its first frame's clock time, exactly.
- **Sessions.** `SessionClockDto` keeps the offset it started with and gives the first
  frame's UTC time (`first_frame_utc`), shown in Recordings with the clock it was on.
  The mapping is fixed per session, so a controller's wall clock being adjusted (NTP)
  mid-recording doesn't step the timecode.
- Nodes in different time zones would write different timecode hours: keep a venue's
  machines in one zone.

_Still planned:_ real source timecode.

- Reads LTC from a designated audio channel or VITC from the video signal via GStreamer timecode elements and the Decklink SDK timecode API (it would take priority: a frame that has timecode keeps it)
- Exposed per-source via the status WebSocket and REST

### Clock sync across nodes

Synchronized starts, timecode for sources that carry none, and multi-angle replay all
need every node to agree on the time. Built (`clock.rs`): every pipeline on a node runs
on its **node clock**, and a controller's nodes share one.

- **Node clock.** A node's own system clock until a controller claims it. Monitors take
  it, and recording and output legs take the monitor's (with its base time), as before;
  channel players take their program's.
- **Controller mode** (the default): a controller serves its clock with GStreamer's
  `NetTimeProvider` on UDP `--clock-port` (default: HTTP port + 100, so 7800), and its
  nodes follow it with a `NetClientClock`. No per-machine setup or admin rights; a wired
  LAN gets well under a millisecond (a few hundred µs at worst, tens of µs on one switch).
  The served clock is the monotonic system clock, so an NTP step on the controller
  doesn't jump every pipeline on the network.
- **PTP mode** (`{ kind: "ptp", domain }`): the controller and its nodes all follow a
  PTP grandmaster already on the network (Dante, AES67 or SMPTE 2110 gear) with a
  `PtpClock`, for tighter sync and the same clock as that gear. GStreamer's helper
  binds UDP 319/320: Linux needs `cap_net_bind_service,cap_net_admin` on
  `gst-ptp-helper` (Arch ships them), macOS and Windows need nothing beyond the firewall.
  PTP time is TAI-based, so it's a shared time base, not wall-clock time.
- **Claims.** A controller claims each node with every health check
  (`POST /api/v1/node/clock/claim { controller_id, controller_name, mode, port }`), and
  the node follows the clock at the claim's source address. A node keeps the first
  controller that claims it until that one has been quiet for 20 s; then the next claim
  wins. A controller takes no claims: it follows its own mode, so two controllers make
  two clock domains rather than a loop. A node that stops being a controller keeps its
  clock until it's claimed.
- **Switching.** A new clock is used once it reports synced; until then the old one
  stays (status `pending`). A running pipeline can't change clocks, so idle monitors are
  rebuilt on the new one at once, and a source that's recording or playing a clip keeps
  its old clock until it's idle (`stale_sources`), when the monitor recovery loop
  rebuilds it. Tested: a recording across a switch finishes with no dropped frames, and
  its source moves over once it stops.
- **Losing the master.** A followed clock that hears nothing for 10 s is `lost`: it keeps
  running at its last measured rate and drifts slowly until the master is back.
- **Status.** `ClockStatusDto` (source, synced, pending, delay, lost, stale sources,
  error) is on `NodeStatus` and `NodeDto`; the Nodes page shows it on each card, and a
  controller emits `node.updated` when a peer's clock state changes (not for delay
  wobble). Settings has the controller's mode.

- **Clock domains.** Nodes agree on the time only within a domain: `controller:{id}` (a
  controller and the nodes following it), `ptp:{domain}`, or `local:{node id}` for a node
  on its own. `ClockStatusDto::domain` says which a node is in; `GET /clock/now` gives
  the time on it. Times in the API are in µs, which picks a frame and stays exact as a
  JavaScript number even on a PTP (TAI) clock.
- **Synchronized start.** A start can carry `start_at: { domain, time_us }`. On a node
  in that domain, whose source's monitor is on the node clock, every leg drops frames due
  before it (a probe on the leg's appsrc, by running time) and begins with the first one
  at or after it. Elsewhere it starts now. At most 10 s ahead; one that arrives late
  starts with the first frame still to come. The Record view's bulk Record sets one 1.5 s
  ahead (from the instance it talks to) for all its feeds and warns about any that
  couldn't wait for it. Tested: feeds on two nodes begin within a frame after the start
  (+14 and +20 ms at 30 fps). Closer than a frame needs the sources themselves locked
  (genlock); each runs its own frame grid.
- **Session clock.** Every session records its domain, the start it waited for, and when
  its first video frame was due on the clock (`SessionClockDto`, the `clock` column), so
  recordings in one domain can be lined up later, synchronized start or not. Missing if
  the source was still on a previous clock.

Built on it: each recorded frame's capture time on the shared clock, mapped to wall-clock
time per domain and written as MOV timecode (see Timecode above). Replay buffers will use
the same capture times as one timeline across nodes.

---

## Benchmark Runner

Finds how many feeds a node can record with a set of outputs, keep replay buffers of in
a codec, or run as playout channels (`benchmark.rs`), and estimates capacity from the
results (`capacity.rs`).

**A run** records more and more feeds of real footage (a file on the node played in a
loop: test patterns compress unrealistically) with a preset's outputs, sent inline like
a recording start. Each feed is a `FileSource` with its own monitor and recording legs,
built exactly as a real recording but kept out of the `SourceManager`, so it never shows
up as a source. Each step runs some number of feeds and, after a warm-up (5 s, plus 1 s
per 4 feeds started), measures:

- **Source shortfall:** frames the feeds didn't deliver. The monitor counts frames at
  its video producer (`MonitorPipeline::video_progress`), and the frames counted are
  compared with the time their timestamps span, not the wall clock, since frames arrive
  in bursts. A looping file marks the first frame of each loop `DISCONT`, and the gap
  before it (a stream shorter than the file's longest leaves one) is left out.
- **Output drops:** frames the legs' input queues dropped because an encoder couldn't
  keep up (the same count recordings report).
- Whole-machine CPU (averaged) and memory, and what the legs wrote per second, per
  output and per feed. GPU load isn't measured: there's no portable way to read it.

A step fails when the larger of the two drop percentages passes the threshold (default
0.5%). The feed count is searched for rather than stepped through one at a time
(`Search` in `benchmark.rs`):

1. **Doubling** (1, 2, 4, 8, …) with **quick checks** of 5 s. A quick check passes only
   well clear of the limit (under half the threshold, CPU under 85%), fails outright
   over 4× the threshold, and is otherwise followed by full-length steps.
2. **Narrowing:** once a count fails, the gap between the most feeds that passed and the
   fewest that failed is halved with full-length steps (`step_secs`, default 20).
3. The answer is always confirmed by a full step; if a count that only passed a quick
   check fails one, the search carries on below it.

A full step that fails is measured once more before it counts, so one hiccup elsewhere
on the machine doesn't decide it; every measurement is kept. A node that sustains 40
feeds takes about a dozen steps (5 min or so) instead of 40. The search goes no higher
than the feed limit (default 64; reaching it reads "at least N") and stops climbing when
memory runs short or a scratch volume drops under 2 GB free; a run also ends on an error
or when cancelled.

- **Memory:** an encoder that keeps up never fails a step, so memory is what stops a
  run on a machine with a fast encoder (ProRes on VideoToolbox: a run once climbed past
  24 feeds and froze an 8 GB Mac). Available memory is never let fall below 1.5 GB or an
  eighth of the machine's, whichever is more. It's read before each feed starts and
  every 0.5 s while feeds run; falling under that abandons the step (the feeds above the
  last count that passed go at once) and the count counts as failed. Each step also
  notes the most memory any one feed has taken, and the search doesn't try a count that
  wouldn't fit with 25% to spare. A run won't start on a machine already under the
  floor. On macOS "available" is the kernel's own free level (`kern.memorystatus_level`,
  as `memory_pressure` shows it): sysinfo's figure subtracts compressed memory and reads
  nothing on a Mac that's merely compressing. Elsewhere it's `MemAvailable`. The cap is
  cautious on a machine that's already swapping, and says so in the run's message.
- **Replay buffer runs:** each feed keeps a replay buffer (`ReplayLeg`, the real one)
  instead of recording, in a scratch folder beside the node's buffers, keeping 30 s so
  old segments are deleted as they would be. Output drops are the buffer's input-queue
  drops; what it writes is counted from the segments it finished, deleted ones included.
- **Channel runs:** each feed is a playout channel (`ChannelSource`, the real one) at the
  footage's size and rate, playing the footage on loop through its player, with its
  monitor (the program: compositor and mixer) and its outputs. Only outputs that run all
  the time and can run N times on one machine are measured: NDI (each feed's sender
  named `Benchmark N`, seen on the network while the run lasts) and SRT (each listening
  on a free localhost port, so it encodes with nobody connected). RTSP and WHEP encode
  only per viewer, so an unwatched one costs nothing, and a display needs a screen of
  its own; they're left out, and a run with neither NDI nor SRT measures the program
  alone. Source shortfall counts the program's frames, and also the clip's: frames its
  player didn't deliver, or that reached the program after their slot (the compositor
  drops those), counted at the lane (`Playout::played`); its percentage is taken against
  the clip's frames alone. Output drops are the outputs' input-queue drops, as a
  recording leg's. Nothing is written, so no scratch folder. Channels already running
  stay on (they always run); the run's message says how many did.

- **Where it writes:** each output writes into a scratch folder,
  `.capture-room-benchmark-{id}`, in the nearest existing folder of where the output
  would record, so the same disks are measured. Legs are discarded rather than
  finalized, and the folders deleted when the run ends. A run left `running` by a crash
  is marked failed on startup and its folders deleted. Only folders named so are ever
  removed.
- **Real recordings win:** a benchmark won't start while anything records or a replay
  buffer runs, only one runs at a time, and starting a recording or a replay buffer
  cancels a running benchmark (the start waits for its teardown, up to 15 s).
- **Results** are stored whole as JSON in `benchmark_results`, saved at every step and
  sent as `benchmark.updated`.

**Capacity estimate.** A result is looked up by what the outputs encode
(`outputs_key`: the outputs without their names, paths and split settings; replay buffers
by codec, `replay_key`; channels by `channel_key`, below) and the footage format; a
setup's latest completed run counts. A source of the same format gets
that result as is; another format scales the nearest result by pixels per second, and a
source whose format isn't known yet takes the newest (both flagged `estimated`). A feed
of a setup that sustains N feeds uses 1/N of the node, and the shares of the active
recordings, the replay buffers recording and the channels running add up to its
**load** (1 = full; one that couldn't sustain a single feed counts double). Shares only roughly add across different setups (a GPU-bound encoder and
a CPU-bound one share less than this assumes), so the estimate is a guide. Verdicts:
fits (≤ 80%), tight (≤ 100%), over, or unknown when something has no benchmark.

A running channel's key (`channel_key`) is its NDI and SRT outputs (`channel_outputs`):
without names, URLs, latency and passphrase, which don't change the work, with SRT's
bitrate, in a fixed order; RTSP, WHEP and display outputs are left out as in the
benchmark. Its format is its program's (the channel's size and rate), looked up like a
recording's, so a channel of another format scales from the nearest run. A channel counts
whenever its program runs, idle or playing: its program and outputs run either way, and
it can start playing at any moment. Viewers of RTSP and WHEP outputs aren't counted.

**Storage headroom.** `/storage` reports, per volume, what active recordings write
(each leg's bytes since its session started, once it has run 5 s) and the recording time
that leaves. The pre-start check adds the new legs at the rate a matching benchmark
measured, or else an estimate from their settings (`estimated_bytes_per_sec`: the
bitrate, Apple's target rates for ProRes, raw size for uncompressed). A recording isn't
started on a volume with less than 1 GB free.

The Record workspace checks before starting (`POST /capacity/check`, per node and
preset) and warns, never blocks, when a node would pass 80% of its capacity or a volume
would fill within the hour. Replay › Buffers checks the same way when a buffer is turned
on or changes codec (the check leaves out the source's own buffer, so it isn't counted
twice); a buffer keeps a fixed length, so it has no volume check. Running playout
channels count in the load these checks start from. Playback checks when a channel is
created or changed (`channel` in the request: one feed of its `channel_key` at its
format, leaving out the channel being changed, named in `source_ids`); channels write
nothing, so there's no volume check either.

---

## Playback

A playout system: Capture Room plays clips out to the same kinds of channels it
captures from. A channel runs a playlist of clips from its node's media library,
handing over from one to the next without a gap, out over NDI.

### Channels are sources

A playout channel is a configured source (`SourceConfig::Channel`, `sources/channel.rs`):
a fixed format (size, rate, audio channels) and a list of outputs. Its *program* is the
source's bin, run by an ordinary `MonitorPipeline`. So a channel gets thumbnails and
meters, shows in Sources and the Record multiview, and can be recorded like any feed.

```
Clip player (one per loaded clip)
uridecodebin ─┬→ queue → videoconvert → appsink (sync) ─ push ─┐
              └→ queue → audioconvert → audioresample → appsink ─ push ─┐
                                                               ▼        ▼
Program bin (ChannelSource::build_bin, run by the monitor)
  appsrc lane → input chain ─┐
     black background ───────┴→ compositor → channel caps (I420)  → "video"
  appsrc lane → input chain ─┐
              silence ───────┴→ audiomixer → 48 kHz F32, N ch      → "audio"

MonitorPipeline producers ─┬─► recording legs (unchanged)
                           └─► output legs (pipeline/output.rs), one pipeline each:
                               appsrc v/a → convert → ndisinkcombiner → ndisink
```

- **Never dark, never renegotiates.** The program bin reuses `live.rs`'s compositor over
  black and mixer over silence (`live::video_output` / `audio_output`), locked to the
  channel format from the start. Clips are scaled, converted and resampled into it.
  Audio is 1–8 channels; beyond stereo it has GStreamer's usual layout for the count
  (5.1 for 6, 7.1 for 8), which the mixer needs, so stereo clips are spread onto it and
  outputs can mix it down. Live sources' audio is laid out the same way, except picked
  device channels, which stay numbered (unpositioned).
- **Lanes.** Loading a clip adds an `appsrc` + input chain ("lane") per stream to the
  program bin, and unloading removes them: back to black and silence. The monitor builds
  a new bin on every (re)start, and `Lanes::attach` moves a loaded clip's lanes over to
  it, so a monitor restart doesn't drop the clip.
- **Decoupled players.** Each clip decodes in its own pipeline, on the program's clock
  (the node clock its monitor was built on). A bad file fails its player (the clip unloads and the error shows on the
  channel), never the program.
- **Timestamps.** A player buffer at running time `rt` renders when the clock reads
  `player base time + rt`; it's restamped to that moment in the program's running time,
  plus a 100 ms lead so it reaches the compositor before its slot. Video and audio share
  the mapping, so they stay in sync, and a pipeline moves its base time on resume, so
  pauses keep sync too (tested: identical offsets across a pause). Prerolls (a cued or
  paused frame) are stamped "now". A lane drops anything that doesn't start after the
  previous buffer, which covers seeks and the duplicate frame a loop point can produce.
- **Transport** (`Playout`, one per channel, in memory on the node; after a restart a
  channel is idle and black). The next-up item is always prerolled in standby (see
  Gapless handover), also while the channel is idle, so nothing needs cueing by hand:
  *play* plays the clip on air, or idle, the next-up item (the item after the last one
  played, else the first); *play item* plays any playlist item now, straight on from
  what's on air; *next* plays the next item now; *pause*; *seek* within the in–out range
  (the frame there shows while paused); *stop* unloads. *Cue* is still there to hold an
  item's first frame on air before rolling: it prerolls off air and switches when ready,
  so what's on air stays up meanwhile (over a clip playing, the lanes are replaced, or
  the cued frame would land behind what they've queued). At its out point an item plays
  the **next** one, holds its last frame, goes to black, or loops. Loops are non-flushing segment seeks, frame-accurate and gapless. The demuxer
  runs ahead of playback (even paused), so `SEGMENT_DONE` can arrive while a clip is
  still cueing; it's handled there too. The transport outlives the channel's config: a
  new format restarts its program and `InputSource::adopt` hands the transport over, so
  what's on air plays on. A rename restarts nothing: no source's fingerprint has its
  name, and a rescan renames the source it keeps (`InputSource::set_name`). Outputs are
  compared by identity (`output::identity`: the config with the channel-name defaults
  filled in), so only those named after the channel (an NDI name, an RTSP or WHEP path
  left unset) restart, and recordings of the channel carry on.
- **Playlists** (`playlists` and `playlist_items` tables) are stored per channel and
  handed to the transport when first used and after every edit; the API replaces the
  whole list at once (`PUT /channels/{id}/playlist`). Items keep their media entry's
  path and go with it when it's removed from the library. A playlist can loop. Edits
  don't touch the clip on air, except what it does at its out point (between next,
  hold and black), so "stop after this one" works while it plays.
- **Gapless handover.** The next item is cued in the background in *standby*: a prerolled player whose appsinks push nothing. Each player is in a
  *mode* (standby, cued: prerolls show, playing: buffers play). At the out point the
  next player starts with a fixed base time, chosen so its first frame (the running
  time of its last preroll) is stamped at the end of the last frame the video lane
  queued; it pushes into the same lanes, so the compositor sees one continuous stream
  (caps changes are converted in the lane's chain). Taking the next item by hand works
  the same way, from the latest of that point and now. The EOS that triggers it
  arrives about `LEAD` (100 ms) before the program needs the next frame, which covers the
  switch. Without a cued next item (it was still cueing, or failed), it's cued then,
  with a gap. Tested: a three-item playlist keeps 33 ms between every video frame and
  no audio gap across handovers.
- **Outputs** are data (`OutputConfig`, tagged by `type`: `ndi`, `srt`, `rtsp`, `whep`,
  `display`), each
  built by an `OutputSink` (the counterpart of `InputSource`). Pipeline outputs are fed
  from the monitor's producers, like a recording leg, with the monitor's clock and base
  time; a failing output (no NDI Runtime) fails alone and shows on the channel. `SourceManager`
  starts them with the monitor and restarts the ones that change; a channel's outputs
  are left out of its fingerprint, so editing them doesn't restart the program or the
  clip. NDI names are unique across a node's channels.
- **SRT output:** H.264 and AAC in MPEG-TS (`alignment=7`, one SRT payload per buffer)
  into `srtsink`, with fixed live settings: the first live encoder the node has
  (`vtenc_h264` realtime, then `x264enc` `zerolatency`), a keyframe every second and
  parameter sets with each one, so a receiver joining mid-stream waits at most a second.
  The output sets only bitrate, latency and an optional passphrase. URLs follow the
  Stream source's rules: no host listens (receivers connect; `wait-for-connection=false`
  so nobody connected drops the stream rather than holding up the program), a host
  calls (`srtsink` retries on its own until it answers). A listener counts its receivers
  (`caller-added` / `caller-removed`), shown beside the output. Listening ports are
  checked against every source's and output's on the node. Audio is at most 8 channels
  (AAC). Both encoders' output is held until both have caps, and video then starts at a
  keyframe: otherwise `mpegtsmux` writes a first program table with one stream and a
  second adding the other, and `srtsink` replays that first table to every receiver
  that connects, so each would see the program change (and many drop a stream).
- **RTSP output:** the node's RTSP server (`gst-rtsp-server`, port 8554, its own GLib
  main loop, started with the first RTSP output) serves each output as a mount,
  `rtsp://<node>:8554/<path>`, the path defaulting to the channel's name made URL-safe
  and unique across the node's channels. A shared media factory: the pipeline (the same
  live H.264 settings as SRT, AAC, `rtph264pay` / `rtpmp4gpay`) is built when the first
  viewer connects and torn down when the last leaves, so an unwatched mount encodes
  nothing. `media-configure` connects its appsrcs to the monitor's producers on the
  monitor's clock and base time; `unprepared` drops the links. Viewers are the session
  pool's sessions on the mount. Removing the output unmounts it and unprepares its
  media. Port 8554 is refused for listening sources while any channel serves RTSP (and
  the other way round). `OutputSink::start` returns a `RunningOutput`, so a sink needn't
  own a pipeline; NDI and SRT run theirs through `PipelineOutput`.
- **WHEP output:** WebRTC to browsers and WHEP players. Each output runs a
  `whepserversink` (gst-plugins-rs `webrtcsink` with the WHEP server signaller) whose
  HTTP server listens on localhost, on a free port; the node's API proxies
  `/whep/<path>` to it (`api/whep.rs`), so every channel is reached on the API port, at
  `http://<node>:<API port>/whep/<path>` (the path as RTSP's, unique across the node's
  WHEP outputs). Only these routes allow any origin, so web players elsewhere can
  connect; they only give out the program. The answer's resource becomes
  `/whep/<path>/resource/<id>` (PATCH, DELETE). The signaller starts once the program
  reaches the sink, so a viewer that comes before then gets 503 with `Retry-After`.
  `webrtcsink` encodes for each viewer and adapts the bitrate to their link, up to the
  output's; video is H.264 (`x264enc`) or VP8, audio Opus mixed to stereo. VideoToolbox
  is hidden from it (ranked none before the plugin registers): it fails `webrtcsink`'s
  negotiation and its bitrate can't be adapted; everything else creates encoders by
  name, so nothing else changes. Viewers are counted from `consumer-added` /
  `consumer-removed`. STUN is off (LAN). The WebRTC elements block on their own tokio
  runtime while changing state, so output pipelines change state off ours (as monitors
  do). Media goes over ports ICE picks per viewer, which can't be listed in the checks.
  The Playback workspace's Watch plays an output in the browser the same way.
- A channel no longer needs NDI to run: the Channel type needs the compositor, mixer
  and decoder, and reports the output types the node can send (`protocols`: `ndi`,
  `srt`, `rtsp`, `whep`, `display`).
- **Capacity:** running channels count against the node's benchmarked capacity, and the
  benchmark measures channels (see Benchmark Runner).
- **Decklink output** (🟡 built, untested on hardware): `decklinkvideosink` +
  `decklinkaudiosink` on a sub-device, in the mode matching the channel's format
  (progressive only for now; a pinned mode has to match it). Audio is embedded on 2
  or 8 channels, the program's first and silence after. The output runs on the node
  clock like every other output: the sink schedules frames on the card's clock and
  drops or repeats one when they drift apart (crystal error of 20–50 ppm is one every
  5–15 minutes at 59.94). Its dropped-frame count includes the card's. Making the card
  the clock master would avoid those frames, but changes which clock a channel runs
  on and how that relates to the shared node clock; deferred until hardware shows it
  matters, or there's house reference to lock to.
- **Still to come:** AJA outputs; interlaced SDI out; slates.

### Media library

A node's playable files: `media` table (path, name, probed `MediaInfo`, origin
`import | recording`, the session a recording came from). Files are added from the
node's file browser or from a session's files in Recordings, probed with the same
`Discoverer` check as file sources. Removing an entry leaves the file on disk; the list
flags entries whose file has gone (`missing`). A channel plays files on its own node
only.

### UI

The **Playback** workspace: channels on the left (state and clip), the selected
channel's program monitor with its outputs, a transport bar (play/pause, stop, next
with the next item's name, elapsed and remaining, a scrubber over the file with the
in–out range marked), the playlist (on air and next marked, total running time, loop
toggle; drag to reorder, double-click or a row's play button to play an item now), the
selected item's setup (in, out, what happens at the out point, Apply, Show first frame,
Play), and the node's media library on the right (double-click or drag an entry to add
it to the playlist). Play with nothing on air plays the next-up item. Keyboard: Space
play/pause, N next, Enter plays the selected item, I/O set in/out at the playhead, Esc
stop, Delete removes a focused playlist item. A WHEP output on the program monitor has
Watch, which plays it in the browser.
Channels are created and edited in Setup › Sources ("Playout channel").

### Browser preview

Separate from playout: the Recordings view plays a finished session's files in the
browser. A session stores each output's format when it starts (`outputs`), with
`playable` set for what browsers decode: H.264 4:2:0 with AAC, in .mp4 or .mov
(`RecordingProfile::browser_playable`; a playable .mov is served as `video/mp4`, which
Chrome and Firefox need). The view previews the output its preset marked **Preview in
Recordings** (`preview`, one per preset), else the first playable one; ProRes and PCM
outputs can only be downloaded.

Nodes serve a session's files with range requests (`tower-http`'s `ServeFile`), and only
files the session recorded, by output and file index, never arbitrary paths. Files of an
active session aren't served. The controller's forwarder streams peer responses and
passes range and caching headers both ways; its timeout covers only the wait for the
response headers, so a long download isn't cut off.

---

## Instant Replay

Builds on playback: replay is a rolling buffer per source plus a playout channel that
plays from it at variable speed.

- **Rolling buffer** (`pipeline/replay.rs`): a replay-enabled source records
  continuously, an extra consumer of its monitor like a recording leg, into one-second
  segments (`splitmuxsink` → `qtmux`, PCM audio) under
  `~/capture-room/.replay/<node id>/<source id>/`. The oldest are deleted past the
  buffer's length (minutes, per source). An always-on leg: it counts against capacity,
  and the benchmark measures it (see Benchmark Runner). The sound goes through
  `audiorate` (from the first sample): qtmux fails a PCM stream that jumps ("timestamps
  are drifting"), as a looping file source's does at each loop.
- **Codec is the operator's choice, per buffer:** all-intra, so slow motion, reverse
  and frame stepping are smooth and any frame can be cut. ProRes 422 Proxy (VideoToolbox,
  the default where it exists), intra-only H.264 (VideoToolbox, else x264) with as good
  a picture at about 60% of the size, or Motion JPEG on any platform. A 2 s GOP is fine
  for recording but not here. `/status` lists the ones a node can use. Settings come
  from a picture comparison (see What the comparison showed): `vtenc_h264` at quality
  0.75 (a bitrate is ignored with every frame a keyframe), `x264enc` at quantizer 18
  with its rate cap lifted (`pass=qual` otherwise caps at `bitrate`, 2 Mbit/s by
  default), and `jpegenc` given full-range BT.601, which is what JPEG is and what
  `jpegdec` reads it as.
- **Index:** each finished segment's span on the node clock, from splitmuxsink's
  fragment messages (running time plus the monitor's base time). The index outlives the
  leg: a source restart starts a new leg on it, leaving a gap; a new codec or clock
  clears it. Nothing survives a node restart (the folder is cleared).
- **Playing from it:** a clip is a span of one gap-free stretch, given to the channel as
  the finished files that cover it. The channel's player reads them through
  `splitmuxsrc` (its `format-location` signal) and decodes in software; a hold keeps
  them on disk while the clip lives. Replay reaches to within a segment of live. Speed is
  the player's seek rate, so the channel's lanes are unchanged (running time advances
  backwards and in slow motion too); audio is only played at 1×. Steps are `Step`
  events after a seek to the stepping direction; the position shown is the frame on
  screen (a player's own position is off after a step).
- **Shared timeline:** times are on the node clock, mapped to UTC by its domain's
  offset, so "the same moment" can be picked across angles on nodes sharing a clock.
- **Where it runs:** a replay node subscribes to the same NDI sources and keeps its own
  buffers, so a replay never reads another node's disk and nodes still don't know about
  each other. Set to connect to chosen NDI senders, it takes in only the angles it
  buffers (see Input Plugin System › `NdiSource`). Its buffers line up with the
  capture nodes' recordings of the same camera to within a couple of milliseconds
  (see What the NDI timing showed). The UI marks a sender several nodes have ("Also
  on"), and the Buffers sheet warns when another node already buffers it. Sources that only exist on another machine (SDI) can't be replayed from a
  different node; that's acceptable to start.
- **Operation:** the Replay workspace (see Web UI › Views): pick an angle and a moment,
  mark in/out, save clips, play out at variable speed or frame by frame through a
  playout channel, keyboard first. Still to build: jog wheels or a Stream Deck via
  WebHID / MIDI.
- **Saved clips:** saving in → out keeps the segments that cover it as the clip's own
  files, hard links under `~/capture-room/.replay-clips/<node id>/<clip id>/` (a copy
  if a link can't be made), so saving is instant, takes no room while the buffer still
  has them, and the buffer rolls on regardless; nothing holds its segments. Stored in
  `replay_clips` with positions in those files, so a clip plays and exports after the
  buffer has moved on, and after a restart. One angle per clip. A clip on a channel
  reports its id in the status (`replay_clip_id`) and keeps its node clock times, which
  line up with the buffers until the clock changes.
- **Clip groups:** saving on every angle (Shift+S) makes one clip per angle for the
  same span, as a group (`replay_clip_groups`; each clip's `group_id`): all of them or
  none. The group has the name (`Replay 14:03:22` by default) and its clips are named
  after their angles. The UI lists it as one row; with one of its clips on air,
  switching angle (a number key or the angle's chip) cues the other angle's clip at the
  same moment (`at_us`, the node clock time shown) and plays on if it was, so its in
  and out stay the group's. Exporting a group writes a file per angle (`<group> –
  <angle>.mov`); deleting it deletes every clip; a group goes with its last clip.
- **Clip export** (`replay::export`): the clip's files through `splitmuxsrc`, an
  accurate seek on the source to its in and out, into `qtmux` → one .mov in
  `~/capture-room/Replay clips/`, named after the clip (never overwriting), added to
  the media library (origin `replay`) so Playback can play it. No re-encoding: every
  frame is a keyframe, so the cut falls on any frame. A gate on each stream holds
  everything back until the seek's segment, as in the prototype, and trims the sound to
  start there: qtdemux hands out PCM in chunks of about half a second starting on the
  sample before the cut, and qtmux drops a chunk that starts before it whole. Tested:
  the export holds exactly the frames marked, and as long a stretch of sound.

### What the prototype showed

The prototype (`node/examples/replay_spike.rs`, removed after commit 4ced203; 2026-10-11, M3, 1080p59.94 from real footage, 1 s
segments) records each candidate codec, then plays the segments the way a channel's
player would (`splitmuxsrc` → decoder → appsink synced to the clock).

- **GStreamer's own trick modes are enough.** 1×, 0.5×, 0.25×, −1× and −0.5× across
  segment boundaries all play every frame in order with none late; an accurate seek takes
  0–13 ms and always lands on the requested frame; a step (`Step` event, after a seek to
  the stepping direction) takes 1–5 ms and moves exactly one frame either way. Running
  time still advances in reverse and slow motion, so a channel's lanes need no change.
- **Decode in software.** VideoToolbox (`vtdec`) loses a frame every few hundred in
  reverse, for ProRes, H.264 and JPEG alike; `avdec_prores`, `avdec_h264` and `jpegdec`
  lose none and keep up. So the replay player names its decoder instead of using
  decodebin (`--reverse` in the spike compares decoder chains).
- **Read closed segments only, by list.** `splitmuxsrc` with a glob fails on the segment
  being written; give it the closed ones through `format-location` (from
  `splitmuxsink-fragment-closed`). An open player doesn't see segments closed later, so a
  clip reopens it (30–60 ms). Replay can reach to within one segment of live.
- **Never seek through a muxer.** A flushing seek spoils `qtmux`'s file (and
  `splitmuxsink`'s first one), so recorders aren't seeked, and export holds the muxer's
  input back until the seek's segment arrives. Export then remuxes exactly the frames
  asked for, all decodable (a long-GOP cut leaves 45 of 255 frames undecodable).
- **Timestamp details.** Frame times are rounded down to the nanosecond, so cut half a
  frame early. `splitmuxsrc`'s segments start at 1000 s plus the position: compare in
  stream time. `vtenc_prores` marks every frame as a delta, so `splitmuxsink` never
  splits until the flag is cleared.
- **Codecs** (encode speed includes decoding the source):

  | Codec | Data rate | Per hour | Encode |
  |---|---|---|---|
  | ProRes Proxy (`vtenc_prores`) | 88 Mbit/s | 40 GB | 4.3× real time |
  | MJPEG q85 (`jpegenc`) | 87 Mbit/s | 39 GB | 2.9× |
  | H.264 intra (`vtenc_h264`) | 16 Mbit/s | 7 GB | 3.6× |
  | ProRes Proxy (`avenc_prores_ks`) | 83 Mbit/s | 37 GB | 0.2×: unusable |

  The intra H.264 encoder stayed far under its 120 Mbit/s target: with every frame a
  keyframe, `vtenc_h264` ignores a bitrate and falls back to its default quality. Its
  picture is compared below.

Not covered yet: audio (typically only played at 1×), several sources at once, and
playing through an actual channel.

### What the comparison showed

`node/examples/replay_quality.sh` (2026-10-14, M3): three 10 s clips of real footage
(4K30 worship practice scaled to 1080p30: a wide shot with fine detail, drums, a dark
close-up with lens flare), each codec with the settings replay records with, scored by
VMAF, PSNR and SSIM as a channel shows it (the replay decoder, then the program's I420
BT.709). Rates scaled to 1080p59.94:

| Codec | Per hour | VMAF | Worst 1% |
|---|---|---|---|
| ProRes Proxy (`vtenc_prores`) | 38–40 GB | 95.4–97.2 | 93.7–93.9 |
| H.264 intra, `vtenc_h264` quality 0.75 | 20–31 GB | 97.1–97.8 | 95.0–95.7 |
| H.264 intra, `vtenc_h264` quality 0.85 (before) | 37–55 GB | 98.1–98.7 | 96.8–96.9 |
| H.264 intra, `x264enc` q18, rate cap lifted | 20–27 GB | 95.6–97.0 | 93.1–94.0 |
| H.264 intra, `x264enc` q18 (before: 2 Mbit/s cap) | 2 GB | 32–42 | 7–30 |
| MJPEG q85, full-range BT.601 | 38–54 GB | 94.7–96.4 | 90.9–93.1 |
| MJPEG q85 (before: range mismatch) | 35–49 GB | 63–68 | 60–61 |

- **Intra H.264 at quality 0.75 is ProRes Proxy's picture at about 60% of the size.**
  It scores at least as well on every clip, worst frames included; side by side, with
  the shadows lifted, it smooths dark background grain a little more than ProRes and
  shows no banding. 0.70 (VMAF 96.3–97.1, 16–25 GB) is about level with ProRes; 0.85
  costs as much as ProRes for a VMAF gain hardly visible, and each 0.05 above it adds
  half again or more to the size for under 0.2 VMAF.
- **x264 was capped at 2 Mbit/s.** `pass=qual` caps the rate at `bitrate` (2048 kbit/s
  by default), so the fallback recorded a smeared picture. Lifted, quantizer 18 lands
  about level with ProRes Proxy at half its size.
- **MJPEG was recorded as the wrong range.** `jpegenc` stored limited-range BT.709 as
  is; `jpegdec` reads every JPEG as full-range BT.601, so a replay showed lifted blacks
  and greys for white. Converted to full-range BT.601 first, it's about level with
  ProRes Proxy. The Test source type now says what it draws (limited-range BT.709):
  left open, a JPEG encoder downstream negotiated it to full range, which
  `videotestsrc` ignores, and hid the bug in the tests.
- **Decoding keeps up.** In software, all of them decode 1080p at over 200 fps, more
  than three times 59.94 (ProRes 390–580, H.264 310–530, JPEG about 230).

### What the NDI timing showed

A capture node and a replay node receiving the same NDI camera each stamp its frames
themselves, so the same frame could get different times on the two.
`node/examples/ndi_timing.rs` (2026-10-15, M3, one machine, 720p30) sends a test
pattern over NDI, receives it twice, the second receiver joining 5 s after the first,
and matches frames by the sender's NDI timestamp (a `ReferenceTimestampMeta` from
`ndisrcdemux`), comparing each frame's running time plus base time as a node maps it.

- **`auto` (what nodes use) is fine.** It smooths receive time against the sender's
  timecode. A receiver that just connected starts up to a frame off, and is within
  2 ms of one that was already connected after 2 s (1–3 ms on average over 20 s).
  Nodes connect to a sender when they find it, long before anything records or
  buffers it, so recordings and buffers start after it settles.
- **`timestamp` was tightest here, but not across machines.** It maps the sender's
  timestamp through the receiving machine's wall clock, so between machines it's off
  by their NTP offset (milliseconds or worse), not the node clock's tens of µs.
  `receive-time` is unsmoothed: within 1 ms on average, with single frames up to
  25 ms off.
- **Not measured:** two machines. There the LAN adds its delay difference
  (sub-millisecond) and the shared clock its error (tens of µs), both far below a
  frame.
- **Mapping is unchanged:** `ndisrc` stays on `auto`.

## Node API — `/api/v1/node/…`

Local only. Never forwards, never knows about other nodes. Source and session ids are local to the node.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/status` | id, name, version, uptime, `is_controller`, available encoders and source types, clock status, the replay codecs it can use (default first) |
| GET | `/clock` | clock status: source (`local \| controller \| ptp`), domain, synced, pending, delay, lost, stale sources, error |
| GET | `/clock/now` | `{ domain, time_us }`: the time now on this node's clock |
| POST | `/clock/claim` | a controller claims this node's clock (see Clock sync); answers with the clock status |
| GET / PUT | `/settings` | node name, monitor settings (thumbnail fps/size, meter interval) |
| GET | `/storage` | writable volumes: mount point, total/free bytes, removable; what active recordings write to each and the time left |
| GET | `/sources` | sources on this machine |
| POST | `/sources/scan` | rescan |
| GET | `/sources/{id}` | source details |
| GET / POST | `/configured-sources` | configured source configs (test, file, stream, device, WHIP); saving validates them: a file is probed, a stream's protocol must be available and its listening port free, a device must be known (400 otherwise) |
| PUT / DELETE | `/configured-sources/{id}` | |
| GET | `/checks` | Setup checks, read live: each check's status (`ok \| missing \| not_asked \| unknown`), whether something on the node needs it, the fix (steps, System Settings link, command, download link, whether the node can show the OS prompt); the OS, the app holding macOS permissions, and the ports others connect to |
| POST | `/checks/request` | `{ id }`: show the macOS permission prompt (`camera`, `microphone`) on this machine |
| GET | `/update` | where this node's software update stands (`UpdateStatusDto`: current and latest version, why it can't update, phase, download percent, last error) |
| POST | `/update/check` | look for a newer release now |
| POST | `/update` | update to the newest release: answers once started (400 with the reason if it can't), then the node restarts into it |
| GET | `/devices` | capture and audio devices on the node (key, name, class, provider, modes), for picking one in a device source or audio plan |
| GET / POST | `/media` | the media library (each entry flags a missing file) / add a file `{ path, name?, session_id? }`: probed, 400 if it can't be played, 409 if already in |
| DELETE | `/media/{id}` | remove from the library (the file stays) |
| GET | `/channels/{id}` | a playout channel's transport: state (`idle \| cued \| playing \| paused \| ended`), clip, item on air, next item and whether it's in standby, position, duration, error, each output's status |
| GET / PUT | `/channels/{id}/playlist` | the playlist (items with in, out and end, whether it loops) / replace it |
| POST | `/channels/{id}/play` | `{ item_id }` → plays now, straight on from what's on air |
| POST | `/channels/{id}/cue` | `{ item_id }` → on air, paused at its in point |
| POST | `/channels/{id}/transport` | `{ action: play \| pause \| stop \| seek \| next \| speed \| step, position_ms?, rate?, frames? }`; play with nothing on air plays the next-up item. `speed` (a replay only) plays on at `rate` from the frame shown, negative backwards; `step` pauses and moves `frames` (±) |
| POST | `/channels/{id}/replay` | `{ source_id, from_us, to_us?, rate?, play }` → part of a source's replay buffer on air (node clock times; clamped into one stretch of the buffer, to its newest frame without `to_us`), cued on its first frame or playing |
| GET | `/replay` | replay buffers: source, codec, length, encoder, error, the span held (`start_us`, `end_us`, gaps), the clock now and its UTC offset, bytes |
| PUT / DELETE | `/replay/{source_id}` | `{ codec: prores_proxy \| h264_intra \| mjpeg, minutes }` → give a source a buffer or change it (a new codec starts afresh) / remove it and what it recorded |
| GET / POST | `/replay/clips` | saved clips, newest first, a group's together (angle, name, codec, in and out on the node clock, capture time in UTC, bytes, whether files are missing, group id and name) / save one `{ source_id, from_us, to_us, name? }` (named after the angle and its time of day by default) |
| PATCH / DELETE | `/replay/clips/{id}` | `{ name }` → rename / delete it and its files (exports stay) |
| POST | `/replay/clips/{id}/export` | remux it into one .mov in the Replay clips folder, add it to the media library → the media entry |
| POST | `/replay/clip-groups` | `{ source_ids, from_us, to_us, name? }` → save in → out on several angles as one group, a clip each (all or none) |
| PATCH / DELETE | `/replay/clip-groups/{id}` | `{ name }` → rename / delete it with its clips and their files |
| POST | `/replay/clip-groups/{id}/export` | export each clip whose files are there → the media entries |
| POST | `/channels/{id}/replay-clip` | `{ clip_id, rate?, play, at_us? }` → a saved clip on air, cued on its first frame (or at `at_us` on the node clock, as another angle of its group) or playing |
| GET | `/files?path=` | folders and media files in a directory (home if no path), for picking a file source. Read-only |
| GET / POST | `/recordings` | list (`?before=&limit=`: newest first, 100 by default, at most 500, paged by `started_at`; the first page also carries active sessions) / start. Start body: `{ source_id, preset_id?, outputs: [...], start_at?: { domain, time_us } }` (see Clock sync). Refused onto a volume with under 1 GB free; cancels a running benchmark |
| GET / DELETE | `/recordings/{id}` | session details / remove a finished session from history; `?files` deletes its files from disk first, and if any can't be deleted the session stays, listing those still on disk (409 while recording) |
| GET / HEAD / DELETE | `/recordings/{id}/outputs/{i}/files/{j}` | a finished session's file, with range requests; `?download` adds `Content-Disposition` / delete it from disk and the session's list (`?path=` must match, so a stale index is refused; the session stays in history). 409 while recording |
| POST | `/recordings/{id}/stop` | stop (waits for EOS drain) |
| GET / POST | `/benchmarks` | runs, newest first / start one: `{ outputs, media_path, max_feeds?, step_secs?, drop_threshold_pct?, preset_id?, preset_name?, replay?, channel? }` (`replay`: a codec, with `outputs` empty, to benchmark replay buffers; `channel`: NDI and SRT `OutputConfig`s, possibly none, with `outputs` empty, to benchmark playout channels; 409 while recording, keeping replay buffers, benchmarking, or short of memory) |
| GET / DELETE | `/benchmarks/{id}` | a run / delete it (not while running) |
| POST | `/benchmarks/{id}/cancel` | cancel; answers once it has torn down |
| GET | `/capacity` | each benchmarked setup's result, the load of active recordings, replay buffers and channels, the running benchmark |
| POST | `/capacity/check` | `{ outputs, source_ids, replay?, channel? }` → verdict, load before/after, time left on the volumes they'd write to (with `replay`, giving them buffers in that codec instead; with `channel`, running a playout channel of that format and outputs in place of the one in `source_ids`, if any: no volumes either way) |
| GET | `/thumbnails/{source_id}` | latest JPEG |
| WS | `/ws` | this node's events only |

## Controller API — `/api/v1/…`

Mounted on every instance; peer-related routes only do anything while the controller is enabled.

| Method | Path | Description |
|--------|------|-------------|
| PUT | `/controller` | `{ enabled }` — promote/demote live |
| GET / PUT | `/controller/clock` | `{ mode, port, serving }` / set the mode: `{ kind: "controller" }` or `{ kind: "ptp", domain }` (409 if not a controller) |
| GET | `/nodes` | self + registered peers |
| POST | `/nodes` | add a node by URL (persisted) — 409 if not a controller |
| DELETE | `/nodes/{id}` | remove a node |
| GET | `/reachability/{id}` | probe a node's ports from this controller (TCP connect; UDP listed as unknown) as checks; for itself, one unknown row (a machine can't test its own firewall) |
| ANY | `/nodes/{id}/{*path}` | forwarded to that node's `/api/v1/node/{path}`. Self is served in-process; peers over HTTP. |
| GET / POST | `/presets` | presets stored on this instance |
| PUT / DELETE | `/presets/{id}` | |
| WS | `/ws` | this node's events + every peer's (relayed) |

---

## WebSocket Events

All events are JSON with a `type` and the `node_id` they describe. `source_id` / `session_id` are local to that node.

| Event type | Payload |
|------------|---------|
| `recording.started` / `recording.stopped` | session id, source id |
| `recording.error` | session id, source id, error — the session ended in error |
| `recording.leg_failed` | session id, source id, error (the session's accumulated message) — a leg failed; the session keeps recording on its other legs |
| `recording.stats` | session id, source id, dropped frames per leg, ordered like `output_paths` (1 Hz while active) |
| `recording.removed` | session id — removed from history |
| `feed.status` | source id, timecode, monitor error, link state for live sources (1 Hz) |
| `channel.state` | source id, the channel's transport status (on every command, ~10 Hz while playing, else 1 Hz) |
| `media.updated` | none — the node's media library changed |
| `audio.levels` | source id, channel peak/RMS values (~10fps) |
| `thumbnail.updated` | source id (at the configured thumbnail fps) |
| `benchmark.updated` | the run — when it starts, begins a step, finishes a step, and ends |
| `node.updated` | none — this node's name or monitor settings changed |
| `node.online` / `node.offline` | `peer_id` (controller only) |

The controller's relay subscribes to each peer's `/api/v1/node/ws` — local events only — so a peer that is itself a controller is never echoed, and two controllers can watch the same nodes without duplicates.

---

## TypeScript Types

Rust structs used in API responses are annotated with `#[derive(TS)]` from the `ts-rs` crate. Running `cargo test --features export-types` generates TypeScript definitions into `ui/src/types/generated/`. These files are committed to the repo so the UI can be developed without a prior Rust build.

---

## SQLite Schemas

Every instance has the same schema (see `node/migrations/`):

- `node_config` — key/value: `uuid`, `name`, `controller_enabled`, `monitor_*`
- `recording_sessions` — one row per session, `output_paths` as a JSON array; the
  source and preset names and each output's format are kept with it, so history reads the
  same after either is renamed or removed; `clock` (JSON) places it on its clock
- `configured_sources` — test, file, stream, device, WHIP and playout channel configs (`config` is JSON tagged with `type`)
- `media` — the media library: path (unique), name, probed info (JSON), origin, session id
- `presets` + `preset_outputs` — used while acting as controller (or from the UI on a lone node)
- `nodes` — peers added by URL on a controller (mDNS peers are not persisted)
- `benchmark_results` — benchmark runs, each stored whole as JSON (`run`)

---

## Web UI

**Framework:** Vue 3 + TypeScript  
**Build:** Vite  
**State:** Pinia  
**Routing:** Vue Router  
**HTTP:** `ofetch`  
**Components:** shadcn-vue — `npx shadcn-vue@latest init --preset ae2ZjdI` from `ui/`  

In production, the compiled UI is embedded into the Rust binary via `rust-embed` and served by every instance. In development, Vite runs its own dev server and proxies `/api` and `/ws` to the Rust controller.

Also `vue-sonner` for toasts. Tables use a small in-house `DataTable` (sort, filter,
column visibility, grouping) rather than TanStack Table, whose v9 API is brand new and
more than these tables need.

### Design system

- **Tokens:** dark-first OKLCH tokens in `ui/src/style.css`, light kept in step, on
  slightly blue-gray tinted surfaces. `--primary` is sky-cyan (actions, toggles, focus,
  selection, links). Solid red means "live": `--tally` is recording only,
  `--destructive` a soft treatment behind a confirm, and `--brand` red is just the
  logo's record light. Self-hosted Inter + JetBrains Mono; `num` utility for technical
  values. Density = root font size.
- **Preferences:** `usePreferences()` — theme, density, Multiview tile size and overlays,
  inspector, table columns; per browser, surfaced in the view they affect.
- **Shared patterns:** `PageHeader`, `ConfirmDialog`, `EditSheet`, `StatusDot`,
  `TallyBadge`, `KeyValueList`, `CopyButton`, `DataTable` + `ColumnsMenu`,
  `StorageVolumeBar`.

### Workspace shell

The tool will grow into recording, playback and instant replay, so the shell is
organized around workspaces (operating desks) rather than pages. A top header holds
identity, labeled workspace tabs and global status (connection, nodes, lowest free
storage, REC count, clock). Each workspace has a toolbar row and its own panel layout. A
tabbed bottom panel (Log / Recordings / Storage, Ctrl+J) is shared. ⌘K palette and
toasts throughout. Workspaces: Record, Playback, Replay, Recordings and Setup; new ones
join when the backend supports them — not before.

### Views

| Workspace / view | Description |
|------------------|-------------|
| **Record** | Multiview of every feed across all nodes — thumbnail, name, timecode, recording state, audio meters, dropped frames; multi-select with bulk Record/Stop (warning when a node nears its capacity or a volume would fill within the hour); inspector with outputs, session and history |
| **Recordings** | Session history across nodes, filtered by text, status and node and paged per node ("load older"); detail pane with a browser preview, each output's files (play, download, copy path) and remove from history |
| **Setup › Sources** | Sources grouped by node, capabilities, test and file source authoring (with a browser for the node's files) |
| **Setup › Presets** | Create and edit recording presets (outputs, path preview) |
| **Setup › Nodes** | A card per node: health, sources, storage (with time left while recording), capacity (benchmarked setups, replay codecs and channels, load from recordings, replay buffers and channels, running benchmark, history); controller toggle, add/remove nodes |
| **Setup › Checks** | Per node: permissions, software, storage, clock and network checks, problems first in the counts, with how to fix each; Nodes cards badge their problems |
| **Setup › Settings** | Appearance; monitoring settings (thumbnail/meter rate, applied to all nodes); About |
| **Playback** | Playout channels, playlists, transport, channel settings |
| **Replay** | Angles (sources with a buffer, numbered) beside the program of the channel picked to play on, the selected angle live, and its buffer on a timeline up to now (30 s, 2 min or all). Replay the last N seconds (R), switch angle at the same moment (1–9), play, pause, step a frame (← →), reverse and 25–100 % speed (J K L, ↑ ↓), mark in and out on the frame shown (I O), play in → out (Enter) and save it as a clip (S, Shift+S on every angle as one group). The clip list under the timeline cues a clip (click), plays it at the chosen speed (double-click), exports, renames and deletes it; a group is one row that plays the selected angle, with a chip per angle that switches to it at the same moment. Commands queue and each decides what to do when its turn comes, so fast keying isn't lost. A Buffers sheet turns buffers on per source with their codec and length |
| **Schedules** _(planned)_ | Create, edit, and view upcoming scheduled recordings |
| **Logs** _(planned)_ | Aggregated log viewer with filter by node and level |

Planned views stay out of the nav until they're built.

### Real-time State

A single WebSocket connection (`/ws`) feeds all reactive UI state via Pinia stores. Sources and sessions are keyed by `(node_id, id)`. Components subscribe to store slices; they don't manage WebSocket connections directly.

---

## File Naming

| Token | Value |
|-------|-------|
| `~` | Leading only: the recording node's home directory |
| `{date}` | `YYYY-MM-DD` |
| `{node}` | Node name |
| `{source}` | Source id |
| `{datetime}` | `YYYYMMDD_HHMMSS` |
| `{output}` | Output leg name |
| `{ext}` | Container file extension |

Planned: `{source_name}` (display name, made filename-safe — source ids aren't readable
for NDI) and `{preset}`.

Default template (the built-in default output, and new legs in the preset editor):
```
~/capture-room/{date}/{source}_{datetime}.{ext}
```

Templates are resolved on the node (`profile::plan_legs`). Two legs of one
preset that would resolve to the same file are rejected when the preset is
saved, and again when recording starts.

A recording never writes over an existing file. A template with `{take}` gets the first
take number none of whose files exist; any other gets a `_2`, `_3`, … suffix on the file
name (e.g. `{datetime}` restarted within the same second). Starting also refuses a path
that exists or that an active recording writes to, checked under the source manager's
lock, which catches two sources racing for one file (a template without `{source}`).

Example (`/media/recordings/{date}/{node}/{source}_{datetime}_{output}.{ext}`):
```
/media/recordings/2026-06-21/node-01/cam3_20260621_143022_master.mov
/media/recordings/2026-06-21/node-01/cam3_20260621_143022_proxy.mp4
```

---

## Node Discovery

Every instance registers an mDNS service (`_capture-room._tcp.local.`). A controller browses for it, identifies each service via `GET /api/v1/node/status`, and adds it to its registry. mDNS peers are pruned after ~15 s of failed health checks and re-added when they re-announce. Peers added by URL are persisted and never pruned — they're just shown as unreachable.

Discovery is asymmetric: the controller initiates all connections; nodes never need to know a controller exists. The one thing pushed is the clock claim, sent with every health check; a node holds it only as long as claims keep coming (see Clock sync).

---

## Ports & Networking

One HTTP port per machine, configurable, default `7700`. Set via config file or `--port` flag.

A controller also serves its clock on UDP `--clock-port` (default: the HTTP port + 100,
so `7800`), which its nodes must be able to reach. PTP mode uses UDP 319/320 multicast
on every node.

Plain HTTP/WebSocket over LAN. No TLS required for v1 (trusted network assumed).

**Threat model, stated plainly:** there is no authentication. Anyone who can reach port
7700 on any node can start and stop recordings, change settings, delete recorded
files, and (through a
controller) do the same on every peer. Before packaging, at least a shared token
(sent by the UI, checked by every node, and passed on by the controller's forwarder and
relay).

---

## Deployment

One binary, installed as a system service.

- **Linux:** systemd unit
- **macOS:** launchd plist. A LaunchDaemon can't get camera, microphone or
  screen-recording permission (macOS has no user session to ask). Local capture devices
  have landed, so macOS needs a per-user LaunchAgent in a logged-in
  session instead.
- **Windows:** Windows Service via `windows-service` crate. Similar limits: a service
  runs in session 0, with no access to the desktop for display capture or fullscreen
  output.

Config file:
- Linux/macOS: `/etc/capture-room/config.toml`
- Windows: `%APPDATA%\CaptureRoom\config.toml`

Cross-compiled for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `x86_64-apple-darwin` via GitHub Actions.

### macOS package (✅)

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
```

- **Release:** pushing a tag `vX.Y.Z` (it must match `node/Cargo.toml`) runs
  `.github/workflows/release.yml` on a macOS runner, which publishes
  `capture-room-macos-{aarch64,x86_64}.tar.gz`, their `.sha256` and `install.sh` as a
  GitHub release. Run by hand, it builds the same files as a workflow artifact.
- **Build:** `scripts/macos/package.sh <aarch64|x86_64>` (works locally too).
  `scripts/macos/gstreamer.sh` downloads the official GStreamer packages (version
  pinned in the workflow and the script) and unpacks them without installing into an
  SDK, which the node links against, and a runtime that ships in the app. The runtime
  leaves out the python, gtk, editing, visualizers, dvd and devtools packages, static
  libraries and headers, and the framework's own `ndi`, `fallbackswitch` and
  `rswebrtc` plugins (the node links its pinned ones in; two of each would clash).
  Each app is one architecture, thinned from the universal framework: about 100 MB
  to download, 300 MB installed.
- **App:** `Capture Room.app` (`com.capture-room.node`): the binary in
  `Contents/MacOS`, GStreamer in `Contents/Frameworks` (found through an rpath). At
  startup `plugins::use_bundled_gstreamer` points GStreamer's plugin path, plugin
  scanner, PTP helper and GIO modules into the bundle, with a registry of its own in
  `~/Library/Caches/capture-room`, so a Homebrew GStreamer on the same Mac is never
  mixed in. `Info.plist` has the camera, microphone and local network usage strings
  and the Bonjour types (`_capture-room._tcp`, `_ndi._tcp`), `LSUIElement` (no Dock
  icon) and `NSAppSleepDisabled`. The app is the permission holder for macOS privacy
  settings.
- **Install:** `install.sh` downloads the app for the Mac's architecture, checks
  its checksum, puts it in `/Applications` (`~/Applications` if that isn't writable)
  and runs `capture-room service install`; everything after that is the binary's
  (`service` module), so installing, updating and the Finder agree. Options:
  `--controller`, `--port`, `--version`, `--archive <local build>`,
  `--uninstall [--purge]` (runs `service uninstall`); recordings in `~/capture-room`
  are never removed. Opening the app from the Finder (no arguments, not run by the
  service) installs the service if it isn't, starts it, opens the UI in the browser
  and exits.
- **Service:** a LaunchAgent, `com.capture-room.node`. Its plist lives in
  `~/Library/Application Support/Capture Room/`, with the database
  (`capture-room.db`); the log is `~/Library/Logs/Capture Room/capture-room.log`.
  The service is always loaded from there, and *Start at Login* is a copy of the plist
  in `~/Library/LaunchAgents` (what launchd loads at login), so turning it off never
  stops the running node. `KeepAlive { SuccessfulExit = false }`: launchd restarts the
  node when it crashes, panics or is killed, but not after a clean exit (Quit, or a
  stop from launchd); it also starts it on load. (`KeepAlive { Crashed }` would allow
  not starting at load, but misses panics and `SIGKILL`.) `ProcessType Interactive`,
  so it isn't throttled, and `CAPTURE_ROOM_SERVICE=1` in its environment. A
  reinstall keeps the port and the Start at Login choice. `capture-room service
  status` reports it.
- **Quitting:** Quit, `SIGTERM` and Ctrl+C stop every recording and wait for its files
  to finish (`session::quit`) before exiting with 0; exit code 75 asks for a restart
  (the updater).
- **Menu bar icon** (`tray.rs`, on the window loop's thread; shown by the service, or
  with `--tray`): a record symbol, drawn as a template image when idle and red while
  recording. Its menu: the version, what it's doing (recording, channels playing),
  Open Capture Room (the UI in the browser), Check for Updates / Update to X
  (disabled while busy, with progress while updating), Start at Login, and Quit
  (which says how many recordings it stops). The node sends it a status every second.
  On a crowded menu bar (a notched MacBook) macOS hides items that don't fit.
- **Updates** (`update.rs`): every release has `latest.json` (`{"version": …}`) and,
  per archive, a minisign signature (`.minisig`). A node checks `latest.json` at
  start and every 6 hours. Updating, from the node card's Update button or the menu:
  only the installed app running as the service, only to a newer version, never while
  recording, playing a channel or benchmarking. It downloads the archive and its
  signature into `.capture-room-update` beside the app, verifies the signature with the
  public key compiled in (`scripts/release-key.pub`), unpacks it, checks its
  `Info.plist` version is the one expected, swaps it in with two renames (the old app
  is kept in the staging folder until the next start), and exits with 75 so launchd
  starts the new version. A failed download or signature is reported on `/update` and
  in the menu, and nothing changes. `CAPTURE_ROOM_RELEASES` (passed to the service when
  set at install) points the updater at a folder served over HTTP instead of GitHub,
  for testing. Nodes run from a shell report `unsupported` and the UI shows nothing.
- **Release key:** minisign, unencrypted, kept outside the repo (the maintainer's
  `~/.config/capture-room/release.key`) and in the `MINISIGN_KEY` Actions secret;
  `package.sh` signs when `MINISIGN_KEY_FILE` is set, and a tagged release fails
  without it. Replacing the key means nodes on older versions refuse the new
  releases; they'd need `install.sh` once.
- **Signing:** ad hoc (`CODESIGN_IDENTITY`, default `-`) until there's a Developer ID.
  Nothing downloaded by `curl` or the updater is quarantined, so Gatekeeper never
  checks it; a browser download would need Open Anyway. An ad hoc signature changes
  with every build, so macOS asks for camera, microphone, screen recording and local
  network access again after each update; a stable identity (a self-signed
  certificate, or a Developer ID) fixes that. No hardened runtime yet: with ad hoc
  signatures it would refuse to load the plugins.
- **Not yet:** Linux (a systemd user unit, GStreamer from distro packages) and Windows
  (a logon task, a tray icon, renaming the running exe to replace it) behind the same
  `service` and `update` modules; rolling back an update that won't start; the API has
  no authentication yet, so anyone on the network can start an update (only to a
  newer, signed release).

### Plugin provenance

Three tiers, the same on every OS:

| Tier | Examples | How it ships |
|------|----------|--------------|
| Core GStreamer (C) | encoders, `rtspsrc`, `srtsrc`, `webrtcbin`, device providers, and the RTSP server library (`libgstrtspserver`, linked, not a plugin: a node without it won't start; Linux `gst-rtsp-server` / `libgstrtspserver-1.0-0`) | Linux: distro package dependencies. macOS: GStreamer.framework bundled in the .app. Windows: the official MSVC runtime DLLs and plugin dir next to the exe, with `GST_PLUGIN_PATH` set relative to it at startup |
| Rust plugins | ndi, fallbackswitch, webrtc (WHIP, WHEP) | statically linked into the binary (one pinned gst-plugins-rs rev), registered in `main.rs` |
| Vendor runtimes | NDI Runtime | user-installed (licensing, below) |

The official macOS and Windows GStreamer builds include srt, rtsp, nice and webrtcbin, so
no source type is missing there. Adding a gst-plugins-rs git dependency re-resolves
gstreamer-rs `branch=main`; pin it back with `cargo update --precise`.

### OS permissions for capture

- **macOS:** `NSCameraUsageDescription` / `NSMicrophoneUsageDescription` in Info.plist;
  screen recording needs TCC approval. Run as a LaunchAgent (see above).
- **Windows:** camera and microphone privacy toggles; Media Foundation and screen
  capture need an interactive session.
- **Linux:** the user must be in the `video` group for v4l2. Wayland screen capture needs
  a desktop session and the portal.

### Setup checks

Setup › Checks lists, per node, what each feature needs from the machine and whether it
has it (`node/src/checks.rs`). Checks run live on the node on every request (the OS is
the source of truth, so nothing is recorded). A check is a *problem* when it's missing
and `required`: something on the node needs it now (a source using a camera, a channel
with an NDI output) or it's essential (storage, the clock it's meant to follow). Missing
but not required reads "Not set up": it only matters once the feature is used. Nodes
cards badge their problem count. Checks are a list each feature adds to:

| Check | Platforms | How |
|-------|-----------|-----|
| Screen Recording, Camera, Microphone | macOS | `CGPreflightScreenCaptureAccess`, `AVCaptureDevice authorizationStatusForMediaType:` (screen recording can't tell denied from not asked) |
| NDI Runtime | all | the NDI device provider started (it fails without `libndi`; the plugin tries once per process, so installing needs a restart) |
| Recording folder | all | `~/capture-room` can be created and written, with at least the 1 GB a start needs on its volume |
| NDI intake | all | connecting to every NDI sender, with more than 4 of them connected (about 130 Mbit/s each at 1080p60): never required, it suggests connecting to chosen ones |
| Controller's clock / PTP | all | from the clock status, when following one |
| API (also WHEP outputs), WHIP and RTSP ports | all | TCP connect from the controller (`/reachability/{id}`); UDP listeners (SRT, UDP streams) listed as unknown; WebRTC media ports are picked per session and can't be listed |

Not yet: Linux `video` group and PTP-helper capabilities, Windows privacy toggles.

Shortcuts help the user grant what's missing; nothing grants it for them. When the
browser is on that machine (loopback, and the node is this instance or at a loopback
URL): a link to the right System Settings pane, and, for camera and microphone not yet
asked, Ask macOS, which shows the permission prompt (`requestAccessForMediaType:`).
Screen recording gets no prompt button: macOS can't say whether it has asked, and only
asks once (`CGRequestScreenCaptureAccess` did nothing in testing), so the fix is System
Settings, adding the app with + if it isn't listed. Otherwise a command to copy
(`New-NetFirewallRule` on Windows, `ufw` / `setcap` on Linux) or a download link. macOS
privacy permissions belong to whatever launched capture-room (the nearest app bundle
among its parent processes: Terminal from a shell, until there's an app bundle), and
the page names it.

### NDI licensing

`gst-plugin-ndi` is MPL-licensed and compiled into the binary. It dlopens `libndi` at
runtime; users install the NDI Runtime separately (same pattern as OBS). We never
distribute `libndi` itself. The plugin only looks in `NDI_RUNTIME_DIR_V6` / `_V5` and the
default library path, and on macOS nothing installs it there (the SDK has its own folder,
NDI Tools bundles a copy in each component), so on macOS the node looks in those places at
startup and points the plugin at the first it finds (`plugins::locate_ndi_runtime`).
Installing NDI Tools is enough.

| Dependency | Who provides |
|------------|-------------|
| `gst-plugin-ndi` | compiled into binary (static) |
| `libndi` | user installs NDI Runtime redistributable |

---

## Monorepo Structure

```
capture-room/
├── Cargo.toml                   # Rust workspace
├── pnpm-workspace.yaml          # pnpm workspace (ui/ only)
├── package.json                 # root scripts: dev, build, types
│
├── node/                        # Rust binary
│   ├── src/
│   │   ├── main.rs
│   │   ├── api/
│   │   │   ├── node.rs          # Node API (/api/v1/node/…, local only)
│   │   │   ├── error.rs         # ApiError / ApiResult for handlers
│   │   │   └── types.rs         # DTOs (exported to TS; some double as DB rows)
│   │   ├── controller/
│   │   │   ├── mod.rs           # Controller enable/disable (live toggle)
│   │   │   ├── api.rs           # /nodes, /controller, /presets, merged /ws
│   │   │   ├── forward.rs       # /nodes/{id}/… → node API (in-process or HTTP)
│   │   │   ├── discovery.rs     # mDNS advertise/browse + health polling
│   │   │   ├── relay.rs         # peer WS → merged /ws
│   │   │   └── registry.rs      # NodeRegistry
│   │   ├── benchmark.rs         # Benchmark runner
│   │   ├── clock.rs             # Node clock: local, a controller's (net clock) or PTP; serving
│   │   ├── capacity.rs          # Capacity estimates from benchmark results
│   │   ├── checks.rs            # Setup checks (permissions, NDI, storage, clock) and port probes
│   │   ├── storage.rs           # Storage volumes, write rates, file browsing
│   │   ├── pipeline/
│   │   │   ├── monitor.rs       # MonitorPipeline: thumbnail, audio meter, StreamProducers
│   │   │   ├── recording.rs     # RecordingLeg: one pipeline per output leg
│   │   │   ├── profile.rs       # RecordingProfile (codec, container, bitrate, chroma, …)
│   │   │   └── mod.rs           # Latest<T> (AudioMeter, ThumbnailStore), element helpers
│   │   ├── sources/
│   │   │   ├── mod.rs           # InputSource trait
│   │   │   ├── manager.rs       # SourceManager — sources, per-source monitors, recording sessions
│   │   │   ├── test.rs          # TestSource
│   │   │   ├── file.rs          # FileSource (looping media file) + probe
│   │   │   └── ndi.rs           # NdiSource + NDI device monitor
│   │   ├── session.rs           # Background lifecycle: stops/teardowns, leg-failure reports, monitor recovery
│   │   └── db/                  # sqlx migrations and queries
│   ├── migrations/
│   └── Cargo.toml
│
└── ui/                          # Vue 3 web UI
    ├── src/
    │   ├── types/generated/     # Auto-generated by ts-rs (committed)
    │   ├── views/
    │   ├── components/
    │   ├── stores/              # Pinia stores
    │   ├── composables/         # useWebSocket, useApi
    │   └── router/
    ├── package.json
    └── vite.config.ts           # /api and /ws proxied to :7700 in dev
```

### Root Scripts

```json
{
  "scripts": {
    "dev": "concurrently \"pnpm dev:node\" \"pnpm dev:ui\"",
    "dev:node": "cargo watch -x 'run -p capture-room -- --controller'",
    "dev:ui": "pnpm --filter ui dev",
    "build": "pnpm build:ui && cargo build --release",
    "build:ui": "pnpm --filter ui build",
    "types": "cargo test -p capture-room --features export-types"
  }
}
```

---

## Build Order

The original v1 build order has been folded into [ROADMAP.md](ROADMAP.md), which owns
sequencing. Its history is in git.
