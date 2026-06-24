# Capture Room — Roadmap

Active sequencing of work, decisions, and rationale. This complements
[ARCHITECTURE.md](ARCHITECTURE.md) (the design spec) — when the two disagree on
*order*, this file wins; ARCHITECTURE.md remains the source of truth for *design*.

_Last updated: 2026-06-23_

---

## Current sequence

1. ✅ **Generic TestSource + Sources view**
2. ✅ **Live source monitoring**
3. **NDI capture** (+ plugin build/packaging) ← active
4. **Multi-pipeline output per preset**
5. **Benchmark + capacity estimator**
6. **UI overhaul / dark mode** — woven through 1–5; design-token pass up front
7. **Follow-on:** scheduler engine, timecode, packaging/CI, additional source types

Rationale for front-loading 1–2 ahead of NDI: a configurable TestSource plus the
Sources view gives a real authoring/verification surface, and live monitoring forces
the source-lifecycle design (connect-on-discovery, not connect-on-record) that NDI
capture depends on anyway.

---

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

Touches: `node/src/sources/test.rs`, `node/src/sources/registry.rs`, `ui/src/views/SourcesView.vue`.

## 2. ✅ Live source monitoring

**Biggest architectural change.** `ThumbnailStore` and `AudioMeter` are now owned by
`MonitorPipeline` (`node/src/pipeline/monitor.rs`), so thumbnails and meters are always
live — no recording required.

- **Single pipeline per source.** `source bin → vtee + atee → [thumbnail branch] + [audio meter branch]`. Recording attaches as an additional branch off vtee/atee and detaches cleanly on stop.
- **`SourceManager`** (`node/src/sources/manager.rs`) owns per-source `MonitorPipeline`s, the `SourceRegistry`, and active `RecordingSession`s — single source of truth for all capture state.
- **Live settings reconfiguration.** `MonitorPipeline::reconfigure()` updates GStreamer element properties in place (capsfilter caps, level interval) without stopping pipelines — applies immediately during recording.
- **Non-blocking stop recording.** `begin_stop_recording` removes the session and clones `Arc<MonitorPipeline>`, releasing the write lock before awaiting EOS. The WS emitter (which holds a read lock every 100 ms) is never blocked during the multi-second EOS drain.
- **Leaky video recording queue.** The video branch off vtee uses `leaky=upstream` so a slow encoder (e.g. x264enc on a complex ball-pattern source) drops frames instead of stalling the tee and backpressuring through the muxer's collect-pads to freeze audio monitoring.

Touches: `node/src/pipeline/monitor.rs`, `node/src/pipeline/profile.rs`, `node/src/sources/manager.rs`, `node/src/sources/registry.rs`, `node/src/api/node/mod.rs`, `DashboardView.vue`.

## 3. NDI capture

NDI hardware is on hand. Built on the `gst-plugin-ndi` GStreamer elements
(`ndisrc` / `ndisrcdemux`), **not** raw NDI SDK FFI — keeps parity with TestSource's
bin + `video`/`audio` ghost-pad contract.

- `NdiSource` impl + device discovery (`ndi-device-monitor`) wired into
  `SourceRegistry::scan()`.
- Decklink remains deferred (no hardware).

### Plugin build & packaging — licensing-driven

`gst-plugin-ndi` is open source, but it links `libndi`, whose NDI SDK license restricts
redistribution. Strategy:

- **Our plugin `.so`/`.dll`** — we build and ship it freely.
- **`libndi` runtime** — loaded dynamically; the *user* installs the free NDI Runtime
  redistributable. We never bundle `libndi` itself. (Same pattern OBS uses.)

| Platform | Approach |
|----------|----------|
| Linux | Ship `libgstndi.so` next to the binary; set `GST_PLUGIN_PATH` in the systemd unit. NDI Runtime documented/scripted as a dependency. |
| macOS | Bundle the plugin; direct users to NDI Tools/Runtime for `libndi`. |
| Windows | Bundle the plugin DLL; NDI Runtime installer is a documented prereq. |

- Add a Cargo feature flag `ndi` so builds without the SDK still compile (TestSource-only).

**Dev-machine setup (CachyOS / Arch):** install NDI SDK/runtime (AUR `ndi-sdk` / `libndi`)
→ build `gst-plugin-ndi` (`cargo cbuild`) → place `.so` on `GST_PLUGIN_PATH` →
verify `gst-inspect-1.0 ndisrc`.

## 4. Multi-pipeline output per preset

Replace the fixed `output_template` / `secondary_output_template` /
`redundant_output_template` triple with **N output legs** (e.g. H.264 MP4 to one path +
ProRes MOV to another). Absorbs the old "redundant path" concept.

- **Schema:** new `preset_outputs` table (preset_id, codec, container, resolution, fps,
  bitrate/quality, path_template, role) — migration `0003`. `presets` keeps source-level
  settings.
- **Pipeline:** `Pipeline::new`'s `secondary: Option<(...)>` becomes `legs: Vec<(path,
  profile)>`, fanning out from the existing `tee`. Generalize the encode-sharing
  optimization (identical profiles split bitstream instead of re-encoding).
- **UI:** Presets view becomes a list-of-outputs editor.

Touches: `node/migrations/`, `node/src/pipeline/mod.rs`, `node/src/pipeline/profile.rs`,
`node/src/api/types.rs`, `ui/src/views/PresetsView.vue`.

## 5. Benchmark + capacity estimator

`node/src/benchmark/mod.rs` is an empty stub today.

- Synthetic pipelines at increasing feed counts → measure **dropped frames, CPU, disk
  write throughput, memory pressure**. Stop past a dropped-frame threshold.
- Persist runs in `benchmark_results` (table already specced in ARCHITECTURE.md).
- **Capacity estimator:** given a preset (incl. multi-leg) and feed count, predict
  sustainability from stored benchmarks — surfaced in the Nodes view.
- Sequenced after #4 so benchmarks reflect realistic multi-leg presets.

## 6. UI overhaul / dark mode

shadcn-vue is already in place, so dark mode is mostly CSS-variable theming + a toggle.

- Up front: a design-token pass (define the dark palette / tokens once).
- Then woven incrementally into each view as it's built (Sources, Recordings, Schedules,
  Logs are placeholders today).
- **Per-page tweaks backlog:** many small per-page refinements to tackle one at a time —
  tracked here as work surfaces, not planned in bulk.

## 7. Follow-on

- **Scheduler engine** — `controller/scheduler.rs` is an empty stub; schedules table specced.
- **Timecode** — real LTC/VITC extraction; `timecode/mod.rs` is a stub (TestSource fakes wall-clock TC).
- **Packaging + GitHub Actions** — cross-platform builds; folds in the NDI packaging strategy above.
- **Additional source types** (each is a new `InputSource` impl, additive):
  - **RTSP** (`rtspsrc`) — IP cameras; easy, high value.
  - **SRT** (`srtsrc`) — contribution feeds over unreliable networks.
  - **HDMI/USB capture** (`v4l2src`).
  - **SDI via Decklink** — already specced; needs hardware.
