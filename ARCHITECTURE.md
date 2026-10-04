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
- `NdiSource` — ✅ implemented — built on the `gst-plugin-ndi` GStreamer elements (`ndisrc` + `ndisrcdemux`), not the raw NDI SDK FFI. Discovery via a persistent `GstDeviceMonitor`. Follows the same bin/ghost-pad contract as `TestSource`.
- `FileSource` — ✅ implemented — a media file on the node played in a loop as a live feed (real footage for the benchmark, demos without hardware). The file plays in its own *player* pipeline (`uridecodebin` → appsinks, synced to the clock) that loops with segment seeks, so loops are gapless unless the file's streams differ in length. Its appsinks push into live `appsrc`s in the source bin, which restamp every buffer with the monitor's running time, so the monitor sees one continuous live feed. The player starts when the monitor first wants data and stops when the bin is disposed; its errors are posted on the monitor. A file without audio plays stereo silence; one without video is refused. The file is probed (`Discoverer`) when the source is saved, which rejects anything undecodable and gives the source its capabilities up front.

**Configured vs discovered sources.** Test and file sources are *configured*: stored in `configured_sources` as a JSON config tagged with its `type` (`SourceConfig` in `api/types.rs`), and served from `/configured-sources`. A new configured type adds a `SourceConfig` variant and a case in `sources::configured`. NDI sources are *discovered* and not stored.
- `DecklinkSource` — ⬜ deferred (no hardware) — Decklink SDK via FFI / `decklinkvideosrc`

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
  Opus 160 kbps in MKV.
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

_Planned — not built. Today `TestSource` reports a fake wall-clock timecode and other
sources report none._

- Reads LTC from a designated audio channel or VITC from the video signal via GStreamer timecode elements and the Decklink SDK timecode API
- Exposed per-source via the status WebSocket and REST
- Written into output file metadata where the container supports it (MOV)

### Clock sync across nodes (planned)

Synchronized starts, timecode for sources that carry none, and multi-angle replay all
need every node to agree on the time. Not specced in detail yet:

- Nodes keep their system clocks in sync (NTP at minimum, PTP where frame accuracy
  matters); a node reports its measured offset, and the UI warns when it's too large.
- Pipelines use a network-synced GStreamer clock, so running time means the same thing
  on every node.
- Each captured frame can be stamped with wall-clock capture time, giving recordings
  and replay buffers one shared timeline across nodes.
- **Synchronized start:** a start command can carry a wall-clock start time; every leg
  begins at the first frame at or after it.

---

## Benchmark Runner

Determines sustainable recording capacity for a given machine on demand:

1. Spins up pipelines fed by looping media files (real footage — test patterns compress
   unrealistically) at increasing feed counts
2. Measures: dropped frames per pipeline (leg and source side), CPU and GPU/encoder load,
   disk throughput, memory pressure
3. Stops when dropped frames exceed a configurable threshold (TBD — likely expressed as a percentage of frames over a rolling window)
4. Reports: max sustainable feed count at that profile, raw metrics per step
5. Stores results in local SQLite

Later, playout channels and replay buffers (below) count against the same capacity and
should be part of the measurement.

---

## Playback (planned)

A full playout system: Capture Room plays clips out to the same kinds of channels it
captures from, in reverse. An NDI source has an NDI output counterpart, Decklink input
has Decklink output, and so on.

### Output plugin system

```rust
pub trait OutputSink: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn output_type(&self) -> OutputType;
    /// Formats the output accepts (an SDI card is fixed; NDI takes anything).
    fn capabilities(&self) -> Option<OutputCapabilitiesDto>;
    fn fingerprint(&self) -> String;
    /// A `gst::Bin` with `"video"` / `"audio"` ghost *sink* pads.
    fn gst_sink_element(&self) -> gst::Element;
}
```

Implementations, in roughly the order of the matching sources:

- **NDI** — `ndisinkcombiner` + `ndisink` (already in `gst-plugin-ndi`); first, since it
  needs no hardware
- **SRT** (`srtsink`) and **RTSP** (`gst-rtsp-server`) — encode + mux in the bin
- **Local display / HDMI out** — fullscreen window on a chosen screen, plus an audio
  device
- **WHEP** (`whepserversink`) — browser and OBS pull
- **SDI via Decklink / AJA** (`decklinkvideosink`, `ajasink`) — needs hardware; the card
  is the clock master

### Playout channel

A channel is the output-side twin of a source's monitor pipeline: one persistent
pipeline per configured output, always running, at a fixed channel format (e.g.
1080p59.94, 48 kHz stereo).

```
Clip player pipeline, one per loaded clip
[filesrc] → [demux/decodebin] → [appsink] = video/audio StreamProducer

Channel pipeline (always running)
[appsrc video] → [videorate → videoscale → videoconvert → caps channel format] → [vtee]
[appsrc audio] → [audioconvert → audioresample → caps channel format]          → [atee]
     vtee/atee ─► OutputSink bin
               └► thumbnail + audio meter branches (same as a monitor)
```

- **Never goes dark:** with nothing playing, the channel outputs black and silence (or a
  configured slate). Downstream equipment never loses signal.
- **Decoupled players:** clips decode in their own pipelines and feed the channel
  through `StreamProducer`s, the same pattern recording legs use in reverse. A bad file
  fails its player, not the channel.
- **Gapless cueing:** the next clip is prerolled (PAUSED, at its in-point) and its
  producer is connected at a frame boundary, so cuts between clips are frame-accurate.
- **Conform:** clips are scaled, rate-converted and resampled to the channel format;
  the channel never renegotiates downstream.
- **Monitoring:** channels get thumbnails and meters, so they show in Multiview beside
  sources.
- **Loopback (optional):** a channel can appear as an internal source, so its program
  output can itself be recorded.

### Media and playlists

- A channel plays files on its own node: recordings, or imported media. Playing a file
  that lives on another node means copying it first.
- **Playlist (rundown):** ordered items, each a file with in/out points; cue, play,
  pause, stop, next, loop, auto-advance or hold at the end of each item.
- **State lives on the node.** Gapless timing can't depend on a controller round trip,
  so the node running a channel owns its loaded playlist and transport state. This is
  the one deliberate exception to "nodes keep no state": the controller still only sends
  plain commands, and the playlist is sent inline when loaded.

### API and events (sketch)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/outputs` | output devices/targets on this machine (Decklink outputs, screens, …) |
| GET / POST / PUT / DELETE | `/channels…` | channel configs: output, format, idle slate |
| PUT | `/channels/{id}/playlist` | load a playlist (inline) |
| POST | `/channels/{id}/transport` | `{ action: cue \| play \| pause \| stop \| next \| seek, … }` |
| GET | `/media` | playable files on this node, with probed format |

Events: `channel.state` (item, position, remaining; ~10 Hz while playing),
`channel.error`. New tables: `output_channels`, `media` (probed file info). Playlists
are not stored on the node.

### Browser preview

Separate from playout: previewing a recording in the browser (Recordings view) needs a
browser-playable file. A preset output can be flagged as the **playback proxy** (H.264
AAC MP4); nodes serve files with HTTP range requests, and the controller's forwarder
streams them rather than buffering.

---

## Instant Replay (planned)

Builds on playback: replay is a rolling buffer per source plus a playout channel that
plays from it at variable speed.

- **Rolling buffer:** each replay-enabled source records continuously into short
  segments on disk (`splitmuxsink`), oldest deleted past a configured duration. This is
  an always-on extra leg per source and counts against capacity.
- **Codec built for scrubbing:** all-intra (ProRes Proxy, MJPEG or intra-only H.264), so
  slow motion, reverse and frame stepping are smooth. A 2 s GOP is fine for recording
  but not here.
- **Shared timeline:** segments are indexed by wall-clock capture time (see Clock sync),
  so "the same moment" can be picked across angles.
- **Where it runs:** a replay node subscribes to the same NDI sources and keeps its own
  buffers, so a replay never reads another node's disk and nodes still don't know about
  each other. Sources that only exist on another machine (SDI) can't be replayed from a
  different node; that's acceptable to start.
- **Operation:** mark in/out on any angle, a clip list, play out at variable speed
  (e.g. 50%, 25%, frame-step) through a playout channel. Keyboard first; jog wheels or
  a Stream Deck via WebHID / MIDI later.
- **Clip export:** all-intra segments can be cut on any frame and remuxed into a file
  without re-encoding.

---

## Node API — `/api/v1/node/…`

Local only. Never forwards, never knows about other nodes. Source and session ids are local to the node.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/status` | id, name, version, uptime, `is_controller` |
| GET / PUT | `/settings` | node name, monitor settings (thumbnail fps/size, meter interval) |
| GET | `/storage` | writable volumes: mount point, total/free bytes, removable |
| GET | `/sources` | sources on this machine |
| POST | `/sources/scan` | rescan |
| GET | `/sources/{id}` | source details |
| GET / POST | `/configured-sources` | test and file source configs; saving a file source probes the file (400 if it can't be played) |
| PUT / DELETE | `/configured-sources/{id}` | |
| GET | `/files?path=` | folders and media files in a directory (home if no path), for picking a file source. Read-only |
| GET / POST | `/recordings` | list / start. Start body: `{ source_id, preset_id?, outputs: [...] }` |
| GET | `/recordings/{id}` | session details |
| POST | `/recordings/{id}/stop` | stop (waits for EOS drain) |
| GET | `/thumbnails/{source_id}` | latest JPEG |
| WS | `/ws` | this node's events only |

## Controller API — `/api/v1/…`

Mounted on every instance; peer-related routes only do anything while the controller is enabled.

| Method | Path | Description |
|--------|------|-------------|
| PUT | `/controller` | `{ enabled }` — promote/demote live |
| GET | `/nodes` | self + registered peers |
| POST | `/nodes` | add a node by URL (persisted) — 409 if not a controller |
| DELETE | `/nodes/{id}` | remove a node |
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
| `feed.status` | source id, timecode, monitor error (1 Hz) |
| `audio.levels` | source id, channel peak/RMS values (~10fps) |
| `thumbnail.updated` | source id (at the configured thumbnail fps) |
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
- `recording_sessions` — one row per session, `output_paths` as a JSON array
- `configured_sources` — test and file source configs (`config` is JSON tagged with `type`)
- `presets` + `preset_outputs` — used while acting as controller (or from the UI on a lone node)
- `nodes` — peers added by URL on a controller (mDNS peers are not persisted)
- `benchmark_results` — reserved

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
toasts throughout. **Playback** and **Replay** workspaces join when the backend supports
them — not before.

### Views

| Workspace / view | Description |
|------------------|-------------|
| **Record** | Multiview of every feed across all nodes — thumbnail, name, timecode, recording state, audio meters, dropped frames; multi-select with bulk Record/Stop; inspector with outputs, session and history |
| **Setup › Sources** | Sources grouped by node, capabilities, test and file source authoring (with a browser for the node's files) |
| **Setup › Presets** | Create and edit recording presets (outputs, path preview) |
| **Setup › Nodes** | A card per node: health, sources, storage; controller toggle, add/remove nodes (benchmarks planned) |
| **Setup › Settings** | Appearance; monitoring settings (thumbnail/meter rate, applied to all nodes); About |
| **Recordings** _(planned)_ | Session history and files, across nodes; browser preview via the playback proxy |
| **Playback** _(planned)_ | Playout channels, playlists, transport |
| **Replay** _(planned)_ | Replay buffers, marks and clips, variable-speed playout |
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

Discovery is asymmetric: the controller initiates all connections; nodes never need to know a controller exists. Nothing is pushed on connect because nodes hold no controller state.

---

## Ports & Networking

One port per machine, configurable, default `7700`. Set via config file or `--port` flag.

Plain HTTP/WebSocket over LAN. No TLS required for v1 (trusted network assumed).

**Threat model, stated plainly:** there is no authentication. Anyone who can reach port
7700 on any node can start and stop recordings, change settings, and (through a
controller) do the same on every peer. Before packaging, at least a shared token
(sent by the UI, checked by every node, and passed on by the controller's forwarder and
relay).

---

## Deployment

One binary, installed as a system service.

- **Linux:** systemd unit
- **macOS:** launchd plist. A LaunchDaemon can't get camera, microphone or
  screen-recording permission (macOS has no user session to ask), so once local capture
  devices or display output land, macOS needs a per-user LaunchAgent in a logged-in
  session instead.
- **Windows:** Windows Service via `windows-service` crate. Similar limits: a service
  runs in session 0, with no access to the desktop for display capture or fullscreen
  output.

Config file:
- Linux/macOS: `/etc/capture-room/config.toml`
- Windows: `%APPDATA%\CaptureRoom\config.toml`

Cross-compiled for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `x86_64-apple-darwin` via GitHub Actions.

### NDI licensing

`gst-plugin-ndi` is MPL-licensed and compiled into the binary. It dlopens `libndi` at
runtime; users install the NDI Runtime separately (same pattern as OBS). We never
distribute `libndi` itself.

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
│   │   ├── storage.rs           # Storage volume listing
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
