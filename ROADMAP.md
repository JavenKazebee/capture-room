# Capture Room — Roadmap

Active sequencing of work, decisions, and rationale. This complements
[ARCHITECTURE.md](ARCHITECTURE.md) (the design spec) — when the two disagree on
*order*, this file wins; ARCHITECTURE.md remains the source of truth for *design*.

_Last updated: 2026-10-03_

---

## Current sequence

1. ✅ **Generic TestSource + Sources view**
2. ✅ **Live source monitoring**
3. ✅ **NDI capture** (+ plugin build/packaging)
4. ✅ **Multi-pipeline output per preset**
5. **Benchmark + capacity estimator**
6. **UI overhaul / dark mode** — woven through 1–5; design-token pass up front
7. **Follow-on:** scheduler engine, higher-fps thumbnails, timecode, packaging/CI, additional source types

Rationale for front-loading 1–2 ahead of NDI: a configurable TestSource plus the
Sources view gives a real authoring/verification surface, and live monitoring forces
the source-lifecycle design (connect-on-discovery, not connect-on-record) that NDI
capture depends on anyway.

---

## ✅ Node / controller simplification (2026-10-02)

Replaced the node/aggregator role split. Every instance is a node with a local-only
API (`/api/v1/node/…`); the controller is a live toggle that adds discovery, a generic
`/api/v1/nodes/{id}/…` forwarder and a merged event stream. Presets are sent inline
with start commands, so preset sync and composite source ids are gone. Nodes also
report their storage volumes.

## 1. ✅ Generic TestSource + Sources view

Turn `TestSource` from a fixed `videotestsrc`/`audiotestsrc` into a first-class,
parameterized source.

- **Video:** selectable `videotestsrc` pattern (SMPTE bars, ball, snow, …),
  resolution, fps, format.
- **Audio:** tone / silence / pink noise, frequency, channel count.
- **Plumbing:** a `TestSourceConfig` struct exposed via source config so the UI can
  author test feeds.
- **UI:** build out `SourcesView.vue` (placeholder today) — per-node source list,
  connect/disconnect, capabilities, and test-source authoring.
- Keep TestSource discoverable behind a `--dev` / config flag once NDI discovery lands.

Touches: `node/src/sources/test.rs`, `node/src/sources/manager.rs`, `ui/src/views/SourcesView.vue`.

## 2. ✅ Live source monitoring

**Biggest architectural change.** `ThumbnailStore` and `AudioMeter` are now owned by
`MonitorPipeline` (`node/src/pipeline/monitor.rs`), so thumbnails and meters are always
live — no recording required.

- **Monitor pipeline per source.** `source bin → vtee + atee → [thumbnail branch] + [audio meter branch] + [video/audio StreamProducers]`. Recordings consume the producers from their own pipelines (see #4), so they never touch the monitor's tees.
- **`SourceManager`** (`node/src/sources/manager.rs`) owns the sources, their `MonitorPipeline`s, and active recording sessions — single source of truth for all capture state.
- **Live settings reconfiguration.** `MonitorPipeline::reconfigure()` updates GStreamer element properties in place (capsfilter caps, level interval) without stopping pipelines — applies immediately during recording.
- **Non-blocking stop recording.** `begin_stop_recording` takes the session out of the manager under the write lock and hands back a `StopJob`; its legs drain to EOS on a spawned task (`node/src/session.rs`). The WS emitter (which holds a read lock every 100 ms) is never blocked during the multi-second drain.

Touches: `node/src/pipeline/monitor.rs`, `node/src/sources/manager.rs`, `node/src/session.rs`, `node/src/api/node.rs`, `DashboardView.vue`.

## 3. ✅ NDI capture

Built on `gst-plugin-ndi` GStreamer elements (`ndisrc` / `ndisrcdemux`), statically
linked into the binary. Follows the same source bin + `video`/`audio` ghost-pad contract
as `TestSource`. Decklink remains deferred (no hardware).

### What shipped

- **`NdiSource`** (`node/src/sources/ndi.rs`) — bin with `ndisrc → ndisrcdemux`, dynamic
  pad-added linking to `videoconvert`/`audioconvert`, ghost pads `video` and `audio`.
- **`NdiMonitor`** — persistent `GstDeviceMonitor` started once at startup via
  `spawn_blocking`. Avoids the NDI device provider singleton bug where `stop()`/`start()`
  cycles silently no-op (the provider never clears its internal `FindInstance`).
- **Static linking** — `gst-plugin-ndi` compiled as an rlib into the binary
  (`plugin_register_static()` called at startup). No `libgstndi.so` to manage;
  only `libndi.so` (the proprietary NDI runtime) remains as a user dependency.
- **Recording fix** — recording legs run `videoconvert` (and `audioconvert` +
  `audioresample`) ahead of the encoders. NDI provides UYVY; x264enc needs I420. Without
  the converter, x264enc's RECONFIGURE event propagated upstream to ndisrc and caused an
  "Internal data stream error" immediately on recording start.

### Packaging / licensing

`gst-plugin-ndi` is MPL-licensed and may be bundled. It dlopen's `libndi.so` at runtime;
users must install the NDI Runtime separately (same pattern as OBS). We never distribute
`libndi` itself.

| Dependency | Who provides |
|------------|-------------|
| `gst-plugin-ndi` | compiled into binary (static) |
| `libndi.so` | user installs NDI Runtime redistributable |

## 4. ✅ Multi-pipeline output per preset

Replaced the fixed primary / secondary / redundant output templates with **N output legs**
per preset (e.g. H.264 MP4 to one path + ProRes MOV to another). A redundant copy is just
another leg.

### What shipped

- **Schema:** `preset_outputs` table (name, codec, container, resolution, framerate,
  bitrate, chroma, path_template, sort_order). Codec, container and chroma are closed
  enums shared with the UI through ts-rs.
- **One pipeline per leg.** Each leg is its own GStreamer pipeline fed by the monitor's
  `StreamProducer`s (`node/src/pipeline/recording.rs`). A tee would pass a failing leg's
  flow error back to the source and stop everything; a producer only logs it, so a bad
  leg (codec the container rejects, full disk) fails alone.
- **Failure handling (2026-10-03):** a failed leg is reported immediately
  (`recording.leg_failed`, shown on the feed card) while the other legs keep recording;
  if every leg fails the session stops itself. A failed source monitor is restarted
  automatically (rescan every 5 s while one has failed), ending its recordings with
  `source failed: …`.
- **Start/stop:** all legs are built before any starts, so config errors open no files;
  a failed start rolls back and deletes partial files. Stop drains every leg to EOS
  concurrently with a timeout.
- **UI:** the Presets view is a list-of-outputs editor; nodes keep no preset store —
  outputs are sent inline with each start command.

### Not done

- Encode sharing: legs with identical profiles each run their own encoder.

## 5. Benchmark + capacity estimator

Not started (the empty `benchmark/` stub was removed).

- Synthetic pipelines at increasing feed counts → measure **dropped frames, CPU, disk
  write throughput, memory pressure**. Stop past a dropped-frame threshold.
- Persist runs in `benchmark_results` (table already specced in ARCHITECTURE.md).
- **Capacity estimator:** given a preset (incl. multi-leg) and feed count, predict
  sustainability from stored benchmarks — surfaced in the Nodes view.
- Sequenced after #4 so benchmarks reflect realistic multi-leg presets.

## 6. UI overhaul / dark mode

Staying on shadcn-vue (reka-ui, Tailwind v4), plus `vue-sonner` for toasts. Tables use a
small in-house `DataTable` (sort, filter, column visibility, grouping) rather than
TanStack Table, whose v9 API is brand new and more than these tables need.

- **Design system** (done): dark-first OKLCH tokens in `ui/src/style.css`, light kept
  in step, on slightly blue-gray tinted surfaces. `--primary` is sky-cyan (actions,
  toggles, focus, selection, links). Solid red means "live": `--tally` is recording
  only, `--destructive` a soft treatment behind a confirm, and `--brand` red is just
  the logo's record light. Self-hosted Inter +
  JetBrains Mono; `num` utility for technical values. Density = root font size.
- **Preferences** (done): `usePreferences()` — theme, density, Multiview tile size and
  overlays, inspector, table columns; per browser, surfaced in the view they affect.
- **Workspace shell** (done): the tool will grow into recording, playback and instant
  replay, so the shell is organized around workspaces (operating desks) rather than
  pages. A top header holds identity, labeled workspace tabs and global status
  (connection, nodes, lowest free storage, REC count, clock). Each workspace has a
  toolbar row and its own panel layout. A tabbed bottom panel (Log / Recordings /
  Storage, Ctrl+J) is shared. Today: **Record** and **Setup** (Sources, Presets, Nodes,
  Settings behind a labeled side list). **Replay** and **Playback** join when the
  backend supports them — not before. ⌘K palette and toasts throughout.
- **Shared patterns** (done): `PageHeader`, `ConfirmDialog`, `EditSheet` (replaces
  `FormModal`), `StatusDot`, `TallyBadge`, `KeyValueList`, `CopyButton`, `DataTable` +
  `ColumnsMenu`.
- **Views, one per pass:** ✅ Record workspace (filters, overlay toggles, tile size,
  Ctrl/Shift multi-select with bulk Record/Stop behind a confirm, per-feed preset
  remembered per browser, resizable inspector with outputs, session and history) →
  ✅ Sources (DataTable grouped by node, column chooser, status incl. recording, edit in a
  side sheet, confirmed delete, scan results as a toast, jump to Record) → Presets
  (master-detail editor, path-template preview) → Nodes (cards, room for capacity
  panel) → Settings (appearance, monitoring).
- Recordings, Schedules and Logs views are kept out of the nav until they exist.

## 7. Follow-on

- **Scheduler engine** — runs on the controller and sends ordinary start/stop commands (with inline outputs) at the scheduled times. Nodes stay stateless about schedules.
- **Higher-fps thumbnails via a subscription WebSocket** — today each thumbnail is a
  `thumbnail.updated` event plus an HTTP GET (forwarded and buffered by the controller
  for peers), capped at 10 fps by the 100 ms emitter tick. Request-per-frame doesn't
  scale past ~10–15 fps with more than a handful of feeds (browsers allow 6 HTTP/1.1
  connections per host, and every frame is a round trip through the controller).
  Replace it with a dedicated thumbnail WebSocket:
  - The client sends a subscription: which sources are on screen and at what fps.
    Frames are pushed as binary messages (source key + JPEG), driven by the appsink
    callback rather than the emitter tick. Unsubscribed or off-screen feeds cost nothing.
  - On a controller, subscriptions are relayed to each peer for only the sources someone
    is watching; peer frames are forwarded as they arrive.
  - Makes ~25–30 fps practical (source-rate thumbnails, 29.97 included) and removes
    `thumbnail.updated`, the UI's `thumbnailSeqs` cache-busting, and the thumbnail part
    of the periodic emitter. Keep `GET /thumbnails/{id}` for one-off snapshots.
  - Not MJPEG (`multipart/x-mixed-replace`): each open stream holds one of the
    browser's 6 connections, so 7+ feeds would stall the page.
  - Possible later complement: a click-to-watch full-size monitor for one feed using
    real video (WebRTC, or H.264 over this WebSocket decoded with WebCodecs), with audio.
- **Timecode** — real LTC/VITC extraction; not started (TestSource fakes wall-clock TC).
- **Packaging + GitHub Actions** — cross-platform builds; folds in the NDI packaging strategy above.
- ✅ **Node registry persistence** — peers added by URL are stored in the `nodes` table and restored when the controller starts.
- **Additional source types** (each is a new `InputSource` impl, additive):
  - **RTSP** (`rtspsrc`) — IP cameras; easy, high value.
  - **SRT** (`srtsrc`) — contribution feeds over unreliable networks.
  - **HDMI/USB capture** (`v4l2src`).
  - **SDI via Decklink** — already specced; needs hardware.
