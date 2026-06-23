# Capture Room — Architecture

A multi-feed video capture and recording platform for live broadcast environments, built to run on a single machine or a cluster of up to ~20 nodes managed from a single web UI.

---

## Design Principles

- **One binary, two modes.** The same Rust executable runs as a capture-only node or as a controller. No separate runtime to install or manage.
- **Nodes are autonomous.** A node records without a controller present. The controller is an orchestration layer, not a dependency.
- **Controller is source of truth for config.** Presets and schedules live on the controller and sync down to nodes, which cache them locally for offline operation.
- **Input sources are pluggable from day one.** NDI and Decklink are the first implementations of a formal trait; adding a new source type is additive, not a refactor.
- **Types flow from Rust outward.** API types are defined once as Rust structs and exported to TypeScript via `ts-rs`. No hand-maintained type mirrors.
- **API-first.** Every action the UI can take is available via the REST + WebSocket API.

---

## System Components

```
Browser (Vue 3 UI)
      │
      │  HTTP / WebSocket (:7700)
      ▼
┌──────────────────────────────────┐
│  Rust binary — controller mode   │
│  ─ Serves embedded Vue UI        │
│  ─ Node registry + health polls  │
│  ─ Unified API (proxies nodes)   │
│  ─ Preset management             │
│  ─ Scheduling engine             │
│  ─ WebSocket aggregation         │
│  ─ Log aggregation               │
│  ─ Local capture (if hw present) │
└──────────────┬───────────────────┘
               │  HTTP / WebSocket (:7700) per node
               ▼
┌─────────────────────────┐     ┌─────────────────────────┐
│  Rust binary — node mode │     │  Rust binary — node mode │  ...
│  ─ Input plugin system   │     │  ─ Input plugin system   │
│  ─ GStreamer pipelines   │     │  ─ GStreamer pipelines   │
│  ─ Recording sessions    │     │  ─ Recording sessions    │
│  ─ Timecode reader       │     │  ─ Timecode reader       │
│  ─ Thumbnail generator   │     │  ─ Thumbnail generator   │
│  ─ Audio metering        │     │  ─ Audio metering        │
│  ─ Benchmark runner      │     │  ─ Benchmark runner      │
│  ─ Local SQLite          │     │  ─ Local SQLite          │
└─────────────────────────┘     └─────────────────────────┘
```

The controller mode instance can also run local capture pipelines if hardware is present on that machine — it registers itself as a node in its own registry.

---

## The Rust Binary

**Language:** Rust  
**HTTP/WS:** Axum + `tower-http`  
**Media pipeline:** GStreamer via `gstreamer-rs`  
**Database:** SQLite via `sqlx`  
**UI embedding:** `rust-embed` (release builds) / served from `ui/dist/` (debug builds)  
**Type export:** `ts-rs` — derives TypeScript types from Rust structs  

Started with a mode flag:

```
capture-room --role node         # capture only (default)
capture-room --role aggregator   # capture + orchestration + UI
```

Both modes listen on the same configurable port (default `7700`). The mode determines which route groups are registered, not which port is used. The controller calls its own local capture subsystem in-process — no loopback HTTP.

On first run with no config file, generates a UUID, writes defaults, and starts mDNS announcement.

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

One pipeline per active source. Branches are enabled or disabled based on the recording profile.

```
[InputSource gst src element]
    │
    ├─► [timecode extractor]
    │
    └─► [tee]
          ├─► [queue] → [primary encoder] → [tee] → [muxer] → [filesink: primary path]
          │                                       └─► [muxer] → [filesink: redundant path]  (optional, same profile only)
          ├─► [queue] → [secondary encoder] → [muxer] → [filesink: proxy path]
          ├─► [queue] → [videoscale] → [jpegenc] → [thumbnail HTTP endpoint]
          └─► [queue] → [audioconvert] → [level] → [audio meter WebSocket publisher]
```

If the redundant path uses a different profile than primary, it gets its own encoder branch (same topology as the secondary path). If the profiles are identical, the encoded bitstream is split via `tee` after a single encoder, saving a full re-encode.

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

## Node API — REST (port 7700)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/status` | Node health, UUID, version, uptime, mode |
| GET | `/api/v1/sources` | List all discovered input sources |
| GET | `/api/v1/sources/{id}` | Source details and capabilities |
| POST | `/api/v1/sources/scan` | Rescan for available sources |
| GET | `/api/v1/recordings` | Active and recent recording sessions |
| POST | `/api/v1/recordings` | Start a recording session |
| GET | `/api/v1/recordings/{id}` | Session details |
| PATCH | `/api/v1/recordings/{id}` | Stop or update a session |
| GET | `/api/v1/thumbnails/{source_id}` | Latest thumbnail JPEG |
| GET | `/api/v1/presets` | Locally cached presets |
| POST | `/api/v1/presets/sync` | Receive preset sync push from controller |
| POST | `/api/v1/schedules/sync` | Receive schedule sync from controller |
| GET | `/api/v1/benchmark` | Most recent benchmark results |
| POST | `/api/v1/benchmark` | Start a benchmark run |

## Controller API — REST (port 7700)

All `/api/v1/nodes/{id}/*` routes proxy to the target node and return the result directly.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/nodes` | All nodes, status, source counts |
| POST | `/api/v1/nodes` | Register a node manually |
| DELETE | `/api/v1/nodes/{id}` | Remove a node |
| GET | `/api/v1/nodes/{id}/sources` | → proxied to node |
| POST | `/api/v1/nodes/{id}/sources/scan` | → proxied to node |
| GET | `/api/v1/nodes/{id}/recordings` | → proxied to node |
| POST | `/api/v1/nodes/{id}/recordings` | → proxied to node |
| PATCH | `/api/v1/nodes/{id}/recordings/{rid}` | → proxied to node |
| GET | `/api/v1/nodes/{id}/thumbnails/{sid}` | → proxied to node |
| GET | `/api/v1/nodes/{id}/benchmark` | → proxied to node |
| POST | `/api/v1/nodes/{id}/benchmark` | → proxied to node |
| GET | `/api/v1/presets` | List all presets |
| POST | `/api/v1/presets` | Create preset (syncs to all nodes) |
| PUT | `/api/v1/presets/{id}` | Update preset (syncs to all nodes) |
| DELETE | `/api/v1/presets/{id}` | Delete preset |
| GET | `/api/v1/schedules` | List schedules |
| POST | `/api/v1/schedules` | Create schedule |
| PUT | `/api/v1/schedules/{id}` | Update schedule |
| DELETE | `/api/v1/schedules/{id}` | Delete schedule |
| GET | `/api/v1/logs` | Aggregated logs (query: node, level, since) |
| GET | `/api/v1/overview` | All nodes + active recordings snapshot |

---

## WebSocket Events

### Node (`ws://node:7700/ws`)

All events are JSON with a `type` field.

| Event type | Payload |
|------------|---------|
| `source.available` / `source.lost` | source id, name |
| `recording.started` / `recording.stopped` / `recording.error` | session id, source id |
| `feed.status` | source id, timecode, bitrate, dropped frames, duration (periodic) |
| `audio.levels` | source id, channel peak/RMS values (~10fps) |
| `thumbnail.updated` | source id, URL |
| `benchmark.progress` | step, feeds, metrics |
| `benchmark.complete` | result summary |
| `log` | level, message, timestamp |

### Controller (`ws://controller:7700/ws`)

Re-emits all node events with `node_id` added, plus controller-level events:

| Event type | Description |
|------------|-------------|
| `node.online` / `node.offline` | Node connectivity change |
| `schedule.triggered` / `schedule.completed` | Schedule lifecycle |

---

## TypeScript Types

Rust structs used in API responses are annotated with `#[derive(TS)]` from the `ts-rs` crate. Running `cargo test --features export-types` generates TypeScript definitions into `ui/src/types/generated/`. These files are committed to the repo so the UI can be developed without a prior Rust build.

---

## SQLite Schemas

### Node mode (every instance)

```sql
CREATE TABLE node_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
    -- keys: uuid, name, mode, controller_url
);

CREATE TABLE recording_sessions (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL,
    preset_id       TEXT NOT NULL,
    started_at      TEXT NOT NULL,
    stopped_at      TEXT,
    primary_path    TEXT NOT NULL,
    secondary_path  TEXT,
    redundant_path  TEXT,
    status          TEXT NOT NULL,  -- active | stopped | error
    error_message   TEXT
);

CREATE TABLE presets_cache (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    data        TEXT NOT NULL,  -- JSON blob
    version     INTEGER NOT NULL,
    synced_at   TEXT NOT NULL
);

CREATE TABLE schedules_cache (
    id        TEXT PRIMARY KEY,
    data      TEXT NOT NULL,  -- JSON blob
    synced_at TEXT NOT NULL
);

CREATE TABLE benchmark_results (
    id        TEXT PRIMARY KEY,
    run_at    TEXT NOT NULL,
    profile   TEXT NOT NULL,  -- JSON blob
    max_feeds INTEGER NOT NULL,
    metrics   TEXT NOT NULL   -- JSON blob
);
```

### Controller mode (additional tables)

```sql
CREATE TABLE nodes (
    id         TEXT PRIMARY KEY,  -- UUID from node
    name       TEXT NOT NULL,
    ip         TEXT NOT NULL,
    port       INTEGER NOT NULL DEFAULT 7700,
    discovered INTEGER NOT NULL DEFAULT 0,  -- 1 = mDNS, 0 = manual
    last_seen  TEXT,
    status     TEXT NOT NULL DEFAULT 'unknown'  -- online | offline | unknown
);

CREATE TABLE presets (
    id                        TEXT PRIMARY KEY,
    name                      TEXT NOT NULL,
    codec                     TEXT NOT NULL,
    container                 TEXT NOT NULL,
    resolution                TEXT,           -- null = match source
    framerate                 TEXT,           -- null = match source
    bitrate_kbps              INTEGER,        -- null = quality-based
    quality                   TEXT,
    output_template           TEXT NOT NULL,
    secondary_output_template TEXT,
    redundant_output_template TEXT,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    version                   INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE schedules (
    id         TEXT PRIMARY KEY,
    node_id    TEXT NOT NULL REFERENCES nodes(id),
    source_id  TEXT NOT NULL,
    preset_id  TEXT NOT NULL REFERENCES presets(id),
    start_at   TEXT NOT NULL,  -- ISO 8601
    stop_at    TEXT NOT NULL,
    recurrence TEXT,           -- cron expression, null = one-shot
    status     TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL
);

CREATE TABLE logs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id     TEXT NOT NULL,
    level       TEXT NOT NULL,
    message     TEXT NOT NULL,
    recorded_at TEXT NOT NULL
);
```

---

## Web UI

**Framework:** Vue 3 + TypeScript  
**Build:** Vite  
**State:** Pinia  
**Routing:** Vue Router  
**HTTP:** `ofetch`  
**Components:** shadcn-vue — `npx shadcn-vue@latest init --preset ae2ZjdI` from `ui/`  

In production, the compiled UI is embedded into the Rust binary via `rust-embed` and served by the controller. In development, Vite runs its own dev server and proxies `/api` and `/ws` to the Rust controller.

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

A single WebSocket connection to the controller feeds all reactive UI state via Pinia stores. Components subscribe to store slices; they don't manage WebSocket connections directly.

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

Nodes register an mDNS service on startup:
- Service type: `_captureroom._tcp.local`
- TXT records: `uuid`, `name`, `version`, `mode`

The controller listens for announcements and adds discovered nodes automatically. Manual IP registration is always available as a fallback.

Discovery is **asymmetric** — the controller initiates all connections to nodes; nodes never need to know the controller's address. On first contact and on every reconnect after a health-poll gap, the controller immediately pushes a full preset and schedule sync to the node. This is the recovery path for offline nodes: no pull handshake required.

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
│   │   │   ├── node/            # Node-mode routes (sources, recordings, etc.)
│   │   │   └── controller/      # Controller-mode routes (nodes, presets, schedules)
│   │   ├── controller/
│   │   │   ├── registry.rs      # Node registry + health polling
│   │   │   ├── scheduler.rs     # Schedule execution engine
│   │   │   ├── proxy.rs         # HTTP proxy to node APIs
│   │   │   └── sync.rs          # Preset + schedule sync to nodes
│   │   ├── pipeline/            # GStreamer pipeline manager
│   │   ├── sources/
│   │   │   ├── mod.rs           # InputSource trait + SourceType enum
│   │   │   ├── ndi.rs
│   │   │   ├── decklink.rs
│   │   │   └── test.rs          # Synthetic test source
│   │   ├── recording/           # Session lifecycle
│   │   ├── timecode/            # LTC/VITC extraction
│   │   ├── thumbnail/           # JPEG generation
│   │   ├── audio/               # Level metering
│   │   ├── benchmark/           # Benchmark runner
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
    "dev:node": "cargo watch -x 'run -p capture-room -- --mode controller'",
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
   - ✅ `NodesView.vue`, ✅ `PresetsView.vue`
   - ⬜ `SourcesView`, `RecordingsView`, `SchedulesView`, `LogsView` are still "coming soon" placeholders
10. 🟡 **Rust — NDI implementation** (NDI hardware on hand)
    - ⬜ Prereq: install `gst-plugin-ndi` (`ndisrc` / `ndisrcdemux`) — not present on dev machine
    - ⬜ `NdiSource` impl + device discovery in `SourceRegistry::scan()`

> **Sequencing note:** the v1 build order above is the original plan. Active work is now
> sequenced in [ROADMAP.md](ROADMAP.md), which front-loads a generic TestSource, the Sources
> view, and live source monitoring ahead of NDI capture.
11. ⬜ **Rust — Decklink implementation** — deferred (no hardware)
12. 🟡 **Rust — preset sync + scheduling engine**
    - ✅ Preset sync (`controller/sync.rs`); ⬜ scheduler engine (`controller/scheduler.rs` is an empty stub)
13. ⬜ **Rust — benchmark runner** — empty stub (`benchmark/mod.rs`)
14. ⬜ **Rust — timecode** — empty stub (`timecode/mod.rs`); TestSource fakes a wall-clock TC
15. ⬜ **Rust — redundant recording path**
16. ⬜ **Cross-platform packaging + GitHub Actions**
