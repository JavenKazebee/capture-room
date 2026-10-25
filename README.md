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

<!-- TODO(v0.2.0): Linux requirements (distros, GStreamer minimum). -->

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

<!-- TODO(v0.2.0): Linux install. -->

## Install (Windows)

Needs Windows 10 or 11, x64. In PowerShell (not as administrator):

```
irm https://github.com/JavenKazebee/capture-room/releases/latest/download/install.ps1 | iex
```

It installs Capture Room for your user in `%LOCALAPPDATA%\Capture Room`, with GStreamer
inside, starts it when you sign in (a Task Scheduler logon task) with a tray icon, adds it
to the Start menu, and updates itself. With options:

```
& ([scriptblock]::Create((irm https://github.com/JavenKazebee/capture-room/releases/latest/download/install.ps1))) -Controller
```

- `-Controller`, `-Port <n>`, `-Version <tag>` — as for macOS above

The first time it starts, Windows Firewall asks whether to let it in; allowing that takes
an administrator. Without it other machines can't reach the node; Setup › Checks has the
command that allows it.

## Getting started

1. **Open the UI.** Click the menu bar icon (the tray icon on Windows) › Open Capture
   Room; it opens the browser already signed in. A node without a menu bar (run from a
   terminal) prints a one-time sign-in link instead, good for 10 minutes. From another computer, go to
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

The app doesn't put `capture-room` on your `PATH`; from a terminal run it as

```
"/Applications/Capture Room.app/Contents/MacOS/capture-room" access-token show
```

(or from `~/Applications` if it was installed there). On Windows:

```
& "$env:LOCALAPPDATA\Capture Room\app\bin\capture-room.exe" access-token show
```

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

## SmartScreen

The Windows app isn't code-signed. Installed with the PowerShell command above it isn't
marked as downloaded, so it starts without a prompt. If you downloaded the archive in a
browser, Windows may stop it with "Windows protected your PC": click More info › Run
anyway, or unblock the files first:

```
Get-ChildItem -Recurse "$env:LOCALAPPDATA\Capture Room\app" | Unblock-File
```

If Smart App Control is on (Windows 11), it may block the app however it was installed,
because it isn't signed.

## Uninstall (macOS)

```
curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash -s -- --uninstall
```

Stops the service and removes the app and its login item. The database stays in
`~/Library/Application Support/Capture Room`; add `--purge` to also remove it and the
logs in `~/Library/Logs/Capture Room`. Recordings in `~/capture-room` are never removed.

## Uninstall (Windows)

```
& ([scriptblock]::Create((irm https://github.com/JavenKazebee/capture-room/releases/latest/download/install.ps1))) -Uninstall
```

Stops it and removes the app, its logon task and Start menu shortcut. The database stays
in `%LOCALAPPDATA%\Capture Room\data`; add `-Purge` to also remove it and the logs in
`%LOCALAPPDATA%\Capture Room\logs`. Recordings in `~\capture-room` are never removed.

<!-- TODO(v0.2.0): Linux uninstall. -->

## Develop

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Copyright © 2026 Javen Kazebee.

Capture Room is free software under the [GNU General Public License v3.0 or later](LICENSE).
The macOS and Windows apps bundle GStreamer, including GPL encoders (x264, x265).
