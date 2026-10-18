# Capture Room — Architecture

Multi-feed video capture, playout and instant replay for live broadcast, on one machine
or a cluster of up to ~20 nodes managed from one web UI. Open work is in
[ROADMAP.md](ROADMAP.md); module-level detail lives in each module's `//!` docs.

## Design principles

- **One binary, every instance is a node.** A node exposes its own machine: sources,
  storage, recordings, channels, replay buffers. It knows nothing about other nodes.
- **Controller is a live toggle.** Any node can also be a controller (`--controller`,
  persisted, or `PUT /api/v1/controller`). It discovers nodes, forwards commands to them
  and merges their event streams.
- **Controllers send plain commands.** A start carries its output settings inline, so
  nodes keep no preset store and nothing is synced.
- **One addressing scheme.** The UI always calls `/api/v1/nodes/{node_id}/…`; on a plain
  node that list is just itself.
- **Pluggable inputs and outputs.** Sources implement `InputSource`; outputs implement
  `OutputSink`, its mirror. Adding a type is additive.
- **Types flow from Rust.** API types are Rust structs exported to TypeScript by `ts-rs`.

**Non-goals:** a vision mixer, a graphics system, a media asset manager or editor, or a
general automation system (scheduling and playlists drive Capture Room's own channels
only).

## Components

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
│  ─ Presets, clock server                     │
│  ─ /ws: own events + relayed peer events     │
└──────────────┬───────────────────────────────┘
               │  HTTP /api/v1/node/… + WS /api/v1/node/ws
               ▼
┌─────────────────────────┐     ┌─────────────────────────┐
│  capture-room (node)     │     │  capture-room (node)     │  ...
│  sources, pipelines,     │     │                          │
│  sessions, storage,      │     │                          │
│  channels, replay, SQLite│     │                          │
└─────────────────────────┘     └─────────────────────────┘
```

Every instance serves the UI. Stack: Rust, Axum, GStreamer (`gstreamer-rs`), SQLite
(`sqlx`), UI embedded with `rust-embed` in release builds.

## Sources

```rust
pub trait InputSource: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn source_type(&self) -> SourceType;
    fn capabilities(&self) -> Option<SourceCapabilitiesDto>; // None: negotiated at runtime (NDI)
    fn fingerprint(&self) -> String; // a change rebuilds the source and its monitor
    fn gst_src_element(&self) -> gst::Element; // a Bin with "video" / "audio" ghost pads
    fn timecode(&self) -> Option<String>;
}
```

| Type | Built on | Notes |
|---|---|---|
| Test | `videotestsrc` + `audiotestsrc` | Reference pattern for a source bin |
| NDI | `gst-plugin-ndi` (`ndisrc`), device monitor | Discovered, not stored. Connects to every sender or only chosen ones (`ndi_subscriptions`) plus those in use; never to its own outputs |
| File | `uridecodebin` player → `appsrc` | Looping media file as a live feed; restamped onto the monitor's running time; first frame after a loop gap is `DISCONT` |
| Stream | `uridecodebin3` | RTSP, SRT, RTMP, HLS/HTTP, UDP MPEG-TS, by URL scheme |
| Device | `GstDeviceMonitor` | Capture cards, webcams. PipeWire and raw ALSA devices are skipped (duplicates / fights the sound server) |
| WHIP | `whipserversrc` | WebRTC ingest from OBS and browsers |
| Decklink | `decklinkvideosrc` + `decklinkaudiosrc` | Keyed by `persistent-id`; per-card profile (`decklink_profiles`) is in the fingerprint. Untested on hardware |
| Channel | see Playback | A playout channel's program |

**Configured vs discovered.** All but NDI are configured: JSON tagged by `type`
(`SourceConfig`) in `configured_sources`. A new type adds a variant and a case in
`sources::configured`.

**Live sources** (`sources/live.rs`): stream, device, WHIP and Decklink share one bin so a
dropout never stops the monitor or its recordings; the feed plays black and silence while
the source retries forever.

```
source → [fallbacksrc] → input chain ─┐
            black (direct sources only) ┴→ compositor → locked caps → video
source → [fallbacksrc] → input chain ─┐
                        silence (always) ┴→ audiomixer → 48 kHz F32 → audio
```

- Output format is locked to the first live format (or a configured one), so recordings
  never see a caps change. The aggregators run on the clock and repeat the last frame or
  play silence instead of leaving gaps.
- `fallbacksrc` gets a fixed `min-latency`; otherwise every switch re-queries latency and
  stalls the pipeline.
- WHIP and Decklink sit in the bin directly ("direct mode"): `fallbacksrc`'s wrapper aborts
  the process with `webrtcsrc`. The WebRTC elements block on their own tokio runtime during
  state changes, so monitor state changes run off ours.
- `LinkState` (`live`, `connecting`, `waiting`, `reconnecting`) comes from a pad probe
  counting real frames.
- **Audio pairing:** an `AudioPlan` takes the source's own audio, channels from an audio
  device on the node, or silence.

**Optional elements.** `plugins::REQUIRED` fails startup. Other types list what they need;
a missing element makes that type (or stream protocol) unavailable
(`NodeStatus.source_types`) rather than stopping the node.

## Pipelines

One always-on **monitor pipeline** per source, plus one pipeline per **consumer** (recording
leg, output, replay buffer). Consumers are fed by `StreamProducer`s (an `appsink` fanning
out to `appsrc`s in other pipelines), not a tee: a tee passes a branch's error back to the
source, so one failing leg would stop the monitor and every other leg.

```
Monitor (pipeline/monitor.rs)
source bin ─┬─ vtee ─┬─ videorate → scale → jpegenc → ThumbnailStore
            │        └─ appsink = video producer
            └─ atee ─┬─ level → AudioMeter → WS audio.levels
                     └─ appsink = audio producer

Recording leg (pipeline/recording.rs)
appsrc v → [deinterlace] → [rate/scale] → convert → encoder → [parse] ─┐
appsrc a → queue 10 s → convert/resample → [encoder] ──────────────────┴→ muxer → filesink
```

- Consumers share the monitor's clock and base time. Each `appsrc` holds 500 ms and drops
  the oldest when full, so a slow encoder drops frames in its own leg only; drops are
  counted per leg.
- **Start:** every leg is built before any starts; a failed start rolls back and deletes
  partial files. **Stop:** EOS on the appsrcs, wait for the leg's EOS with a timeout.
- **Failure:** a failed leg is reported (`recording.leg_failed`) and the session continues
  on its other legs; when all have failed it ends as `error`. A failed monitor is rebuilt
  by a 5 s recovery check; its recordings stop and are finalized, not restarted.
- **Encoders:** each codec lists encoders best first (VideoToolbox, then software); the
  first that builds wins. VideoToolbox H.264/H.265 are 4:2:0 only. `OutputAdvanced` holds
  per-output overrides (encoder, rate control, keyframes, audio); `check_advanced` and
  `ui/src/lib/advanced.ts` reject impossible combinations.
- **Crash safety:** every file is playable after a crash with no recovery step. MOV/MP4
  with compressed audio are fragmented and finalized on stop. PCM fragments can't be read
  back, so MOV+PCM reserves the index up front and always records through
  `splitmuxsink`, rolling over at 4 h. MKV is readable as is.
- **Splitting** by time or size goes through `splitmuxsink`; new files are saved to the
  session immediately. `vtenc_prores` marks every frame a delta, so the flag is cleared or
  splitmuxsink never splits.
- PCM goes through `audiorate`: qtmux fails a PCM stream that jumps (a looping file).
- Multichannel PCM in MOV needs its channel layout dropped (`capssetter`), or qtmux mixes
  it to stereo.

Codecs: ProRes (4444…Proxy), uncompressed, H.264, H.265, VP9; containers MOV, MP4, MKV
(`incompatible()` in `pipeline/profile.rs`). MXF isn't supported (`mxfmux` rejected every
codec).

**Thumbnails** are JPEGs at 1–30 fps (node setting), held in memory (`thumbnails.rs`).
The UI opens one `/ws/thumbnails` socket and sends the sources it shows on screen and an
optional fps cap as JSON. Each new frame comes back as a binary message: key length (u16
BE), `node/source` key, then the JPEG. A controller serves peers' sources through one
upstream socket per peer, open only while one of that peer's sources is watched. Frames
are numbered so unchanged ones aren't sent again. `GET /thumbnails/{id}` still returns
the latest one.

## Clock and timecode

Every pipeline on a node runs on its **node clock** (`clock.rs`).

- **Controller mode** (default): the controller serves its monotonic clock with
  `NetTimeProvider` on UDP `--clock-port` (HTTP port + 100); nodes follow with a
  `NetClientClock`. Tens of µs on a wired LAN; an NTP step on the controller can't jump
  every pipeline.
- **PTP mode:** everything follows an existing grandmaster (`PtpClock`); TAI-based.
- **Claims:** a controller claims each node with every health check. A node keeps the first
  claimant until it has been quiet 20 s. Controllers take no claims, so two controllers make
  two domains, not a loop.
- **Switching:** a new clock is used once synced. A running pipeline can't change clocks,
  so busy sources keep the old one until idle (`stale_sources`).
- **Domains:** `controller:{id}`, `ptp:{domain}` or `local:{node id}`. API times are µs on
  the domain's clock (exact as a JS number, even on TAI). Each domain has a `utc_offset_us`
  to map to wall time, fixed per session so an NTP adjustment can't step timecode.
- **Synchronized start:** `start_at: { domain, time_us }` (≤ 10 s ahead); each leg drops
  frames due before it. Frames still sit on each source's own grid; closer than a frame
  needs genlock.
- **Timecode:** MOV legs stamp each frame's capture time (running time + base time, mapped
  to UTC, in local time-of-day, drop-frame at 29.97/59.94); qtmux writes the first as the
  `tmcd` track. MP4 and MKV get none. Keep a venue's machines in one time zone.
- **Source timecode:** a frame's own timecode (Decklink RP188) wins over the time of day.
  A live bin's compositor makes new frames, so it notes each input frame's timecode and
  stamps its output with the newest, counted on by the frames since (it repeats a late
  frame) and scaled to another output rate; after 1 s without one, frames get none.

## Playback

A channel is a configured source (`SourceConfig::Channel`, `sources/channel/`) with a fixed
format and a list of outputs. Its program is the source bin, run by an ordinary monitor, so
it gets thumbnails, meters and can be recorded.

```
Clip player (one per loaded clip, own pipeline)
uridecodebin ─┬→ videoconvert → appsink ─ push ─┐
              └→ audioconvert → appsink ─ push ─┐
                                                ▼
Program bin (run by the monitor)
  appsrc lane ─┐  black ─┴→ compositor → channel caps → video
  appsrc lane ─┐  silence ┴→ audiomixer → 48 kHz, N ch → audio
                      │
  monitor producers ──┴─► recording legs, output legs (pipeline/output.rs)
```

- **Never dark, never renegotiates:** reuses the live-source compositor/mixer, locked to
  the channel format; clips are converted into it.
- **Lanes:** a loaded clip adds an `appsrc` chain per stream; `Lanes::attach` moves them to
  a rebuilt bin so a monitor restart keeps the clip.
- **Decoupled players:** a bad file fails its player, never the program. Buffers are
  restamped into program running time plus a 100 ms lead; a lane drops anything not after
  its previous buffer.
- **Transport** (`Playout`, in memory): the next-up item is always prerolled in standby, so
  play needs no cue. Loops are non-flushing segment seeks. At the out point: next, hold,
  black or loop. The transport survives a format change (`InputSource::adopt`); names are
  outside fingerprints, so renames restart nothing.
- **Gapless handover:** the standby player starts with a base time that puts its first
  frame right after the last queued one, pushing into the same lanes.
- **Outputs** (`OutputConfig` by `type`), fed like recording legs, each failing alone and
  outside the channel's fingerprint:
  - **NDI** — `ndisinkcombiner` → `ndisink`; names unique per node.
  - **SRT** — H.264 + AAC in MPEG-TS, fixed live settings, a keyframe every second.
    Encoders are held until both have caps and video starts on a keyframe: otherwise
    `srtsink` replays a one-stream program table to every new receiver.
  - **RTSP** — mounts on the node's RTSP server (port 8554), encoding only while watched.
  - **WHEP** — `whepserversink` on localhost, proxied at `/whep/<path>` on the API port
    (`api/whep.rs`), CORS open. VideoToolbox is hidden from `webrtcsink` (it fails
    negotiation).
  - **Display** — fullscreen on a local screen via a winit loop and `glimagesink`.
  - **Decklink** — `decklinksink`s on the node clock; the card drops/repeats a frame when
    clocks drift. Untested on hardware.
- **Media library:** `media` table of playable files per node, probed with `Discoverer`.
  Removing an entry leaves the file.
- **Browser preview** (Recordings): H.264 4:2:0 + AAC in .mp4/.mov is `playable`; files
  are served with range requests by output and index, never arbitrary paths.

## Instant replay

A replay-enabled source records continuously into a rolling buffer that a channel plays
from at variable speed (`pipeline/replay.rs`).

- **Buffer:** one-second all-intra segments (`splitmuxsink` → `qtmux`, PCM) under
  `~/capture-room/.replay/`, oldest deleted past the length. Codec per buffer: ProRes
  Proxy, intra H.264 (`vtenc_h264` quality 0.75, about ProRes Proxy's picture at 60% of
  the size; x264 q18 with the rate cap lifted) or MJPEG (converted to full-range BT.601,
  which `jpegdec` assumes).
- **Index:** each finished segment's span on the node clock. Survives source restarts
  (leaving gaps); cleared on node restart or codec/clock change.
- **Playback:** a clip's closed segments are read through `splitmuxsrc` (`format-location`;
  globbing fails on the open segment), decoded in software (VideoToolbox drops frames in
  reverse). Speed is the seek rate; steps are `Step` events. Reaches within a segment of
  live.
- **Multi-node:** replay nodes subscribe to the same NDI senders and keep their own buffers;
  NDI `auto` timestamping puts two receivers within ~2 ms of each other once settled.
- **Saved clips:** segments are hard-linked into `~/capture-room/.replay-clips/` (instant,
  no extra space), stored in `replay_clips`, surviving the buffer and restarts. Clip groups
  save one clip per angle for the same span (all or none).
- **Export:** remux (no re-encode) via an accurate seek into one .mov, stamped with
  time-of-day timecode, added to the media library. A gate holds each stream until the
  seek's segment: never seek through a muxer.

## Benchmark and capacity

`benchmark.rs` finds how many feeds a node sustains for a set of outputs, a replay codec, or
a playout channel; `capacity.rs` turns results into load estimates.

- A run plays real footage (test patterns compress unrealistically) as `FileSource`s with
  real legs, kept out of the `SourceManager`. A step fails when source shortfall or output
  drops exceed the threshold (0.5%).
- Search: doubling with 5 s quick checks, then halving with full steps; the answer is
  confirmed by a full step, and a failing full step is measured twice.
- **Memory guard:** fast hardware encoders never drop, so memory is what stops them
  (a ProRes run once froze an 8 GB Mac). Available memory stays above max(1.5 GB, 1/8);
  on macOS read from `kern.memorystatus_level`, since sysinfo misreads compressed memory.
- Writes go to `.capture-room-benchmark-{id}` beside the real destination, deleted after.
  Real recordings and replay buffers win: they cancel a running benchmark.
- **Capacity:** results keyed by what is encoded (`outputs_key`, `replay_key`,
  `channel_key`) and format; other formats scale by pixels/s. Load is the sum of 1/N shares;
  fits ≤ 80%, tight ≤ 100%. Checks warn, never block. Starts are refused under 1 GB free.

## Schedules

`controller/scheduler.rs` starts recordings and playout at set times and stops them at the
end. Only a controller runs schedules, through the same node API the UI uses
(`forward::call_node`); nodes know nothing of them.

- A schedule runs once (a date) or weekly (weekdays), at an `HH:MM` in the controller's
  local time, with an optional duration. A time the clocks skip moves to the first minute
  after; a repeated one is the first.
- Each occurrence that comes due gets one `schedule_runs` row, inserted before anything
  starts (unique on schedule and start), so it can't run twice. The row keeps the session
  ids it started, so the end still stops them after a restart, and only them: a source
  already recording fails that target alone (`partial`).
- The loop wakes 2 s before a start, so recordings get a shared-clock `start_at` on the
  scheduled frame (within a node's 10 s lead); playout has no synchronized start and is
  called on time.
- An occurrence whose window is still open when the controller sees it (after a restart,
  or just created) starts late; one whose window passed while it was off is `skipped`.
  Windows that closed before the schedule was last saved are ignored.

## Logs

`logs.rs` is a `tracing` layer beside the formatter: it keeps the last 10,000 lines in
memory, numbered, and broadcasts each new one. `/api/v1/node/logs` pages back through them;
`/api/v1/node/ws/logs` streams them from a level up. The controller's `/ws/logs` relays the
nodes a subscription names, one peer socket each (like thumbnails). Lines from `logs.rs`
itself are never kept, so streaming can't log itself. Everything is still in the service's
log file.

## API

### Node — `/api/v1/node/…`

Local only; ids are local to the node.

| Path | Purpose |
|---|---|
| `/status`, `/settings`, `/storage`, `/checks`, `/update` | node info, monitor settings, volumes, setup checks, self-update |
| `/access-token` | set this node's token |
| `/clock`, `/clock/now`, `/clock/claim` | clock status, current time, controller claim |
| `/sources`, `/configured-sources`, `/devices`, `/decklink`, `/ndi`, `/displays`, `/files` | sources, and what can become a source or output |
| `/recordings[/{id}[/stop \| /outputs/{i}/files/{j}]]` | sessions, start/stop, files |
| `/media[/{id}[/file]]` | media library |
| `/channels/{id}[/playlist \| /play \| /cue \| /transport \| /replay \| /replay-clip]` | playout |
| `/replay[/{source_id}]`, `/replay/clips…`, `/replay/clip-groups…` | buffers, clips, export |
| `/benchmarks…`, `/capacity`, `/capacity/check` | benchmark runs, load, pre-start checks |
| `/thumbnails/{id}`, `/ws`, `/ws/thumbnails` | latest JPEG, this node's events, its thumbnails |
| `/logs`, `/ws/logs` | kept log lines, live log lines |

### Controller — `/api/v1/…`

| Path | Purpose |
|---|---|
| `/controller`, `/controller/clock` | toggle, clock mode |
| `/nodes`, `/nodes/{id}`, `/reachability/{id}` | registry, port probes |
| `/nodes/{id}/{*path}` | forwarded to that node's `/api/v1/node/{path}` |
| `/presets` | presets |
| `/schedules[/{id}[/stop \| /runs]]` | schedules (controller only), ending a run, run history |
| `/access-token` | set the token here and on every registered node |
| `/auth`, `/auth/sign-in`, `/auth/sign-out` | open: whether a token is needed, browser sign-in |
| `/ws`, `/ws/thumbnails`, `/ws/logs` | own events + every peer's; thumbnails and log lines from any node |

### Events

JSON with `type` and `node_id`: `recording.started|stopped|error|leg_failed|stats|removed`,
`feed.status`, `channel.state`, `media.updated`, `audio.levels`,
`benchmark.updated`, `node.updated`, `node.online|offline`, `schedule.updated`. The relay subscribes to each
peer's local-only `/api/v1/node/ws`, so controllers never echo each other.

Types: `pnpm types` regenerates `ui/src/types/generated/` (committed; CI checks it).

## Storage

SQLite, same schema on every instance (`node/migrations/`): `node_config` (key/value),
`recording_sessions` (names and formats copied in, so history survives renames),
`configured_sources`, `media`, `presets` + `preset_outputs`, `nodes`, `benchmark_results`,
`playlists`, `replay_clips`, `replay_clip_groups`, `ndi_subscriptions`, `schedules` +
`schedule_runs`.

**Path templates** resolve on the node (`profile::plan_legs`): `~`, `{date}`, `{datetime}`,
`{node}`, `{source}`, `{output}`, `{ext}`, `{take}`, `{segment}`. Default
`~/capture-room/{date}/{source}_{datetime}.{ext}`. Never overwrites: `{take}` advances,
otherwise a `_2` suffix; paths held by another active recording are refused.

## Web UI

Vue 3, TypeScript, Vite, Pinia, Vue Router, `ofetch`, shadcn-vue, `vue-sonner`, an
in-house `DataTable`. Dev: Vite proxies `/api` and `/ws` to :7700.

- **Shell:** workspaces (Record, Playback, Replay, Recordings, Schedule, Setup) and a ⌘K
  palette. Setup › Logs merges nodes' log lines with the page's activity events (⌘J).
  Views join only once the backend supports them.
- **Tokens:** dark-first OKLCH in `ui/src/style.css`. `--primary` sky-cyan; solid red means
  live: `--tally` is recording only, `--destructive` sits behind a confirm, `--brand` is the
  logo.
- **State:** one `/ws` connection feeds Pinia stores keyed by `(node_id, id)`; components
  never open sockets themselves.

## Networking and security

One API port (default 7700), HTTPS and HTTP; the controller's clock on UDP port + 100;
PTP on UDP 319/320; RTSP 8554. Nodes are found by mDNS (`_capture-room._tcp.local.`) or
added by address, approved, stored and health polled. The controller initiates every
connection; the only push is the clock claim.

**TLS** (`tls.rs`): each node makes a key on first run and a self-signed certificate for
it (renewed at startup within 60 days of its 800: macOS refuses longer). Its fingerprint
is the SHA-256 of the key, so renewing keeps it; Settings › About shows it.

- One port: a connection's first byte tells a TLS handshake from plain HTTP. Plain HTTP
  is answered from loopback and for `/whep` (players can't trust a self-signed
  certificate); anything else gets a 308 to HTTPS. The menu bar opens `http://localhost`.
- Nodes always reach each other over HTTPS. The peer client (HTTP and WebSockets) trusts
  only the keys of approved nodes; a separate client takes any key, only for the open
  `/api/v1/auth/identity`, and never sends the token.
- Browsers accept one certificate warning, for the controller: the UI reaches nodes only
  through it, WHEP too (`/api/v1/nodes/{id}/whep/…`; the media itself is WebRTC).

**Approving nodes** (`controller/discovery.rs`): a controller asks a node who it is and
reads its key from the handshake before trusting it. Approved nodes are stored with that
key pinned and never pruned.

- An mDNS announcement for an approved node moves it to a new URL only if it holds the
  pinned key. An unknown node is approved at once while the controller has no access
  token (nothing to protect; zero setup), else listed under Found on the network until
  approved or dismissed. Found nodes drop off after ~15 s unreachable.
- Adding by address approves. Approving gives the node this controller's token; one with
  another token refuses and shows as `access_denied`. An address that claims an approved
  node's ID with another key is refused.
- Removing a node unpins it and stops it being offered again until the controller
  restarts. Nodes stored before TLS have their key pinned at their stored URL on first
  contact.

**Access token** (`auth.rs`): one shared secret per cluster, stored in each node's config.
With none set the API is open (Setup › Checks warns). Once set, every `/api` route and
`/ws/…` need it; the UI assets, `/api/v1/auth/…` and `/whep` stay open, and WHIP has its own
port.

- Browsers sign in once and get an HttpOnly, `SameSite=Strict` cookie named per port, so
  `<img>`, `<video>`, downloads and the WebSocket need no code of their own. A 401 shows
  the sign-in; a changed token closes open WebSockets so pages notice.
- Nodes send it as `Authorization: Bearer` (health polls, clock claims, the forwarder, the
  relay, port probes). The forwarder never passes the browser's cookie on, and turns a
  peer's 401 into a 502 so the browser isn't signed out. A peer that refuses the token
  shows as `access_denied`.
- Setting it on a controller sets it on each registered node first (with the old one),
  then itself. Unreachable nodes keep their old token and are listed.
- The cookie is `Secure` when signed in over HTTPS (plain HTTP is only answered from this
  machine).
- **A lost token:** the menu bar's Copy Access Token (whoever is at the machine is
  trusted), or `capture-room access-token show | clear [--restart]` on the node's
  database (the installed service's by default).

## Deployment (macOS)

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
```

- **Release:** a `vX.Y.Z` tag matching `node/Cargo.toml` runs `release.yml`, publishing an
  archive per architecture, checksums, minisign signatures, `latest.json` and `install.sh`.
- **Build:** `scripts/macos/package.sh <arch>`; `gstreamer.sh` unpacks the official
  GStreamer packages into an SDK and a trimmed runtime, without the framework's `ndi`,
  `fallbackswitch` and `rswebrtc` (the node links its pinned ones; two would clash).
- **App:** `Capture Room.app` with GStreamer in `Contents/Frameworks`;
  `plugins::use_bundled_gstreamer` keeps a Homebrew GStreamer out. The app holds the macOS
  privacy permissions.
- **Service:** a LaunchAgent (a LaunchDaemon can't get camera/mic/screen permission).
  `KeepAlive { SuccessfulExit = false }` restarts crashes and panics but not Quit. Exit 75
  asks for a restart. Start at Login is a copy of the plist, so turning it off never stops
  the node.
- **Menu bar icon** (`tray.rs`): status, open UI, update, Start at Login, Quit.
- **Updates** (`update.rs`): checks `latest.json` every 6 h; refuses while busy; verifies
  the minisign signature against the compiled-in key (`scripts/release-key.pub`), swaps
  the app with two renames, exits 75. `CAPTURE_ROOM_RELEASES` points it at a test server.
- **Signing:** ad hoc until there's a Developer ID, so macOS re-asks permissions after each
  update. No hardened runtime: with ad hoc signing it refuses the plugins.

**Plugins:** core GStreamer from the bundle (or distro packages); gst-plugins-rs (ndi,
fallbackswitch, webrtc) statically linked at one pinned rev — adding one re-resolves
gstreamer-rs, so pin back with `cargo update --precise`; the NDI Runtime is user-installed
(we never ship `libndi`; on macOS the node finds NDI Tools' copy,
`plugins::locate_ndi_runtime`).

**Setup checks** (`checks.rs`) read live state on every request (the OS is the source of
truth). A check is a *problem* only when something on the node needs it. Shortcuts help
grant permissions; nothing grants them silently. Ports are probed from the controller,
since a machine can't test its own firewall.

## Repository

```
node/            Rust binary (api/, controller/, pipeline/, sources/, db/, migrations/)
ui/              Vue 3 UI (views/, components/, stores/, types/generated/)
scripts/macos/   packaging, install.sh
```

`pnpm dev` runs node and UI; `pnpm build` builds both; `pnpm types` regenerates TS types.
