# Capture Room — Architecture

A multi-feed video capture and recording platform for live broadcast environments, built to run on a single machine or a cluster of up to ~20 nodes managed from a single web UI.

---

## Design Principles

- **One binary, every instance is a node.** A node exposes everything its own machine has: sources, storage volumes, recordings, thumbnails, monitor settings. It knows nothing about other nodes.
- **Controller is a toggle, not a mode.** Any node can be promoted to controller (live, no restart). A controller keeps every node capability and additionally discovers other nodes, forwards commands to them, and merges their event streams.
- **Controllers send plain commands.** Settings changes and start/stop. A start command carries the full output settings inline, so nodes keep no preset store and nothing needs syncing.
- **One addressing scheme.** The UI always talks to `/api/v1/nodes/{node_id}/…`; on a non-controller that list contains only the node itself.
- **Input sources are pluggable from day one.** NDI and Decklink are the first implementations of a formal trait; adding a new source type is additive, not a refactor.
- **Types flow from Rust outward.** API types are defined once as Rust structs and exported to TypeScript via `ts-rs`.

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
    fn capabilities(&self) -> SourceCapabilities;
    /// Identifies the config the bin was built from; a rescan rebuilds the
    /// source (and restarts its monitor) only when this changes.
    fn fingerprint(&self) -> String;
    fn gst_src_element(&self) -> gst::Element;
    fn timecode(&self) -> Option<Timecode>;
}
```

Initial implementations:
- `TestSource` — ✅ implemented — `videotestsrc` + `audiotestsrc`, the reference pattern for all sources (`gst::Bin` with `"video"` / `"audio"` ghost pads)
- `NdiSource` — ✅ implemented — built on the `gst-plugin-ndi` GStreamer elements (`ndisrc` + `ndisrcdemux`), not the raw NDI SDK FFI. Discovery via a persistent `GstDeviceMonitor`. Follows the same bin/ghost-pad contract as `TestSource`.
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
[appsrc video] → [videorate → caps]? → [videoscale → caps]? → [videoconvert] → [caps chroma]?
               → [encoder] → [h264parse|h265parse]? ─┐
[appsrc audio] → [queue 10 s] → [audioconvert] → [audioresample] → [encoder] ─┴─► [muxer] → [filesink]
```

Why separate pipelines: a tee passes a branch's flow error back to the source, so a leg
that failed (a codec the container rejects at runtime, a full disk) used to stop the
whole monitor and every other leg. A producer only logs consumer errors, so a failing leg
now fails alone and its error is reported when the session stops.

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
- **Chroma:** H.264/H.265 legs encode the preset's chroma subsampling (default 4:2:0);
  otherwise the encoder would follow the source's format (e.g. 4:4:4 from test patterns).

Legs with identical profiles currently each run their own encoder.

Supported encoder targets:
- **Ingest:** ProRes (4444, 422 HQ, 422, LT, Proxy), uncompressed
- **Delivery/proxy:** H.264, H.265/HEVC, VP9
- **Containers:** MOV, MXF, MP4, MKV

Codecs and containers are closed enums (`VideoCodec`, `Container` in `api/types.rs`), so the API rejects anything else and the UI's choices are generated from Rust. A redundant copy is just another leg with the same profile and a different path.

---

## Thumbnails

The monitor pipeline's thumbnail branch generates JPEG frames at a configurable rate (default **1 fps**, 1–10) regardless of source framerate. Rate and size are node-wide monitor settings (`PUT /api/v1/node/settings`, edited on the Dashboard), applied live without restarting pipelines, and have no effect on encoded output.

The latest JPEG is held in memory and served from `GET /api/v1/node/thumbnails/{source_id}`. The periodic emitter sends `thumbnail.updated` at the configured rate and the UI re-fetches the image. The 10 fps ceiling comes from the emitter's 100 ms tick; a subscription WebSocket that pushes frames is planned (see ROADMAP.md).

---

## Timecode

- Reads LTC from a designated audio channel or VITC from the video signal via GStreamer timecode elements and the Decklink SDK timecode API
- Exposed per-source via the status WebSocket and REST
- Written into output file metadata where the container supports it (MOV, MXF)

---

## Benchmark Runner

Determines sustainable recording capacity for a given machine on demand:

1. Spins up synthetic GStreamer pipelines (`videotestsrc` / `audiotestsrc`) at increasing feed counts
2. Measures: dropped frames per pipeline, CPU usage, disk throughput, memory pressure
3. Stops when dropped frames exceed a configurable threshold (TBD — likely expressed as a percentage of frames over a rolling window)
4. Reports: max sustainable feed count at that profile, raw metrics per step
5. Stores results in local SQLite

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
| POST | `/sources/{id}/connect` · `/disconnect` | |
| GET / POST | `/test-sources` | test source configs |
| PUT / DELETE | `/test-sources/{id}` | |
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
| `recording.started` / `recording.stopped` / `recording.error` | session id, source id |
| `feed.status` | source id, timecode (1 Hz) |
| `audio.levels` | source id, channel peak/RMS values (~10fps) |
| `thumbnail.updated` | source id (at the configured thumbnail fps) |
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
- `test_sources` — test source configs
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

### Views

| View | Description |
|------|-------------|
| **Dashboard** | Feed grid — thumbnail, source name, timecode, recording state, audio meters, dropped frame indicator per source across all nodes |
| **Sources** | Per-node source list, connect/disconnect, capabilities |
| **Recordings** _(planned)_ | Session history, active sessions |
| **Presets** | Create and edit recording presets |
| **Nodes** | Add/remove nodes, view health and storage (benchmarks planned) |
| **Schedules** _(planned)_ | Create, edit, and view upcoming scheduled recordings |
| **Logs** _(planned)_ | Aggregated log viewer with filter by node and level |

Planned views stay out of the nav until they're built.

### Real-time State

A single WebSocket connection (`/ws`) feeds all reactive UI state via Pinia stores. Sources and sessions are keyed by `(node_id, id)`. Components subscribe to store slices; they don't manage WebSocket connections directly.

---

## File Naming

| Token | Value |
|-------|-------|
| `{date}` | `YYYY-MM-DD` |
| `{node}` | Node name |
| `{source}` | Source name |
| `{datetime}` | `YYYYMMDD_HHMMSS` |
| `{preset}` | Preset name |
| `{ext}` | Container file extension |

Default template:
```
/media/recordings/{date}/{node}/{source}_{datetime}_{preset}.{ext}
```

Example:
```
/media/recordings/2026-06-21/node-01/cam3_20260621_143022_prores_hq.mov
/media/recordings/2026-06-21/node-01/cam3_20260621_143022_h264_proxy.mp4
```

---

## Node Discovery

Every instance registers an mDNS service (`_capture-room._tcp.local.`). A controller browses for it, identifies each service via `GET /api/v1/node/status`, and adds it to its registry. mDNS peers are pruned after ~15 s of failed health checks and re-added when they re-announce. Peers added by URL are persisted and never pruned — they're just shown as unreachable.

Discovery is asymmetric: the controller initiates all connections; nodes never need to know a controller exists. Nothing is pushed on connect because nodes hold no controller state.

---

## Ports & Networking

One port per machine, configurable, default `7700`. Set via config file or `--port` flag.

Plain HTTP/WebSocket over LAN. No TLS required for v1 (trusted network assumed).

---

## Deployment

One binary, installed as a system service.

- **Linux:** systemd unit
- **macOS:** launchd plist
- **Windows:** Windows Service via `windows-service` crate

Config file:
- Linux/macOS: `/etc/capture-room/config.toml`
- Windows: `%APPDATA%\CaptureRoom\config.toml`

Cross-compiled for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `x86_64-apple-darwin` via GitHub Actions.

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
│   │   │   ├── mod.rs           # InputSource trait + SourceType
│   │   │   ├── manager.rs       # SourceManager — sources, per-source monitors, recording sessions
│   │   │   ├── test.rs          # TestSource
│   │   │   └── ndi.rs           # NdiSource + NDI device monitor
│   │   ├── session.rs           # Running stops/teardowns: drain legs, persist, broadcast
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

## Build Order for v1

Status legend: ✅ done · 🟡 partial · ⬜ not started · _(as of 2026-10-02)_

1. ✅ **Monorepo scaffold** — workspace config, root scripts, `pnpm dev` wired up
2. ✅ **UI scaffold** — shadcn-vue init, routing, empty views, Pinia stores, WebSocket composable
3. ✅ **Rust — TestSource + InputSource trait** — unblocks all pipeline work without hardware
4. ✅ **Rust — GStreamer pipeline** — single source, single output, no tee
5. ✅ **Rust — multi-output tee, thumbnail, audio metering**
6. ✅ **Rust — node-mode REST + WebSocket API**, `ts-rs` type export
7. ✅ **UI — dashboard with live feed grid, manual recording controls** — `DashboardView.vue` built
8. ✅ **Rust — controller mode** — node registry, health polling, unified API, UI serving
9. 🟡 **UI — nodes view, preset management, schedules**
   - ✅ `NodesView.vue`, ✅ `PresetsView.vue`, ✅ `SourcesView.vue`
   - ⬜ Recordings, Schedules and Logs views — not started (kept out of the nav)
10. ✅ **Rust — SourceManager + live source monitoring** (ROADMAP step 2)
    - `MonitorPipeline` (`pipeline/monitor.rs`): always-on vtee/atee → thumbnail + audio meter + StreamProducers; each recording leg is its own pipeline (`pipeline/recording.rs`)
    - `SourceManager` (`sources/manager.rs`): owns per-source pipelines and recording sessions behind a single `RwLock`
    - Live settings reconfiguration via `MonitorPipeline::reconfigure()` — no pipeline restart
    - Two-phase `begin_stop_recording`: the session leaves the manager under the lock, and its legs drain to EOS on a spawned task (`session.rs`), so the WS emitter is never blocked
    - Each consumer `appsrc` drops its oldest buffers when full, so a slow encoder drops frames in its own leg instead of stalling the monitor
11. ✅ **Rust — NDI implementation** — `gst-plugin-ndi` statically linked; `NdiSource` + persistent device monitor (`sources/ndi.rs`)

> **Sequencing note:** the v1 build order above is the original plan. Active work is now
> sequenced in [ROADMAP.md](ROADMAP.md), which front-loads a generic TestSource, the Sources
> view, and live source monitoring ahead of NDI capture.
12. ⬜ **Rust — Decklink implementation** — deferred (no hardware)
13. ⬜ **Rust — scheduling engine** — not started. (Preset sync was dropped: presets are sent inline with each start command.)
14. ⬜ **Rust — benchmark runner** — not started
15. ⬜ **Rust — timecode** — not started; TestSource fakes a wall-clock TC
16. ✅ **Rust — redundant recording path** — covered by multi-leg presets (same profile, second path)
17. ⬜ **Cross-platform packaging + GitHub Actions**
