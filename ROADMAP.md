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

- **GStreamer** — distro packages (decide the minimum: Ubuntu 24.04 ships 1.24; the
  crates ask for 1.16+) or bundled like macOS. Distro first; bundle only if a needed
  plugin or version is missing.
- **Service** — `service/linux.rs`: a systemd user unit, XDG data and log paths,
  `loginctl enable-linger` so a headless node starts at boot. Headless sign-in already
  works (the node prints a link).
- **Updater** — swap the binary and restart the unit; drop the "not on this OS" check.
- **Install** — Linux in `install.sh` (or its own script): install, `--version`,
  `--uninstall`, `--purge`.
- **Checks** — `video` group, PTP-helper capabilities, firewall (ufw/firewalld).

### 3. Windows

Written, compiled only by CI; nothing has run on Windows yet. Try on the AMD desktop:

- **Package** (`scripts/windows/package.ps1`) — the exe and the GStreamer MSVC runtime in
  one folder, trimmed like macOS. Check that every required plugin loads from it (the
  smoke test only runs `--version`), and its size.
- **Service** (`service/windows.rs`) — a supervisor (`service run`) that restarts the node
  and writes and rotates its log, started at sign-in by a logon task through the windowless
  `capture-room-launcher.exe`. Check: no console window at sign-in; the tray icon (white,
  black on a light taskbar); Quit stays quit; a killed node comes back; Start at Login.
  Stopping kills the node, so recordings in progress aren't finalised: find a graceful
  stop.
- **Updater** — moves the app's files aside one by one (a running exe and its DLLs can be
  renamed, not replaced) and the new ones in, then the supervisor restarts from them.
- **Install** — `install.ps1`: install, `-Version`, `-Uninstall`, `-Purge`, reinstall over
  a running node, the Start menu shortcut.
- **Checks** — camera and microphone privacy switches (read from the registry), and a
  Windows Firewall hint with the command allowing the exe.

### 4. Release pipeline

- `release.yml`: macOS, Linux and Windows jobs each build and sign
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
