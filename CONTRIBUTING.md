# Contributing

Bug reports, fixes and features are welcome. For anything bigger than a fix, open an
issue first so we can agree on the shape. Each module's `//!` docs explain its design;
[ROADMAP.md](ROADMAP.md) is the planned work.

Security problems: see [SECURITY.md](SECURITY.md), not the issue tracker.

## Setup

You need:

- **Rust**, stable, with `rustfmt` and `clippy`.
- **cargo-watch**, for `pnpm dev`: `cargo install cargo-watch`.
- **Node.js and pnpm** (CI uses Node 24). `packageManager` in `package.json` pins pnpm,
  and pnpm or Corepack switch to that version for you; Corepack must be 0.36 or later to
  run pnpm 11 (`npm install -g corepack@latest`).
- **GStreamer** 1.x with its development files and `pkg-config` (per OS below).
- **NDI Tools**, for the NDI Runtime (`libndi`), to use NDI sources and outputs. We never
  ship it; the node finds it at run time.

The gst-plugins-rs elements the node needs (NDI, fallbackswitch, WebRTC with WHIP/WHEP)
are Cargo dependencies, linked in statically at a pinned revision. Nothing to install for
those.

### macOS

The tested platform. Either:

- Homebrew: `brew install gstreamer libnice-gstreamer pkgconf`, or
- the official framework the release bundles, as CI does:

  ```
  scripts/macos/gstreamer.sh 1.28.8 target/gstreamer-1.28.8
  lib=$PWD/target/gstreamer-1.28.8/sdk/GStreamer.framework/Versions/1.0/lib
  export PKG_CONFIG_PATH=$lib/pkgconfig
  export RUSTFLAGS="-C link-arg=-Wl,-rpath,$lib"
  ```

### Linux and Windows

Being brought up for v0.2.0: CI builds and tests the node on both, but neither has been
run on real hardware yet.

- **Linux:** `apt-get install` the `packages` listed in `scripts/linux/gstreamer.sh`,
  which CI's Ubuntu 24.04 jobs run (it also tunes apt for the runner, so on your own
  machine install the list rather than running the script).

- **Windows:** `scripts/windows/gstreamer.ps1 1.28.8 C:\gstreamer\1.0\msvc_x86_64`
  installs the official MSVC build, as CI does, into a path without spaces and without
  the `gstndi`, `gstfallbackswitch` and `gstrswebrtc` plugins the node links in itself.
  Then put its `bin` on `PATH` and point `PKG_CONFIG_PATH` at its `lib\pkgconfig`.

Reports of what it takes on your distro or machine are useful.

## Running it

```
pnpm install     # also points git at .githooks
pnpm build:ui    # once: the node embeds ui/dist and won't compile without it
pnpm dev
```

`pnpm dev` runs the node on :7700 as a controller (`cargo watch`, so it rebuilds and
restarts on changes; its database is `capture-room.db` in the repo root) and the Vite UI,
which proxies `/api` and `/ws` to the node. Open the URL Vite prints.

Every node needs an access token, and a node run from a terminal has no menu bar to sign
a browser in, so it prints a sign-in link when it starts:

```
  Sign in on this machine (once, within 10 minutes):
  http://localhost:7700/api/v1/auth/link?secret=…
```

Open it in the browser you'll use. It works once, for 10 minutes, from this machine only;
it sets the sign-in cookie for `localhost` and redirects to the node's embedded UI. Then
go back to the Vite URL: the cookie applies there too. Each restart prints a new link, but
the cookie stays, so you only need one.

Missed it? Paste the token into the sign-in page instead:

```
cargo run -p capture-room -- access-token show --db capture-room.db
```

(Without `--db` it shows an installed service's token, if there is one.)

For a second node, run another instance with its own port and database, e.g.
`cargo run -p capture-room -- --port 7701 --db dev-node.db`, and add it from the
controller with its pairing code (`access-token show --db dev-node.db` prints it).
`CAPTURE_ROOM_NODE=localhost:7701 pnpm dev:ui` points the UI at it directly.

## Before you push

```
pnpm check       # cargo fmt --check, clippy -D warnings, cargo test, UI type-check
```

CI runs the same and more: formatting; clippy and tests on Linux, Windows and macOS (the last
two against the official GStreamer packages); the dependency license check below; that the generated TS
types are current; and a UI type-check and build.

**`pnpm types`** regenerates `ui/src/types/generated/` from the Rust API types (ts-rs).
Run it after changing a type in `node/src/api/` that the UI sees, and commit the result;
CI fails if they're stale.

**`pnpm build`** is a release build with the UI embedded. The macOS app is packaged by
`scripts/macos/package.sh <aarch64|x86_64>`, the Linux one by `scripts/linux/package.sh`
(on Ubuntu 24.04 with the packages above, as `release.yml` does) and the Windows one by
`scripts/windows/package.ps1`. `scripts/install.sh` installs the first two and
`scripts/windows/install.ps1` the third; `--archive dist/capture-room-<os>-<arch>.tar.gz` installs a local build.
`scripts/smoke-test.sh <archive>` (`scripts/windows/smoke-test.ps1` on Windows) unpacks
one into a temp folder and checks it starts and answers, as `release.yml` does before
publishing.

## Branches and pull requests

`main` is the only long-lived branch. Releases are tags on it, so what's merged reaches no
one until the next release.

- Work on a short-lived branch (from a fork, unless you have write access) and open a pull
  request against `main`. Everyone does, maintainers too: nothing is pushed to `main`
  directly.
- A fix, a typo or docs can go straight to a pull request. For anything bigger, open an
  issue first so we agree on the shape before you build it.
- CI must pass on every platform before a pull request merges. Say in it how you tested,
  and on what OS and hardware: much of this can only be checked on real devices.
- Pull requests are squash-merged, and the title becomes the commit on `main`, so write it
  like the history: `area: what changed`, e.g. `replay: trim an export's sound at the out
  point too`.
- Migrations in a release never change (sqlx checks their checksums). A schema change
  goes in a new migration; while a release is in progress, keep its changes in one.

A fix for an older release, once `main` has moved on, goes on a `release/X.Y` branch made
from its tag, and is tagged there.

## Issues and releases

Each release has a milestone; issues without one are the backlog. Labels say what an issue
is (`bug`, `enhancement`, `documentation`), where (`area: …`), on which OS
(`platform: …`), and what it waits for (`needs-repro`, `needs-hardware`). Questions go to
[Discussions](https://github.com/JavenKazebee/capture-room/discussions).

Versions follow semver; before 1.0, a minor release may break things. A controller and
its nodes must run the same version: it won't use a node on another one until it's
updated. Release notes are the
pull requests' titles, grouped by label, under highlights written for the people running
the shows.

## Licenses of dependencies

The app ships the license text of every Rust crate it builds in, generated by
[cargo-about](https://github.com/EmbarkStudios/cargo-about) from `about.toml` and
`about.hbs`. Install it with `cargo install cargo-about --locked --features cli`
(`package.sh` needs it).

`about.toml` lists the licenses we accept. A new dependency under any other license fails
CI ("Dependency licenses"). Check it locally:

```
cargo about generate --locked about.hbs > /dev/null
```

Don't add a license to the list to make it pass without checking that it's compatible
with the GPL; say so in the PR if you're unsure. npm packages in the UI bundle get their
notices from the Vite build (`third-party-licenses.txt`).

## Git hooks

`pnpm install` sets `core.hooksPath` to `.githooks`. The pre-commit hook refuses staged
Rust code that isn't formatted (`cargo fmt --all`), and reminds you to run `pnpm types`
when `node/src/api/` changed.

## License

Capture Room is GPL-3.0-or-later. By contributing you agree that your contribution is
licensed under the [GNU General Public License v3.0 or later](LICENSE).
