# Capture Room — Roadmap

Planned work beyond the current release. Work on the release itself is tracked in GitHub
issues.

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
