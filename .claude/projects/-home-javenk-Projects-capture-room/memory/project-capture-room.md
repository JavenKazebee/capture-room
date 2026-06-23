---
name: project-capture-room
description: Core facts about the capture-room project — stack, build order progress, key decisions
metadata:
  type: project
---

Multi-feed video capture and recording platform. Single Rust binary (`capture-room`), two modes: `--mode node` and `--mode controller`. Web UI served by controller on port 7700.

**Why:** Live broadcast environments, single machine or cluster of ~20 nodes.

**Stack:**
- Rust binary in `node/` (Axum, GStreamer, SQLx, ts-rs, rust-embed)
- Vue 3 UI in `ui/` (Vite, Tailwind v4, shadcn-vue preset `ae2ZjdI`, Pinia, Vue Router, ofetch, `@lucide/vue`)
- pnpm workspace; root `package.json` has `dev`, `build`, `types` scripts
- Rust not yet installed on dev machine (only node v24 + pnpm via fnm)

**Build order progress (from ARCHITECTURE.md):**
- [x] Step 1 — Monorepo scaffold
- [x] Step 2 — UI scaffold
- [x] Step 3 — Rust TestSource + InputSource trait
- [x] Step 4 — GStreamer pipeline (single source, single output)
- [x] Step 5 — multi-output tee, thumbnail, audio metering (was in pipeline from step 4)
- [x] Step 6 — node-mode REST + WebSocket API, ts-rs type export
- [ ] Step 7 — UI dashboard (live feed grid, recording controls)
- [ ] Steps 8–16 — see ARCHITECTURE.md

**Key decisions:**
- Icon library is `@lucide/vue` (installed by shadcn-vue), NOT `lucide-vue-next`
- Tailwind v4 uses `@import "tailwindcss"` in CSS (no config file)
- `baseUrl` removed from tsconfig (TS6 deprecated it); `paths` works standalone
- Generated ts-rs types in `ui/src/types/generated/` are committed (not gitignored)

**Step 6 details (node API):**
- New files: `node/src/db/mod.rs`, `node/src/ws.rs`, `node/src/state.rs`, `node/src/recording/mod.rs`, `node/src/api/types.rs`, `node/src/api/node/mod.rs`, `node/migrations/0001_init.sql`
- Axum 0.8 router on port 7700; SQLite via sqlx (no compile-time query macros — uses runtime `query_as::<_, T>`)
- `ts-rs` types are gated behind `--features export-types`; export test in `api/mod.rs` calls `T::export_all()`
- DB file path: passed via `--db` flag, defaults to `capture-room.db` in working directory
- Node UUID persisted to `node_config` table on first run
- `WsEvent` uses `#[serde(tag = "type", rename_all = "snake_case")]` — matches the architecture doc event names
- Recording start creates a `Pipeline`, holds it in `RecordingManager::active` map; stop sends EOS and awaits flush

**How to apply:** When continuing build steps, check this list for current progress and pick up at the next unchecked step.
