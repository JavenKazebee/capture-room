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
- [ ] Steps 5–16 — see ARCHITECTURE.md

**Key decisions:**
- Icon library is `@lucide/vue` (installed by shadcn-vue), NOT `lucide-vue-next`
- Tailwind v4 uses `@import "tailwindcss"` in CSS (no config file)
- `baseUrl` removed from tsconfig (TS6 deprecated it); `paths` works standalone
- Generated ts-rs types in `ui/src/types/generated/` are committed (not gitignored)

**How to apply:** When continuing build steps, check this list for current progress and pick up at the next unchecked step.
