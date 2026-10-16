#!/usr/bin/env bash
# Install, update or remove Capture Room on this Mac. It runs as a service in
# your login session (a LaunchAgent): it starts when you log in and restarts
# if it stops.
#
#   curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
#
# Options (after `bash -s --` when piped):
#   --controller        make this node a controller (remembered)
#   --port <n>          API and UI port (default 7700)
#   --version <tag>     install that release instead of the latest, e.g. v0.2.0
#   --archive <file>    install a local build (capture-room-macos-<arch>.tar.gz)
#   --uninstall         stop it and remove the app and service; keeps its data
#   --purge             with --uninstall, also remove its database and logs
# Recordings in ~/capture-room are never touched.
set -euo pipefail

repo=JavenKazebee/capture-room
label=com.capture-room.node
app_name="Capture Room.app"
support="$HOME/Library/Application Support/Capture Room"
logs="$HOME/Library/Logs/Capture Room"
agent="$HOME/Library/LaunchAgents/$label.plist"
domain="gui/$(id -u)"

controller=false port=7700 version="" archive="" uninstall=false purge=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --controller) controller=true ;;
        --port) port=${2:?--port needs a number}; shift ;;
        --version) version=${2:?--version needs a tag}; shift ;;
        --archive) archive=${2:?--archive needs a file}; shift ;;
        --uninstall) uninstall=true ;;
        --purge) purge=true ;;
        *) echo "Unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

[[ $(uname -s) == Darwin ]] || fail "This installer is for macOS."

stop_service() {
    launchctl bootout "$domain/$label" 2>/dev/null || return 0
    # bootout returns before the process has gone.
    for _ in $(seq 50); do
        launchctl print "$domain/$label" >/dev/null 2>&1 || return 0
        sleep 0.2
    done
}

installed_app() {
    for dir in /Applications "$HOME/Applications"; do
        [[ -d "$dir/$app_name" ]] && { echo "$dir/$app_name"; return; }
    done
    return 0
}

if $uninstall; then
    say "Removing Capture Room"
    stop_service
    rm -f "$agent"
    app=$(installed_app)
    [[ -n $app ]] && rm -rf "$app"
    rm -rf "$HOME/Library/Caches/capture-room"
    if $purge; then
        rm -rf "$support" "$logs"
        echo "Removed the app, the service, its database and its logs."
    else
        echo "Removed the app and the service. Kept its database ($support)"
        echo "and logs ($logs); --purge removes them too."
    fi
    echo "Recordings in ~/capture-room are untouched."
    exit 0
fi

# ── Fetch ────────────────────────────────────────────────────────────────────
case $(uname -m) in
    arm64) arch=aarch64 ;;
    x86_64)
        # A shell under Rosetta reports x86_64 on Apple silicon.
        if [[ $(sysctl -n sysctl.proc_translated 2>/dev/null) == 1 ]]; then arch=aarch64; else arch=x86_64; fi ;;
    *) fail "Unsupported architecture $(uname -m)" ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
asset=capture-room-macos-$arch.tar.gz

if [[ -n $archive ]]; then
    [[ -f $archive ]] || fail "No such file: $archive"
    cp "$archive" "$work/$asset"
else
    if [[ -n $version ]]; then
        base=https://github.com/$repo/releases/download/$version
    else
        base=https://github.com/$repo/releases/latest/download
    fi
    say "Downloading $asset${version:+ ($version)}"
    curl -fL --progress-bar -o "$work/$asset" "$base/$asset" || fail "Download failed: $base/$asset"
    curl -fsSL -o "$work/$asset.sha256" "$base/$asset.sha256" || fail "Download failed: $base/$asset.sha256"
    (cd "$work" && shasum -a 256 -c "$asset.sha256" >/dev/null) || fail "Checksum mismatch: the download is damaged."
fi
tar -xzf "$work/$asset" -C "$work"
[[ -d "$work/$app_name" ]] || fail "The archive has no $app_name"

# ── Install ──────────────────────────────────────────────────────────────────
dest=/Applications
[[ -w $dest ]] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
app="$dest/$app_name"
new_version=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$work/$app_name/Contents/Info.plist")

stop_service
old=$(installed_app)
[[ -n $old ]] && rm -rf "$old"
mv "$work/$app_name" "$app"

mkdir -p "$support" "$logs" "$(dirname "$agent")"
args=("$app/Contents/MacOS/capture-room" --db "$support/capture-room.db" --port "$port")
$controller && args+=(--controller)

xml() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' <<<"$1"; }
{
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$label</string>
	<key>ProgramArguments</key>
	<array>
EOF
    for arg in "${args[@]}"; do printf '\t\t<string>%s</string>\n' "$(xml "$arg")"; done
    cat <<EOF
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>$(xml "$logs/capture-room.log")</string>
	<key>StandardErrorPath</key>
	<string>$(xml "$logs/capture-room.log")</string>
</dict>
</plist>
EOF
} >"$agent"
plutil -lint -s "$agent" || fail "Wrote an invalid $agent"

launchctl bootstrap "$domain" "$agent" || fail "launchctl couldn't start the service"

say "Starting Capture Room $new_version"
for _ in $(seq 60); do
    if curl -fs -o /dev/null "http://localhost:$port/"; then
        host=$(scutil --get LocalHostName 2>/dev/null || hostname -s)
        say "Capture Room $new_version is running: http://$host.local:$port"
        echo "App:  $app"
        echo "Logs: $logs/capture-room.log"
        echo "Stop: launchctl bootout $domain/$label   (starts again at login)"
        exit 0
    fi
    sleep 0.5
done
fail "It didn't answer on port $port within 30 s; see $logs/capture-room.log"
