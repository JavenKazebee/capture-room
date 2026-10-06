# Capture Room — Roadmap

Active sequencing of work, decisions, and rationale. This complements
[ARCHITECTURE.md](ARCHITECTURE.md) (the design spec) — when the two disagree on
*order*, this file wins; ARCHITECTURE.md remains the source of truth for *design*.

_Last updated: 2026-10-06_

---

## Current sequence

1. ✅ **Generic TestSource + Sources view**
2. ✅ **Live source monitoring**
3. ✅ **NDI capture** (+ plugin build/packaging)
4. ✅ **Multi-pipeline output per preset**
5. ✅ **UI overhaul / dark mode**
6. ✅ **Looping media file source**
7. ✅ **Benchmark + capacity estimator** (with storage headroom)
8. ✅ **Recordings view** (+ browser preview)
9. **Playback** — playout channels, NDI output first
10. **Clock sync across nodes** — prerequisite for multi-angle replay
11. **Instant replay**
12. **Follow-on**, in rough priority order (see below)

Rationale for front-loading 1–2 ahead of NDI: a configurable TestSource plus the
Sources view gives a real authoring/verification surface, and live monitoring forces
the source-lifecycle design (connect-on-discovery, not connect-on-record) that NDI
capture depends on anyway.

---

## Shipped

Design detail for all of these lives in ARCHITECTURE.md; the history is in git.

- **Node / controller simplification (2026-10-02)** — every instance is a node with a
  local-only API; the controller is a live toggle that adds discovery, a forwarder and a
  merged event stream. Presets go inline with start commands, so nothing is synced.
- **1. Generic TestSource + Sources view** — parameterized video/audio test feeds,
  authored from the Sources view.
- **2. Live source monitoring** — an always-on `MonitorPipeline` per source owns
  thumbnails and meters; `SourceManager` is the single owner of capture state; live
  reconfiguration; non-blocking stop.
- **3. NDI capture** — `gst-plugin-ndi` statically linked; persistent device monitor;
  users install the NDI Runtime themselves.
- **4. Multi-pipeline output per preset** — N output legs, one pipeline each, fed by
  `StreamProducer`s so a failing leg fails alone; failed-leg reporting and automatic
  monitor restart (2026-10-03); per-leg dropped-frame counts (`recording.stats`);
  encoder fallback (VideoToolbox, then software). Crash-safe files and splitting
  (2026-10-04): MOV/MP4 with compressed audio record as fragments, PCM legs reserve the
  index at the front and roll over to a new file every 4 h, and an output can split by
  time or size through `splitmuxsink` — so all-day recordings are covered.
- **5. UI overhaul / dark mode** — design tokens, preferences, workspace shell (Record
  and Setup), shared components, and a pass over every existing view.
- **6. Looping media file source** — a file on the node played in a loop as a live
  feed, picked through a node file browser. Configured sources (test and file) now share
  one `configured_sources` table and `/configured-sources` API.
- **7. Benchmark + capacity estimator (2026-10-04)** — a node records looping copies
  of real footage with a preset's outputs, searching for the most feeds that keep up
  (doubling with quick checks, then narrowing with full steps), and stores what each
  setup sustains. Capacity load from active recordings and a pre-start check
  (warns, never blocks) in the Record workspace; storage shows the measured write rate
  and recording time left per volume, and a start onto a nearly full volume is refused.
  Run from the node cards. Hardware encoders and encode sharing were dropped from the
  front of the queue (see Follow-on), so the benchmark measures VideoToolbox and
  software encoders for now: re-run it when new encoders land.
- **8. Recordings view (2026-10-04)** — a Recordings workspace: session history across
  nodes (filters, paged per node), a detail pane with a browser preview, each output's
  files (play, download) and remove from history (files stay on disk). Sessions now keep
  their source and preset names and each output's format; any H.264/AAC output in
  .mp4/.mov previews, and a preset can mark the one to prefer. Nodes serve a session's
  own files with range requests and the controller's forwarder streams them. Deleting
  (2026-10-05): one file, or a whole session with or without its files; a session whose
  files can't all be deleted stays, listing the ones still on disk. Not yet: previews of
  growing files.
- **No overwriting on start (2026-10-04)** — an existing file gets a `_2` suffix
  (or the next `{take}`); a path another recording holds is refused.
- **Record and Recordings refinements (2026-10-06)** — the Record multiviewer drops the
  inspector for a per-tile info popover and adds a Fit tile size; page headers share one
  layout.
- **CI (2026-10-06)** — fmt, clippy, tests, generated-types check and UI type-check on
  GitHub Actions. The codebase was run through `rustfmt` once to start clean.
- **Node registry persistence** — peers added by URL are stored and restored on start.

---

## 9. Playback

A full playout system — capture in reverse. Design in ARCHITECTURE.md (Playback).

- **`OutputSink` trait** mirroring `InputSource`; each output type is the counterpart of
  a source type.
- **Playout channels:** one always-running pipeline per output at a fixed format, black
  and silence when idle, fed by per-clip player pipelines through `StreamProducer`s.
  Gapless, frame-accurate cueing; thumbnails and meters like a source.
- **Playlists** with in/out points and transport (cue, play, pause, stop, next, loop),
  owned by the node running the channel.
- **Output types, in order:** NDI (`ndisink`, no hardware needed) → SRT / RTSP → local
  display / HDMI out → WHEP → Decklink / AJA.
- Reuses file decoding from the looping media file source (#6).
- Playout channels count against capacity: extend the benchmark to cover them.
- **Playback workspace** in the UI.
- Start with: one NDI channel playing a single file, then gapless playlists, then more
  output types.

## 10. Clock sync across nodes

Design sketch in ARCHITECTURE.md (Timecode › Clock sync).

- Nodes report their clock offset (NTP, or PTP where frame accuracy matters); the UI
  warns when it's too large.
- Network-synced GStreamer clock; frames stamped with wall-clock capture time.
- **Synchronized start:** a start command can carry a wall-clock time, so bulk Record
  across machines produces files that line up.

## 11. Instant replay

Builds on playback (#9) and clock sync (#10). Design in ARCHITECTURE.md (Instant Replay).

- **Prototype first:** one source into a rolling buffer of all-intra segments, played
  through a channel. The point is to test reverse playback, variable speed and
  frame-stepping in GStreamer before designing the UI.
- Then: replay buffers per source on a replay node (subscribing to the same NDI
  sources), marks and clip list, variable-speed playout, clip export without
  re-encoding, **Replay workspace**.
- Replay buffers count against capacity; extend the benchmark to cover them and
  playout channels.

## 12. Follow-on

Roughly in priority order.

- **Hardware encoders** — VideoToolbox is in. Add NVENC (`nvh264enc` / `nvh265enc`),
  Intel QSV (`qsvh264enc`, …) and VA-API (`vah264enc`, …) to
  `RecordingProfile::encoders`, with the same build-first-that-works fallback. Only
  VA-API can be tested on current hardware (AMD); re-run benchmarks once they land.
  Encode sharing (legs with identical encode settings sharing one encoder) is decided
  against for now.
- **Benchmark: GPU load** — not measured (no portable way to read it); CPU, memory and
  write rate are.
- **Authentication** — the API is open to anyone on the network, and any of them can
  start or stop recordings, or delete recorded files. At least a shared token, before
  packaging.
- **Packaging** — cross-platform builds via GitHub Actions; NDI packaging per
  ARCHITECTURE.md.
- **Timecode** — real LTC/VITC extraction; not started (TestSource fakes wall-clock TC).
- **Scheduler engine** — runs on the controller and sends ordinary start/stop commands
  (with inline outputs) at the scheduled times. Nodes stay stateless about schedules.
- **Higher-fps thumbnails via a subscription WebSocket** — replaces request-per-frame
  thumbnails, which don't scale past ~10–15 fps with more than a handful of feeds.
  Design in ARCHITECTURE.md (Thumbnails).
- **Additional source types** (each is a new `InputSource` impl, additive), in priority
  order:
  1. **RTSP** (`rtspsrc`) — IP cameras; easy, high value.
  2. **Separate audio pairing** (decision first) — record a video source with audio from
     another source (an audio interface, Dante Virtual Soundcard). Needs the source
     contract to allow video-only and audio-only sources, plus a pairing in the source
     config. Settle this before local devices, which have no paired audio.
  3. **Local capture devices** — one source type for HDMI/USB capture cards (Cam Link,
     Magewell), webcams and audio interfaces on every platform, discovered through a
     `GstDeviceMonitor` (`Video/Source`, `Audio/Source`) like `NdiMonitor`. Replaces the
     Linux-only `v4l2src` plan; the monitor yields `v4l2src` / `avfvideosrc` /
     `mfvideosrc` elements per platform. On macOS and Windows screens also appear as
     devices, which covers most of display capture (not under a service session, though).
  4. **SRT** (`srtsrc`) — contribution feeds over unreliable networks.
  5. **WHIP ingest** (`whipserversrc`, gst-plugins-rs) — accept WebRTC pushes from OBS
     30+ and browsers. Statically linked like `gst-plugin-ndi`.
  6. **SDI via Decklink** — already specced; needs hardware.
  7. **SDI via AJA** (`ajasrc`, gst-plugins-bad ≥ 1.24) — alongside Decklink; needs
     hardware.
  8. **Web page** (`wpesrc`) — render an HTML page (graphics, scoreboards) as a feed.
     Niche; WPE is Linux-only.
