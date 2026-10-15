# Capture Room — Roadmap

Active sequencing of work, decisions, and rationale. This complements
[ARCHITECTURE.md](ARCHITECTURE.md) (the design spec) — when the two disagree on
*order*, this file wins; ARCHITECTURE.md remains the source of truth for *design*.

_Last updated: 2026-10-15_

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
9. **Playback** — ✅ gapless playlists; NDI, SRT, RTSP, WHEP and display outputs; play
   without cueing; renames without a restart. Decklink / AJA outputs wait for hardware
10. **Clock sync across nodes** — ✅ shared node clock (controller's clock or PTP),
    synchronized start, and frame capture times as MOV timecode. Prerequisite for
    multi-angle replay
11. ✅ **Setup page** — per-node permission and system checks, with shortcuts to fix them
12. **Instant replay**
13. **Follow-on**, in rough priority order (see below)

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
- **Source types (2026-10-06)** — three live source types, all wrapped in `fallbacksrc`
  so a dropout plays black and silence, retries, and keeps recording, with a link state
  on Sources rows and Record tiles. **Stream**: one URL type for RTSP, SRT
  (caller/listener), RTMP pull, HLS/HTTP and UDP MPEG-TS. **Device**: capture cards and
  webcams, configured from a device picker. **WHIP** ingest from OBS and browsers.
  **Audio pairing**: any of them can take its audio from an audio device on the node
  (picking input channels) or play silence. Missing GStreamer elements make a type
  unavailable instead of stopping the node. Video device capture is untested on real
  hardware so far (OBS Virtual Camera goes live through the same AVFoundation path a
  USB HDMI dongle uses). A dongle's own audio input is paired automatically when its
  video device is picked, and macOS privacy denials (camera, microphone, screen
  recording) fail the source with how to fix it instead of leaving it connecting.
  Known issue: `avfvideosrc` opens a device by index only, and `fallbacksrc` restarts
  the same element, so after another device is unplugged a retry can open the wrong
  camera; re-resolve the index on restart once there's a dongle to test with.
- **9. Playback, milestone 1 (2026-10-06)** — playout channels are configured sources
  whose program bin (compositor over black, mixer over silence, at a fixed format) runs
  through the ordinary monitor, so a channel shows in Record and can be recorded. A clip
  plays in its own player pipeline pushing into per-clip lanes; transport is cue, play,
  pause, seek, stop, with in/out points and hold / black / loop at the out point
  (frame-accurate, gapless loops). NDI outputs are separate consumer pipelines. A media
  library per node (import from the file browser or from Recordings), and a Playback
  workspace. Design in ARCHITECTURE.md (Playback).
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
  display / HDMI out → WHEP (all shipped) → Decklink / AJA.
- Reuses file decoding from the looping media file source (#6).
- **Playback workspace** in the UI.
- ✅ One NDI channel playing a single file (milestone 1, 2026-10-06).
- ✅ **Gapless playlists (2026-10-07)** — a stored playlist per channel; each item
  plays the next one, holds, goes to black or loops at its out point; the next item is
  cued in the background and takes over at the frame after the last one; Next takes it
  by hand; the playlist can loop. A changed channel (rename, new format) keeps its
  transport, so what's on air plays on through the program restart.
- ✅ **SRT output (2026-10-07)** — the `OutputSink` trait, and SRT as its second type:
  H.264/AAC in MPEG-TS, listener or caller, with bitrate, latency and passphrase (fixed
  live encode settings otherwise; a stream preset UI can come if it's needed). Listeners
  show how many receivers are connected. Channels no longer need NDI to run.
- ✅ **RTSP output (2026-10-07)** — the node serves each RTSP output as a mount on its
  RTSP server (port 8554), `rtsp://node:8554/<channel name>`; encoded only while someone
  watches, one encode shared by every viewer. Links `libgstrtspserver` (a new required
  package on Linux). Next: local display / HDMI out.
- 🚧 **Display output (2026-10-11, macOS first)** — a channel fullscreen on a screen
  attached to the node (a monitor, or an HDMI→SDI converter), its audio to an audio
  output (the screen's own HDMI audio, so a converter embeds it). A winit window loop on
  the main thread, `glimagesink` drawing into it; when the screen has the channel's
  exact mode (or double its rate) it goes exclusive fullscreen in it, otherwise it
  fills the screen and scales. On macOS the mode is set for the process only (put back
  when the output closes) and a borderless window covers the screen above the menu bar:
  winit's exclusive fullscreen leaves its window under the display-capture shield.
  Screens are woken before they're listed or opened and kept awake while an output shows
  (a sleeping screen isn't listed, and an output on one goes dark). Per output, an idle
  channel shows black (the default; converters need a steady signal) or hides the window
  to show the desktop. Tried on an external monitor, and its screen id survived a
  replug. Still to do: the HDMI→SDI converter; screen sleep is only handled on macOS.
- **Channels are set up in Playback (2026-10-11)** — New channel and Channel settings
  (format, outputs, delete) open a sheet there; Sources still lists channels (they can
  be recorded) and its edit button goes to Playback.
- ✅ **Play without cueing (2026-10-11)** — the next-up item is always loaded in
  standby, idle too, so Play starts it at once; double-click, Enter or a row's play
  button plays any item now, straight on from what's on air; Next always plays. Cue is
  now Show first frame, and no longer goes black while it loads.
- ✅ **Renames without a restart (2026-10-11)** — names are out of every source's
  fingerprint; a rescan renames the source it keeps. Only outputs named after the
  channel (NDI name, RTSP or WHEP path left unset) restart, and recordings carry on.
  Stream, Device and WHIP sources no longer keep showing their old name after a rename.
- ✅ **WHEP output (2026-10-11)** — WebRTC on the node's API port,
  `http://node:<port>/whep/<path>`, proxied to each output's `whepserversink` on
  localhost, with CORS so web players elsewhere can connect. Encoded per viewer (H.264
  or VP8, Opus stereo) with the bitrate adapted to each link; viewers counted. Watch in
  Playback plays it in the browser. Needs the libnice GStreamer plugin
  (`libnice-gstreamer` on macOS), like WHIP.
- ✅ **Channels in capacity and the benchmark (2026-10-14)** — a benchmark can measure
  playout channels: each feed is a real channel at the footage's format playing it on
  loop, with NDI (numbered names) and SRT (listening on a free local port) outputs, or
  the program alone. RTSP and WHEP (encoded only per viewer) and display outputs are
  left out of the measurement and of a channel's key. Running channels, idle or playing,
  add to a node's load beside recordings and replay buffers. Clip frames that reach the
  program too late to show count as dropped. On the 8 GB dev Mac (1080p30, NDI + SRT)
  3 channels kept up with nothing dropped, and memory stopped the run at 4.
- ✅ **Capacity check on saving a channel (2026-10-14)** — saving a new channel or Channel
  settings in Playback checks the node's capacity (`POST /capacity/check` with
  `channel`, leaving out the channel's own load before the change) and warns, never
  blocks, past 80%, as Record and Replay › Buffers do.

## 10. Clock sync across nodes

Design in ARCHITECTURE.md (Timecode › Clock sync).

- ✅ **Shared node clock (2026-10-07).** Every pipeline runs on the node clock. A
  controller serves its clock (UDP 7800 by default) and claims its nodes with every
  health check; nodes follow it (GStreamer net clock, tens of µs on a wired LAN) or, in
  PTP mode, a PTP grandmaster. First controller wins; busy sources switch once idle.
  Clock status on each node card; mode in Settings.
- ✅ **Synchronized start (2026-10-07).** A start can carry a time on the shared clock;
  legs begin at the first frame due at or after it. Bulk Record sets one 1.5 s ahead and
  warns about feeds on another clock. Every session records its clock domain and first
  frame time, so recordings can be lined up later.
- ✅ **Frame capture times (2026-10-07).** Each domain maps its clock to UTC (the
  controller's wall clock, sent with its claims; TAI − 37 s on PTP). MOV recordings carry
  each frame's capture time as time-of-day timecode (drop-frame at 29.97), so files from
  every node line up by timecode in an NLE; sessions keep their first frame's UTC time,
  shown in Recordings. MP4 and MKV get no timecode track (their muxers write none).
- PTP is untested against a real grandmaster: try it on ACC's Dante network, and check
  the helper's permissions on macOS and Windows builds.

## 11. Setup page

Per-node permission and system checks in one place, so it's obvious why a feature
won't work on a machine and how to fix it. Design in ARCHITECTURE.md (Deployment ›
Setup checks).

- ✅ **Setup › Checks (2026-10-11, macOS first)** — each node checks live: macOS Screen
  Recording, Camera and Microphone (naming the app that holds them, found from the
  node's parent processes), the NDI Runtime, the default recording folder (writable,
  room to record), and the shared clock (the controller's, or PTP). A check is a
  *problem* only when a source on the node needs it (or it's essential); the rest only
  matter once the feature is used. A controller probes each node's TCP ports (API,
  WHIP, RTSP) from outside; UDP ports are listed but can't be tested. Shortcuts: Open
  System Settings and Ask macOS (shows the camera or microphone prompt; macOS never re-asks, and
  shows no prompt for screen recording) when the browser is on that machine,
  a command to copy, or a download link. Node cards badge their problems, linking to the
  node on Checks; the page re-checks when the window regains focus. Linux and Windows
  only get the cross-platform checks so far (no `video` group, firewall or PTP-helper
  capability checks yet).

- **Checked live, not recorded:** the OS is the source of truth (permissions get
  revoked, machines reinstalled), so each check reads the real state: ok, missing or
  unknown.
- **Per node, rolled up on the controller:** each node runs its own checks; the Nodes
  page badges any node with a problem.
- **Shortcuts help grant; they can't grant.** A button opening the right System
  Settings pane (macOS, only when the browser is on that machine), otherwise a command
  to copy (`setcap …`, `New-NetFirewallRule …`). The page says which app holds a macOS
  permission (Terminal when run from a shell).
- **Reachability tested from outside:** a node can't see its own firewall, so the
  controller probes each node's HTTP, clock and WHIP ports.
- **Each feature adds its own checks.** First set: macOS Screen Recording, Camera and
  Microphone; NDI Runtime loads; PTP helper capabilities (Linux); clock / API / WHIP
  ports reachable from the controller; storage writable.

## 12. Instant replay

Builds on playback (#9) and clock sync (#10). Design in ARCHITECTURE.md (Instant Replay).

- ✅ **Prototype (2026-10-11)** — `node/examples/replay_spike.rs` (removed after 4ced203): a 1080p59.94 source
  into 1 s all-intra segments, played back like a channel's player. GStreamer's own
  reverse, slow motion (to 0.25×), frame steps and accurate seeks are all frame-exact
  and fast, and clips export by remuxing on any frame. Two catches: decode in software
  (VideoToolbox drops frames in reverse), and read only closed segments, by list.
  Findings in ARCHITECTURE.md (Instant Replay › What the prototype showed).
- ✅ **Replay buffers on a channel (2026-10-11)** — any source can keep a replay buffer
  (`PUT /replay/{source}`): ProRes Proxy, intra H.264 or Motion JPEG, the operator's
  choice, kept for N minutes. A channel plays a span of it (`POST
  /channels/{id}/replay`) at any speed from −4× to 4×, backwards included, steps a frame
  either way, and changes speed on the fly; sound at 1× only. API only so far. Design in
  ARCHITECTURE.md (Instant Replay).
- ✅ **Replay workspace (2026-10-11)** — angles beside the program, a timeline of the
  selected angle's buffer up to now, replay the last N seconds, switch angle at the same
  moment, speed and reverse, frame steps, in/out marks, all on the keyboard; a Buffers
  sheet for turning buffers on. A step now waits for its frame rather than a state
  change (it used to take 5 s).
- ✅ **Clip list and export (2026-10-14)** — S saves in → out as a clip (Shift+S on
  every angle): the segments covering it become the clip's own files (hard links, so
  instant and free), kept after the buffer rolls on and across restarts. A clip list in
  the Replay workspace cues, plays at the chosen speed, renames and deletes them. Export
  remuxes a clip, frame-exact with its sound, into one .mov in `~/capture-room/Replay
  clips/` and adds it to the media library. Fixed on the way: replacing a channel's
  lanes could fail its program (`not-linked`) when loads came quickly.
- ✅ **Replay buffers in capacity and the benchmark (2026-10-14)** — a benchmark can
  measure replay buffers of a codec instead of a preset's recording; running buffers add
  to a node's load beside its recordings, and turning one on in Replay › Buffers checks
  and warns like Record. Benchmarks now refuse to start while buffers run, and starting
  one cancels a benchmark. Fixed on the way: a buffer failed (qtmux, "timestamps are
  drifting") when its source's sound jumped, as a looping file's does at each loop. And
  memory guards a run: a ProRes buffer run climbed past 24 feeds and froze an 8 GB Mac,
  since a hardware encoder never drops frames; memory is now watched as feeds start and
  run, and the search won't try a count that wouldn't fit. On that Mac (already
  swapping) a ProRes run stops at 2 feeds, for memory.
- ✅ **Replay codecs compared (2026-10-14)** — on real footage, scored by VMAF as a
  channel shows it (`node/examples/replay_quality.sh`): intra H.264 now records at
  `vtenc_h264` quality 0.75, ProRes Proxy's picture at about 60% of its size (20–31 GB
  an hour at 1080p59.94 against 38–40); 0.85 had been ProRes's size. Fixed on the way:
  the x264 fallback was capped at 2 Mbit/s (VMAF 32–42), and MJPEG replays showed
  lifted blacks and grey whites (stored as the wrong range). Results in ARCHITECTURE.md
  (Instant Replay › What the comparison showed). Benchmark results for intra H.264 and
  MJPEG buffers predate the change: re-run them.
- ✅ **NDI subscriptions (2026-10-15)** — so a replay node takes in only the angles it
  buffers: each node connects to every NDI sender it finds (as before) or, set per node
  in Setup › Sources › NDI senders, only to chosen ones; a sender a recording or replay
  buffer uses stays connected. No node connects to its own NDI outputs any more, and
  each receives as `capture-room (<node name>)`. Setup › Checks suggests it on a node
  connected to more than 4 senders. Tried with two nodes on one Mac.
- ✅ **NDI timing across nodes (2026-10-15)** — two receivers of one sender agree
  on each frame's time to within 2 ms once settled (2 s after connecting), on
  `ndisrc`'s default `auto` mode, so nothing changed
  (`node/examples/ndi_timing.rs`; ARCHITECTURE.md, Instant Replay › What the NDI
  timing showed). Measured on one machine: re-run across two when a second Mac runs a
  node. Sources marks a sender several nodes have ("Also on"), and Replay › Buffers
  warns when another node already buffers it.
- **Next:** clips from several angles grouped as one, downloading an export from the
  browser, MOV timecode on exports.

## 13. Follow-on

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
  order. Stream, Device, WHIP and audio pairing shipped (see Shipped).
  1. **Linux screen capture** — X11 `ximagesrc`; Wayland through an xdg-desktop-portal
     ScreenCast session (`ashpd`, `persist_mode=2`, restore token kept in the source
     config) into `pipewiresrc`. Offered as entries in the device list. macOS and Windows
     already list screens as devices.
  2. **SDI via Decklink** — already specced; needs hardware.
  3. **SDI via AJA** (`ajasrc`, gst-plugins-bad ≥ 1.24) — alongside Decklink; needs
     hardware.
  4. **Web page** (`wpesrc`) — render an HTML page (graphics, scoreboards) as a feed.
     Niche; WPE is Linux-only.
- **macOS and Windows CI jobs** — build + clippy with GStreamer from brew / the official
  MSI, so the platform-specific device code can't rot. Device and network tests stay
  Linux-only.
