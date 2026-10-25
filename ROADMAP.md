# Capture Room — Roadmap

Open work, roughly in priority order. [ARCHITECTURE.md](ARCHITECTURE.md) owns design;
shipped history is in git.

_Last updated: 2026-10-25_

## v0.2.0 — first public alpha (macOS, Linux, Windows)

Still testing: no venue runs it, and 0.1.0 was a test build, so nothing has to stay
compatible with it. Plain `v0.2.0` (0.x already means unstable); "alpha" goes in the
README banner and the release title. Not a GitHub pre-release: the updater and the
install scripts read `releases/latest`. Ad hoc signed; the README covers Gatekeeper and
SmartScreen.

### 1. Builds on all three

- **Linux** — find what's failing: the CI `node` job, a release-style build, or a local
  build on the AMD desktop. NDI, fallbackswitch and WebRTC are linked in statically, so
  only core GStreamer and rtsp-server come from the distro.
- **Windows** — a CI job (build, clippy, tests) against the official MSVC GStreamer, so
  platform code can't rot.

### 2. Linux

Written, so far only built and linted in CI: Ubuntu 24.04+ and Debian 13+ on x86_64 with
the distro's GStreamer (1.24+), which `install.sh` installs with apt; `service/linux.rs`
(a systemd user unit, XDG paths, lingering); the updater; `scripts/linux/package.sh`
and the release job; a check that `/dev/video*` can be opened. Left:

- **Try it on the AMD desktop** — install, reboot without logging in (lingering), update
  through `CAPTURE_ROOM_RELEASES`, `--uninstall` and `--purge`; and on a desktop session,
  `capture-room` with no arguments opening the browser signed in.
- **Plugin clashes** — the distro's gst-plugins-rs (`gstreamer1.0-plugins-rs` where
  packaged), if installed, has its own `ndi`, `fallbackswitch` and `rswebrtc`, as the
  Windows install does. Check whether two of each break the node.
- **Display outputs** under the service only get a screen when the desktop imports
  `DISPLAY`/`WAYLAND_DISPLAY` into the user manager; check, or document.

### 3. Windows

- **Package** — the exe plus the GStreamer MSVC runtime. Its notices differ from macOS
  (cargo-about plus GStreamer's licenses).
- **Service** — a per-user logon task (so the tray icon works), data under
  `%LOCALAPPDATA%`.
- **Updater** — rename the running exe, put the new one in place, relaunch. The archive
  format has to match what `update.rs` downloads (`.tar.gz` today; Windows' `tar` reads it).
- **Tray** — the menu `tray.rs` already gates on for Windows.
- **Install** — `install.ps1` with the same options as `install.sh`.
- **Checks** — camera and microphone privacy toggles, firewall rule.

### 4. Release pipeline

- `release.yml`: macOS, Linux (done) and Windows jobs each build and sign
  `capture-room-<os>-<arch>.tar.gz`; one publish job, after all of them, writes
  `latest.json` once and creates the release (`package.sh` writes `latest.json` today).
- Dry run with `workflow_dispatch`, then on each OS: install, then update to a higher
  version served from a folder via `CAPTURE_ROOM_RELEASES`.

### 5. Docs

- **README** — alpha banner and what's untested on real hardware; supported OS versions
  and architectures (macOS 11+ Intel and Apple Silicon, Linux distros, Windows 10/11
  x64); NDI Tools (for the NDI Runtime) as a prerequisite; every port it opens (needs a
  pass over the code); getting started (menu bar, tray or the printed sign-in link; add
  nodes with their pairing codes; WHIP tokens in OBS); Gatekeeper and SmartScreen;
  uninstalling, per OS.
- **CONTRIBUTING.md** — per-OS setup, `pnpm check`, `pnpm types`, cargo-about for
  packaging, the sign-in link `pnpm dev` prints.
- **SECURITY.md** — GitHub private vulnerability reporting.
- **Issue templates** — bug (version, OS and arch, hardware involved), feature request,
  blank issues off.
- **Release notes** — for 0.1.0 testers: set the controller's access token first or each
  node needs pairing once; every WHIP source has a new token to paste into OBS.

### 6. Testing before the tag

On the Mac laptop, the Intel Mac Pro, and the AMD desktop under Linux and Windows:
install, open the UI, pair a node, record NDI and WHIP from OBS, update, uninstall. A Mac
controller with the AMD desktop as a node covers the two-machine items below.

#### Mac Pro (Intel, two Decklink 8K Pro G2s)

- **Decklink sources and outputs** — built, untested on hardware. Deliver through the
  updater and test. Check that MOV recordings carry the input's RP188 timecode, including
  from an interlaced input (1080i59.94).
- **Display output** — try an HDMI→SDI converter.
- **Device capture on real hardware** — so far only OBS Virtual Camera. A camera is now
  found again by unique ID on each restart (it used to reopen by index, so unplugging
  another camera could switch it); check that by unplugging a second camera mid-recording.

#### To verify

- PTP against a real grandmaster (a Dante network); the helper's permissions on macOS.
- NDI timing across two machines (measured on one only).
- Approving nodes found over mDNS, with two machines (mDNS between processes on one Mac
  doesn't work, so only adding by address was tested end to end); the HTTPS UI and WHEP
  through the controller in Safari and Chrome after accepting the certificate.
- Re-run intra H.264 and MJPEG replay benchmarks (they predate the codec settings).

### 7. Tag `v0.2.0`

Bump `node/Cargo.toml`, tag, check the release has all assets and `latest.json`.

## Next

- **Operator help** — in the app, not the repo, for the people running a show: a short
  help panel per workspace (Record, Playback, Replay, Recordings, Schedule) on what it's
  for and the usual steps; a keyboard shortcut sheet; empty states that say what to do
  first; and first-run help (permissions, Setup › Checks, adding nodes, the access token).
  Field-level help already exists (`HelpTip`, `lib/fieldHelp`).

## Later

- **Packaging** — Developer ID (stable signature so permissions survive updates,
  notarization, hardened runtime). Needed before 1.0; until then the README covers Gatekeeper.
- **Authentication** — optional: trust the controller's certificate in the keychain at
  install, so browsers on that Mac don't warn.
- **Hardware encoders** — NVENC, QSV and VA-API in `RecordingProfile::encoders` with the
  same fallback. Only VA-API is testable today (AMD). Re-run benchmarks after. Encode
  sharing between identical legs: decided against for now.
- **Timecode from sources** — LTC from an audio channel, VITC. (A frame's own timecode,
  such as Decklink RP188, already reaches recordings.)
- **Replay** — jog wheels / Stream Deck via WebHID or MIDI.
- **Playback** — AJA outputs, interlaced SDI out, slates.
- **Path tokens** — `{source_name}` (filename-safe), `{preset}`.
- **Recordings** — previews of growing files.
- **Sources** — AJA (`ajasrc`); web page (`wpesrc`, Linux only, niche).
- **Linux and Windows gaps** — display output screen sleep is only handled on macOS.
  Linux screen capture: X11 `ximagesrc`; Wayland via an xdg-desktop-portal ScreenCast
  session (`ashpd`, `persist_mode=2`, restore token in the source config) into
  `pipewiresrc`, offered in the device list.
