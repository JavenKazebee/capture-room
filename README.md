# Capture Room

Multi-feed video capture, playout and instant replay for live broadcast, on one machine
or a cluster of nodes managed from one web UI. It records NDI, SDI (Decklink), cameras,
streams, WHIP and files; plays out to NDI, SRT, RTSP, WHEP, a display or SDI; keeps
rolling replay buffers; and runs schedules. macOS first.

## Install (macOS)

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
```

It installs `Capture Room.app`, runs it as a login service with a menu bar icon, and
updates itself. Open the UI from the menu bar.

## Develop

Needs Rust, pnpm and GStreamer (with gst-plugins-rs' NDI, fallbackswitch and WebRTC
elements; the NDI Runtime comes from NDI Tools).

```
pnpm install     # also points git at .githooks
pnpm dev         # node on :7700 as a controller, plus the Vite UI
pnpm check       # fmt, clippy, tests, UI type-check (what CI runs)
pnpm types       # regenerate ui/src/types/generated/ after changing API types
pnpm build       # release build with the UI embedded
```

## Docs

- [ARCHITECTURE.md](ARCHITECTURE.md) — how it works; module detail is in each module's
  `//!` docs.
- [ROADMAP.md](ROADMAP.md) — open work.

## License

Copyright © 2026 Javen Kazebee.

Capture Room is free software under the [GNU General Public License v3.0 or later](LICENSE).
The macOS app bundles GStreamer, including GPL encoders (x264, x265);
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists what it ships with, under which
licenses, and where to get the source.
