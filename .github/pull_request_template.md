<!--
The title becomes the commit on main (PRs are squash-merged): "area: what changed",
e.g. "replay: trim an export's sound at the out point too".
-->

## What and why

<!-- What this changes, and why. "Fixes #123" closes the issue when it merges. -->

## How it was tested

<!--
OS and version, and hardware where it matters (Decklink model, NDI sources, cameras,
displays). Say what you couldn't test, too.
-->

## Checklist

- [ ] `pnpm check` passes
- [ ] `pnpm types` run and `ui/src/types/generated/` committed, if a type in `node/src/api/` the UI sees changed
- [ ] Both sides changed together, if `check_advanced` / `ui/src/lib/advanced.ts` or `incompatible()` / `CONTAINERS_FOR` changed
- [ ] No license added to `about.toml` to make a new dependency pass
- [ ] Released migrations left as they are; schema changes go in a new one
