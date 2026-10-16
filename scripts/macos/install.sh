#!/usr/bin/env bash
# Install or remove Capture Room on this Mac. The app then installs itself as a
# login service (`capture-room service install`) and updates itself.
#
#   curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
#
# Options (after `bash -s --` when piped):
#   --controller        make this node a controller (remembered)
#   --port <n>          API and UI port (default 7700, or what it was)
#   --version <tag>     install that release instead of the latest, e.g. v0.2.0
#   --archive <file>    install a local build (capture-room-macos-<arch>.tar.gz)
#   --uninstall         stop it and remove the app and service; keeps its data
#   --purge             with --uninstall, also remove its database and logs
# Recordings in ~/capture-room are never touched.
set -euo pipefail

repo=JavenKazebee/capture-room
app_name="Capture Room.app"

service_args=() version="" archive="" uninstall=false purge=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --controller) service_args+=(--controller) ;;
        --port) service_args+=(--port "${2:?--port needs a number}"); shift ;;
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

installed_app() {
    for dir in /Applications "$HOME/Applications"; do
        [[ -d "$dir/$app_name" ]] && { echo "$dir/$app_name"; return; }
    done
    return 0
}

if $uninstall; then
    app=$(installed_app)
    [[ -n $app ]] || fail "Capture Room isn't installed."
    if $purge; then
        "$app/Contents/MacOS/capture-room" service uninstall --purge
    else
        "$app/Contents/MacOS/capture-room" service uninstall
    fi
    exit 0
fi

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

dest=/Applications
[[ -w $dest ]] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
app="$dest/$app_name"
old=$(installed_app)
# The running node keeps its files open; the service restarts it from the
# new app below.
[[ -n $old ]] && rm -rf "$old"
mv "$work/$app_name" "$app"

say "Starting Capture Room (the first start takes up to a minute)"
"$app/Contents/MacOS/capture-room" service install ${service_args[@]+"${service_args[@]}"}
echo "It starts at login and has a menu bar icon. To remove it:"
echo "  curl -fsSL https://github.com/$repo/releases/latest/download/install.sh | bash -s -- --uninstall"
