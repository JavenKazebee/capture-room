# Capture Room

> [!WARNING]
> **Alpha.** Capture Room may be unstable, and you may run into bugs.

Capture Room records, plays back and replays live video for productions with many
cameras. Run it on a single computer, or on several working together, all controlled from
one page in your browser. Record every feed at once, send video back out to screens and
other systems, and cut instant replays while the show is on. It runs on macOS, Windows
and Linux.

## Prerequisites

[NDI Tools](https://ndi.video/tools/), which installs the NDI Runtime. Without it the node
starts but has no NDI sources or outputs.

## Install (macOS)

Needs macOS 11 or later, on Apple Silicon or Intel.

<!-- TODO(v0.2.0): Windows requirements (Windows 10/11 x64). -->

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
```

It installs `Capture Room.app` in `/Applications` (or `~/Applications`), runs it as a
login service with a menu bar icon, and updates itself. Options go after `bash -s --`:

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash -s -- --controller
```

- `--controller` — make this node a controller (can also be turned on later on the Nodes page)
- `--port <n>` — API and UI port (default 7700)
- `--version <tag>` — install that release instead of the latest, e.g. `v0.2.0`

## Install (Linux)

Needs Ubuntu 24.04 or later, or Debian 13 or later, on x86_64. Other distros may work
with GStreamer 1.24 or later but aren't tested.

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
```

Run it as the user the node should run as, not root. It installs the distro's GStreamer
packages with `apt` (asking for your password with `sudo` if any are missing), puts the
app in `~/.local/share/capture-room/app` with `capture-room` linked into `~/.local/bin`,
and runs it as a systemd user service that updates itself. It turns on lingering
(`loginctl enable-linger`) so the node starts at boot, before anyone logs in. The same
options as on macOS go after `bash -s --`.

There's no tray icon. On the machine itself, `capture-room` (no arguments) opens the UI
signed in; from another computer, sign in with the access token
(`capture-room access-token show`). Capture devices (`/dev/video*`) need the user in the
`video` group on a headless machine; Setup › Checks says so. NDI Tools isn't made for
Linux: for NDI, install the [NDI SDK for Linux](https://ndi.video/for-developers/ndi-sdk/)
and copy its `libndi.so.6` into `/usr/local/lib` (then `sudo ldconfig`).

```
systemctl --user status capture-room     # running?
journalctl --user -u capture-room        # service events; the node's own log is in
                                         # ~/.local/state/capture-room/capture-room.log
```

<!-- TODO(v0.2.0): Windows install. -->

## Getting started

1. **Open the UI.** Click the menu bar icon › Open Capture Room; it opens the browser
   already signed in. A node without a menu bar (run from a terminal) prints a one-time
   sign-in link instead, good for 10 minutes. From another computer, go to
   `https://<machine>:7700`, accept the self-signed certificate, and sign in with the
   access token (menu bar › Copy Access Token, or `capture-room access-token show`, below).
2. **Set the access token on the controller** (Settings › Access token) before adding
   nodes, if you want one of your own. Nodes you add take the controller's token, and a
   change made later is pushed to them.
3. **Add nodes.** On the controller's Nodes page, turn on Controller. Nodes on the same
   network appear under Found on the network; Approve each with its pairing code. A node
   on another subnet: Add node, with its address (`192.168.1.20:7700`) and pairing code.
   The pairing code is in that node's menu bar (Pairing Code), or run
   `capture-room access-token show` on it. It changes once used. A node already using the
   controller's token needs no code.
4. **WHIP from OBS.** Each WHIP source has its own port and bearer token, shown in the
   source's settings with the address to publish to:
   `http://<node>:<port>/whip/endpoint` (port 8890 by default). In OBS 30 or later:
   Settings › Stream, Service: WHIP, Server: that address, Bearer Token: the source's
   token. One publisher at a time.

On macOS the app doesn't put `capture-room` on your `PATH` (on Linux it's in
`~/.local/bin`); from a terminal run it as

```
"/Applications/Capture Room.app/Contents/MacOS/capture-room" access-token show
```

(or from `~/Applications` if it was installed there).

Setup › Checks shows what a node is missing (permissions, NDI, the clock) and, from a
controller, which of its ports are reachable.

## Ports

Allow these in a firewall between nodes, controllers and the browsers that use them.

| Port | Protocol | What | When |
|---|---|---|---|
| 7700 | TCP | API, web UI (HTTPS; plain HTTP is redirected, except WHEP and from localhost), WHEP outputs | Always. `--port` |
| 5353 | UDP | mDNS, to find nodes (`_capture-room._tcp`) | Always |
| 7800 | UDP | Controller's clock, which nodes follow | Controllers. API port + 100, or `--clock-port` |
| 8554 | TCP | RTSP outputs | Once a channel has an RTSP output |
| 8890 (default) | TCP | WHIP ingest, plain HTTP | Per WHIP source, its own port |
| set in the URL | UDP | SRT listeners (`srt://:9000`), UDP stream inputs | Per stream input or SRT output that listens |
| 319, 320 | UDP | PTP | When the clock follows a PTP grandmaster |
| ephemeral | UDP | WebRTC media (WHIP and WHEP) and RTSP media over UDP | Per viewer or publisher |
| 5959, 5960 and up | TCP/UDP | NDI (per NDI's documentation) | NDI sources and outputs |

Nodes also make outgoing HTTPS requests to GitHub to check for updates.

## Gatekeeper

The app is ad hoc signed, not notarized. Installed with the `curl` command above it isn't
quarantined, so it opens without a prompt. If you downloaded the archive in a browser,
macOS will refuse to open it: try to open it once, then System Settings › Privacy &
Security › Open Anyway (on macOS 14 and earlier, right-click › Open also works), or run

```
xattr -dr com.apple.quarantine "/Applications/Capture Room.app"
```

## Uninstall (macOS)

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash -s -- --uninstall
```

Stops the service and removes the app and its login item. The database stays in
`~/Library/Application Support/Capture Room`; add `--purge` to also remove it and the
logs in `~/Library/Logs/Capture Room`. Recordings in `~/capture-room` are never removed.

## Uninstall (Linux)

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash -s -- --uninstall
```

Stops and removes the service and the app, and turns lingering off again if the install
turned it on. The database stays in `~/.local/share/capture-room`; add `--purge` to also
remove it and the logs in `~/.local/state/capture-room`. The GStreamer packages stay
installed. Recordings in `~/capture-room` are never removed.

<!-- TODO(v0.2.0): Windows uninstall. -->

## Develop

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Copyright © 2026 Javen Kazebee.

Capture Room is free software under the [GNU General Public License v3.0 or later](LICENSE).
The macOS app bundles GStreamer, including GPL encoders (x264, x265); on Linux it uses the
distro's GStreamer.
