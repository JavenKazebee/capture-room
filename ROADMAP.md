# Capture Room — Roadmap

Open work, roughly in priority order. [ARCHITECTURE.md](ARCHITECTURE.md) owns design;
shipped history is in git.

_Last updated: 2026-10-16_

## Shipped

Test, file, NDI, stream, device, WHIP and Decklink sources; multi-leg recording with
crash-safe files and splitting; benchmark and capacity estimates; Recordings view with
browser preview; playout channels with gapless playlists and NDI, SRT, RTSP, WHEP,
display and Decklink outputs; shared node clock, synchronized start and MOV timecode;
Setup › Checks; instant replay (buffers, clips, groups, export); macOS installer,
login service, menu bar icon and signed self-updates; a shared access token.

## Now: Mac Pro (Intel, two Decklink 8K Pro G2s)

- **Decklink sources and outputs** — built, untested on hardware. Deliver through the
  updater and test. RP188 timecode isn't written to recordings yet (the compositor drops
  the meta).
- **Display output** — try an HDMI→SDI converter.
- **Device capture on real hardware** — so far only OBS Virtual Camera. A camera is now
  found again by unique ID on each restart (it used to reopen by index, so unplugging
  another camera could switch it); check that by unplugging a second camera mid-recording.

## To verify

- PTP against a real grandmaster (a Dante network); the helper's permissions on macOS.
- NDI timing across two machines (measured on one only).
- Re-run intra H.264 and MJPEG replay benchmarks (they predate the codec settings).

## Next

- **Authentication, next steps** — a shared access token is built and off by default.
  Left: TLS (the token crosses the LAN in the clear); only trust nodes that were added or
  approved, not anything that announces over mDNS; a way to recover a lost token without
  editing the database (menu bar or CLI).
- **Packaging** — Developer ID (stable signature so permissions survive updates,
  notarization, hardened runtime); rolling back an update that won't start; log rotation.
- **Hardware encoders** — NVENC, QSV and VA-API in `RecordingProfile::encoders` with the
  same fallback. Only VA-API is testable today (AMD). Re-run benchmarks after. Encode
  sharing between identical legs: decided against for now.
- **Timecode from sources** — LTC from an audio channel, VITC, Decklink RP188; a frame
  that has timecode keeps it.
- **Scheduler** — on the controller, sending ordinary start/stop commands. Adds a
  Schedules view.
- **Subscription thumbnail WebSocket** — request-per-frame thumbnails don't scale past
  ~10–15 fps with many feeds. The client subscribes to visible sources and an fps; frames
  are pushed as binary messages, relayed by a controller only for watched sources.
  Removes `thumbnail.updated`. Not MJPEG: each stream holds one of the browser's 6
  connections.
- **Replay** — jog wheels / Stream Deck via WebHID or MIDI.
- **Playback** — AJA outputs, interlaced SDI out, slates.
- **Path tokens** — `{source_name}` (filename-safe), `{preset}`.
- **Recordings** — previews of growing files.
- **Logs view** — aggregated, filtered by node and level.
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
- macOS and Windows CI jobs (build + clippy) so platform device code can't rot.
