# Capture Room — Roadmap

Open work, roughly in priority order. [ARCHITECTURE.md](ARCHITECTURE.md) owns design;
shipped history is in git.

_Last updated: 2026-10-18_

## Shipped

Test, file, NDI, stream, device, WHIP and Decklink sources; multi-leg recording with
crash-safe files and splitting; benchmark and capacity estimates; Recordings view with
browser preview; playout channels with gapless playlists and NDI, SRT, RTSP, WHEP,
display and Decklink outputs; shared node clock, synchronized start and MOV timecode;
Setup › Checks; pushed thumbnails up to 30 fps; instant replay (buffers, clips, groups, export); macOS installer,
login service, menu bar icon and signed self-updates; a shared access token over TLS, with
pinned node keys, approval of found nodes, lost-token recovery (menu bar, CLI) and node
cards saying why a node is unreachable (offline, changed key, another node, token); a Logs
view of every node's log lines; schedules that record and play out once or weekly.

## Now: Mac Pro (Intel, two Decklink 8K Pro G2s)

- **Decklink sources and outputs** — built, untested on hardware. Deliver through the
  updater and test. Check that MOV recordings carry the input's RP188 timecode, including
  from an interlaced input (1080i59.94).
- **Display output** — try an HDMI→SDI converter.
- **Device capture on real hardware** — so far only OBS Virtual Camera. A camera is now
  found again by unique ID on each restart (it used to reopen by index, so unplugging
  another camera could switch it); check that by unplugging a second camera mid-recording.

## To verify

- PTP against a real grandmaster (a Dante network); the helper's permissions on macOS.
- NDI timing across two machines (measured on one only).
- Approving nodes found over mDNS, with two machines (mDNS between processes on one Mac
  doesn't work, so only adding by address was tested end to end); the HTTPS UI and WHEP
  through the controller in Safari and Chrome after accepting the certificate.
- Re-run intra H.264 and MJPEG replay benchmarks (they predate the codec settings).

## Next

- **Authentication** — optional: trust the controller's certificate in the keychain at
  install, so browsers on that Mac don't warn.
- **Packaging** — Developer ID (stable signature so permissions survive updates,
  notarization, hardened runtime); rolling back an update that won't start; log rotation.
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

## Deferred: Linux and Windows

macOS first (as of 2026-10-11). When picked up:

- Installers behind the same `service` and `update` modules: a systemd user unit with
  distro GStreamer; on Windows a logon task, tray icon and renaming the running exe.
- Checks: Linux `video` group, PTP-helper capabilities, firewall; Windows privacy toggles.
- Display output: screen sleep is only handled on macOS.
- Linux screen capture: X11 `ximagesrc`; Wayland via an xdg-desktop-portal ScreenCast
  session (`ashpd`, `persist_mode=2`, restore token in the source config) into
  `pipewiresrc`, offered in the device list.
- A Windows CI job (build + clippy) so platform device code can't rot.
