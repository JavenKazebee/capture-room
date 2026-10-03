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
    fn connect(&mut self) -> Result<()>;
    fn disconnect(&mut self);
    fn gst_src_element(&self) -> gst::Element;
    fn timecode(&self) -> Option<Timecode>;
    fn is_available(&self) -> bool;
}
```

Initial implementations:
- `TestSource` — ✅ implemented — `videotestsrc` + `audiotestsrc`, the reference pattern for all sources (`gst::Bin` with `"video"` / `"audio"` ghost pads)
- `NdiSource` — ⬜ in progress — built on the `gst-plugin-ndi` GStreamer elements (`ndisrc` + `ndisrcdemux`), not the raw NDI SDK FFI. Discovery via `ndi-device-monitor`. Follows the same bin/ghost-pad contract as `TestSource`.
- `DecklinkSource` — ⬜ deferred (no hardware) — Decklink SDK via FFI / `decklinkvideosrc`

---

## GStreamer Pipeline

One pipeline per active source. The monitor branches (thumbnail, audio meter) are always
running once a source is connected. Recording branches attach and detach at runtime.

```
[InputSource gst src element]
    │
    ├─► [timecode extractor]
    │
    ├─► [vtee]  ── always-on monitor ──────────────────────────────────────────────────
    │      ├─► [queue] → [videoscale] → [capsfilter fps] → [capsfilter res]
    │      │             → [jpegenc] → ThumbnailStore → GET /api/v1/thumbnails/{id}
    │      │
    │      └─► [queue, leaky=upstream] → [encoder] → [muxer] → [filesink]  (recording branch, detachable)
    │
    └─► [atee]  ── always-on monitor ──────────────────────────────────────────────────
           ├─► [queue] → [audioconvert] → [level] → AudioMeter → WS audio.levels
           │
           └─► [queue] → [muxer] → [filesink]  (recording branch, detachable)
```

The video recording queue uses `leaky=upstream` so that a slow encoder drops frames
rather than stalling vtee and backpressuring through the muxer's collect-pads (which
would freeze audio monitoring). A separate large audio queue buffers up to 10 s so the
muxer can interleave audio and video without blocking atee.

If the redundant path uses a different profile than primary, it gets its own encoder branch (same topology). If the profiles are identical, the encoded bitstream is split via `tee` after a single encoder, saving a full re-encode.

Supported encoder targets:
- **Ingest:** ProRes (4444, 422 HQ, 422, LT, Proxy), DNxHD/DNxHR, uncompressed
- **Delivery/proxy:** H.264, H.265/HEVC, VP9
- **Containers:** MOV, MXF, MP4, MKV

The redundant path writes the same profile as primary to a second filesystem path on the same machine.

---

## Thumbnails

The thumbnail branch of the GStreamer pipeline generates JPEG frames at a configurable rate (default **1 fps**) regardless of source framerate. Rate is set per recording preset and applies to the preview thumbnail only — it has no effect on encoded output.

The latest JPEG is held in memory and served from `GET /api/v1/thumbnails/{source_id}`. A `thumbnail.updated` WebSocket event is emitted each time a new frame is ready.

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
| `source.available` / `source.lost` | source id, name |
| `recording.started` / `recording.stopped` / `recording.error` | session id, source id |
| `feed.status` | source id, timecode, duration (1 Hz) |
| `audio.levels` | source id, channel peak/RMS values (~10fps) |
| `thumbnail.updated` | source id, URL |
| `log` | level, message, timestamp |
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
| **Recordings** | Start/stop recordings, assign presets, view active sessions |
| **Presets** | Create and edit recording presets |
| **Nodes** | Add/remove nodes, view health, run benchmarks |
| **Schedules** | Create, edit, and view upcoming scheduled recordings |
| **Logs** | Aggregated log viewer with filter by node and level |

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
│   │   │   └── types.rs         # DTOs (exported to TS)
│   │   ├── controller/
│   │   │   ├── mod.rs           # Controller enable/disable (live toggle)
│   │   │   ├── api.rs           # /nodes, /controller, /presets, merged /ws
│   │   │   ├── forward.rs       # /nodes/{id}/… → node API (in-process or HTTP)
│   │   │   ├── discovery.rs     # mDNS advertise/browse + health polling
│   │   │   ├── relay.rs         # peer WS → merged /ws
│   │   │   └── registry.rs      # NodeRegistry
│   │   ├── storage.rs           # Storage volume listing
│   │   ├── pipeline/
│   │   │   ├── monitor.rs       # MonitorPipeline, MonitorConfig, RecordingBranch, ThumbnailStore, AudioMeter
│   │   │   ├── profile.rs       # RecordingProfile (codec, container, bitrate, …)
│   │   │   └── mod.rs           # GStreamer element helpers
│   │   ├── sources/
│   │   │   ├── mod.rs           # InputSource trait + SourceType + ConnectionMode enums
│   │   │   ├── manager.rs       # SourceManager — per-source monitors + recording sessions
│   │   │   ├── registry.rs      # SourceRegistry — discovered sources indexed by id
│   │   │   ├── test.rs          # TestSource + TestSourceConfig
│   │   │   ├── ndi.rs           # ⬜ NdiSource (stub)
│   │   │   └── decklink.rs      # ⬜ DecklinkSource (stub)
│   │   ├── recording/           # DB persistence helpers for recording sessions
│   │   ├── timecode/            # ⬜ LTC/VITC extraction (stub)
│   │   ├── benchmark/           # ⬜ Benchmark runner (stub)
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

Status legend: ✅ done · 🟡 partial · ⬜ not started · _(as of 2026-06-23)_

1. ✅ **Monorepo scaffold** — workspace config, root scripts, `pnpm dev` wired up
2. ✅ **UI scaffold** — shadcn-vue init, routing, empty views, Pinia stores, WebSocket composable
3. ✅ **Rust — TestSource + InputSource trait** — unblocks all pipeline work without hardware
4. ✅ **Rust — GStreamer pipeline** — single source, single output, no tee
5. ✅ **Rust — multi-output tee, thumbnail, audio metering**
6. ✅ **Rust — node-mode REST + WebSocket API**, `ts-rs` type export
   - _Note: `ts-rs` is wired but `ui/src/types/generated/` is currently empty — run `pnpm types` to populate._
7. ✅ **UI — dashboard with live feed grid, manual recording controls** — `DashboardView.vue` built
8. ✅ **Rust — controller mode** — node registry, health polling, unified API, UI serving
9. 🟡 **UI — nodes view, preset management, schedules**
   - ✅ `NodesView.vue`, ✅ `PresetsView.vue`, ✅ `SourcesView.vue`
   - ⬜ `RecordingsView`, `SchedulesView`, `LogsView` are still "coming soon" placeholders
10. ✅ **Rust — SourceManager + live source monitoring** (ROADMAP step 2)
    - `MonitorPipeline` (`pipeline/monitor.rs`): always-on vtee/atee → thumbnail + audio meter; recording attaches as a detachable branch
    - `SourceManager` (`sources/manager.rs`): owns per-source pipelines and recording sessions behind a single `RwLock`
    - Live settings reconfiguration via `MonitorPipeline::reconfigure()` — no pipeline restart
    - Two-phase `begin_stop_recording` + `Arc<MonitorPipeline>` keeps WS emitter unblocked during EOS drain
    - Leaky video recording queue prevents encoder backpressure from freezing audio monitoring
11. 🟡 **Rust — NDI implementation** (NDI hardware on hand) ← active
    - ⬜ Prereq: install `gst-plugin-ndi` (`ndisrc` / `ndisrcdemux`) — not present on dev machine
    - ⬜ `NdiSource` impl + device discovery in `SourceRegistry::scan()`

> **Sequencing note:** the v1 build order above is the original plan. Active work is now
> sequenced in [ROADMAP.md](ROADMAP.md), which front-loads a generic TestSource, the Sources
> view, and live source monitoring ahead of NDI capture.
12. ⬜ **Rust — Decklink implementation** — deferred (no hardware)
13. 🟡 **Rust — preset sync + scheduling engine**
    - ✅ Preset sync (`controller/sync.rs`); ⬜ scheduler engine (`controller/scheduler.rs` is an empty stub)
14. ⬜ **Rust — benchmark runner** — empty stub (`benchmark/mod.rs`)
15. ⬜ **Rust — timecode** — empty stub (`timecode/mod.rs`); TestSource fakes a wall-clock TC
16. ⬜ **Rust — redundant recording path**
17. ⬜ **Cross-platform packaging + GitHub Actions**
