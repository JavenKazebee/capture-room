# Capture Room — Roadmap

Active sequencing of work, decisions, and rationale. This complements
[ARCHITECTURE.md](ARCHITECTURE.md) (the design spec) — when the two disagree on
*order*, this file wins; ARCHITECTURE.md remains the source of truth for *design*.

_Last updated: 2026-10-04_

---

## Current sequence

1. ✅ **Generic TestSource + Sources view**
2. ✅ **Live source monitoring**
3. ✅ **NDI capture** (+ plugin build/packaging)
4. ✅ **Multi-pipeline output per preset**
5. ✅ **UI overhaul / dark mode**
6. **Looping media file source** — prerequisite for the benchmark
7. **Hardware encoders + encode sharing** — before the benchmark, so it measures the
   encoders people will actually use
8. **Benchmark + capacity estimator** (with storage headroom)
9. **Recordings view** (+ browser preview via a playback proxy)
10. **Playback** — playout channels, NDI output first
11. **Clock sync across nodes** — prerequisite for multi-angle replay
12. **Instant replay**
13. **Follow-on**, in rough priority order (see below)

**Any time, small:** CI (fmt, clippy, tests, UI type-check), which doesn't need to wait
for packaging; and refusing to overwrite an existing file when a recording starts
(see File Naming in ARCHITECTURE.md).

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
  crash-safe MOV/MP4 (index reserved at the front, rewritten every 10 s); encoder
  fallback (VideoToolbox, then software).
- **5. UI overhaul / dark mode** — design tokens, preferences, workspace shell (Record
  and Setup), shared components, and a pass over every existing view.
- **Node registry persistence** — peers added by URL are stored and restored on start.

---

## 6. Looping media file source

`filesrc` / `uridecodebin`, seek to start on EOS. Pulled forward from the source-type
list because the benchmark needs real footage: test patterns compress unrealistically
(bars ≈ free, snow = worst case). Also useful for demos without hardware.

## 7. Hardware encoders + encode sharing

- **Encoders:** VideoToolbox is in. Add NVENC (`nvh264enc` / `nvh265enc`), Intel QSV
  (`qsvh264enc`, …) and VA-API (`vah264enc`, …) to `RecordingProfile::encoders`, with
  the same build-first-that-works fallback. These set how many feeds one machine can
  encode, so they come before the benchmark.
- **Encode sharing:** legs with identical encode settings (codec, resolution, framerate,
  bitrate, chroma) but different containers or paths share one encoder.

## 8. Benchmark + capacity estimator

Not started (the empty `benchmark/` stub was removed).

- Pipelines fed by looping media files (#6) at increasing feed counts → measure
  **dropped frames, CPU, GPU/encoder load, disk write throughput, memory pressure**.
  Stop past a dropped-frame threshold.
- Count drops in both places: the leg's `appsrc` (shipped) and the source/monitor side,
  which isn't counted yet.
- Persist runs in `benchmark_results` (table already specced in ARCHITECTURE.md).
- **Capacity estimator:** given a preset (incl. multi-leg) and feed count, predict
  sustainability from stored benchmarks — surfaced in the Nodes view.
- **Storage headroom:** today a full disk only shows up as a failed leg. Show recording
  time left per volume (free space ÷ bitrate of the legs writing to it), warn below a
  threshold, and check space before a recording starts.

## 9. Recordings view

Session history and the files each session produced, across nodes. Already planned in
ARCHITECTURE.md; arguably more useful day to day than the scheduler.

- **Browser preview:** a preset output can be flagged as the playback proxy (H.264/AAC
  MP4 — browsers can't play ProRes or PCM-in-MOV). Nodes serve files with HTTP range
  requests; the controller's forwarder must stream them, not buffer.
- Doubles as the media browser that Playback (#10) loads clips from.

## 10. Playback

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
- **Playback workspace** in the UI.
- Start with: one NDI channel playing a single file, then gapless playlists, then more
  output types.

## 11. Clock sync across nodes

Design sketch in ARCHITECTURE.md (Timecode › Clock sync).

- Nodes report their clock offset (NTP, or PTP where frame accuracy matters); the UI
  warns when it's too large.
- Network-synced GStreamer clock; frames stamped with wall-clock capture time.
- **Synchronized start:** a start command can carry a wall-clock time, so bulk Record
  across machines produces files that line up.

## 12. Instant replay

Builds on playback (#10) and clock sync (#11). Design in ARCHITECTURE.md (Instant Replay).

- **Prototype first:** one source into a rolling buffer of all-intra segments, played
  through a channel. The point is to test reverse playback, variable speed and
  frame-stepping in GStreamer before designing the UI.
- Then: replay buffers per source on a replay node (subscribing to the same NDI
  sources), marks and clip list, variable-speed playout, clip export without
  re-encoding, **Replay workspace**.
- Replay buffers count against capacity; extend the benchmark to cover them and
  playout channels.

## 13. Follow-on

Roughly in priority order.

- **Authentication** — the API is open to anyone on the network, and any of them can
  start or stop recordings. At least a shared token, before packaging.
- **Packaging** — cross-platform builds via GitHub Actions; NDI packaging per
  ARCHITECTURE.md.
- **Timecode** — real LTC/VITC extraction; not started (TestSource fakes wall-clock TC).
- **Scheduler engine** — runs on the controller and sends ordinary start/stop commands
  (with inline outputs) at the scheduled times. Nodes stay stateless about schedules.
- **Higher-fps thumbnails via a subscription WebSocket** — replaces request-per-frame
  thumbnails, which don't scale past ~10–15 fps with more than a handful of feeds.
  Design in ARCHITECTURE.md (Thumbnails).
- **Long recordings:** the MOV/MP4 crash-safety reserve covers 2 h; past that the index
  goes at the end of the file as before. For all-day recordings, consider segmenting
  (`splitmuxsink`) or a larger reserve.
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
