# Capture Room

Multi-feed capture, playout and instant replay for live video. A Rust node (`node/`:
Axum, GStreamer, SQLite) serves a Vue 3 UI (`ui/`). Setup and dev workflow are in
[CONTRIBUTING.md](CONTRIBUTING.md); design detail is in each module's `//!` docs.

## Commands

- `pnpm build:ui` once before the first cargo build: the node embeds `ui/dist`.
- `pnpm check` before calling a change done: fmt, clippy `-D warnings`, tests, UI type-check.
- `pnpm types` after changing a type in `node/src/api/` the UI sees; commit
  `ui/src/types/generated/` with it (CI fails if stale).
- `pnpm dev` runs a controller on :7700 with `capture-room.db` and the Vite UI.

## Design

- One binary; every instance is a node and knows nothing of other nodes. Any node can
  also be a controller, which forwards commands and merges events. The UI always calls
  `/api/v1/nodes/{node_id}/…`, even on a single node.
- Controllers send plain commands: a start carries its output settings inline, so nodes
  keep no preset store and nothing is synced.
- Sources implement `InputSource`, outputs `OutputSink`; adding a type is additive.
- UI state: one `/ws` connection feeds Pinia stores keyed by `(node_id, id)`. Components
  never open sockets; stores and composables do.
- Each consumer of a source (recording leg, output, replay buffer) runs in its own
  pipeline, so one failure never stops the source or the others.
- Not a vision mixer, graphics system, asset manager or general automation system.

## Rules

- Builds and tests must pass on macOS, Linux and Windows. Watch for paths in
  `gst::parse::launch` strings (Windows backslashes): set paths as properties.
- gst-plugins-rs (ndi, fallbackswitch, webrtc) is linked statically at one pinned rev in
  `node/Cargo.toml`. Adding one re-resolves gstreamer-rs; pin back with
  `cargo update --precise`.
- Never bundle the NDI Runtime (`libndi`); the node finds it at run time.
- New Rust dependencies must have a license `about.toml` accepts (GPL-compatible). Don't
  widen the list to make CI pass.
- `ui/src/lib/advanced.ts` mirrors `check_advanced` in `pipeline/profile.rs`: change
  both sides.

## Comments

- A module's design goes in its `//!` doc, next to the code it describes.
- Comments say why (a GStreamer quirk, a constraint), not what the code does.
- No references to roadmap steps, issues or history; git keeps those.
