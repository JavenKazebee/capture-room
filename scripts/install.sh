#!/usr/bin/env bash
# Install or remove Capture Room on this Mac or Linux machine. The app then
# installs itself as a login service (`capture-room service install`) and
# updates itself.
#
#   curl -fsSL https://github.com/JavenKazebee/capture-room/releases/latest/download/install.sh | bash
#
# Options (after `bash -s --` when piped):
#   --controller        make this node a controller (remembered)
#   --port <n>          API and UI port (default 7700, or what it was)
#   --version <tag>     install that release instead of the latest, e.g. v0.2.0
#   --archive <file>    install a local build (capture-room-<os>-<arch>.tar.gz)
#   --uninstall         stop it and remove the app and service; keeps its data
#   --purge             with --uninstall, also remove its database and logs
# Recordings in ~/capture-room are never touched.
#
# Linux: Ubuntu 24.04+, Debian 13+ or Arch and its derivatives, x86_64.
# GStreamer comes from the distro; missing packages are installed with apt or
# pacman (which asks for sudo). The node runs as a systemd user service, with
# lingering on so it starts at boot.
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
warn() { printf '\033[33m%s\033[0m\n' "$*" >&2; }
fail() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

case $(uname -s) in
    Darwin) os=macos ;;
    Linux) os=linux ;;
    *) fail "This installer is for macOS and Linux." ;;
esac

# Run the installed binary's `service uninstall`.
uninstall_with() {
    if $purge; then
        "$1" service uninstall --purge </dev/null
    else
        "$1" service uninstall </dev/null
    fi
}

## macOS

installed_app() {
    for dir in /Applications "$HOME/Applications"; do
        [[ -d "$dir/$app_name" ]] && { echo "$dir/$app_name"; return; }
    done
    return 0
}

macos_arch() {
    case $(uname -m) in
        arm64) echo aarch64 ;;
        x86_64)
            # A shell under Rosetta reports x86_64 on Apple silicon.
            if [[ $(sysctl -n sysctl.proc_translated 2>/dev/null) == 1 ]]; then echo aarch64; else echo x86_64; fi ;;
        *) fail "Unsupported architecture $(uname -m)" ;;
    esac
}

macos_install() {
    [[ -d "$work/$app_name" ]] || fail "The archive has no $app_name"
    local dest=/Applications app old
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
}

## Linux

# Where the service's `data_dir` is: the app goes in its `app` folder.
data_home=${XDG_DATA_HOME:-}
[[ $data_home == /* ]] || data_home=$HOME/.local/share
linux_app=$data_home/capture-room/app

# What the node needs at run time beyond the libraries every install has:
# GStreamer's plugins (the node checks for each element it uses when it
# starts), rtsp-server, libnice for WebRTC, PipeWire for Wayland screen
# capture, and the audio and GL plugins for devices and display outputs. NDI,
# fallbackswitch and WebRTC are built in.
apt_packages=(
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad
    gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-nice
    gstreamer1.0-alsa gstreamer1.0-pulseaudio gstreamer1.0-gl gstreamer1.0-pipewire
    libgstrtspserver-1.0-0
)
# Arch packs ALSA into base, PulseAudio into good and GL into base-libs (which
# base needs), and libnice ships its own GStreamer plugin.
pacman_packages=(
    gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly
    gst-libav gst-plugin-pipewire gst-rtsp-server libnice
)

# The package manager that installs GStreamer here, if this is a tested distro:
# apt on Ubuntu 24.04+ or Debian 13+ (GStreamer 1.24 or later), pacman on Arch
# and its derivatives (CachyOS, EndeavourOS, Manjaro), which name it in ID_LIKE.
linux_family() {
    [[ -r /etc/os-release ]] || return 0
    local id like ver
    # shellcheck source=/dev/null
    { read -r id; read -r like; read -r ver; } < <(
        . /etc/os-release && printf '%s\n' "${ID:-}" "${ID_LIKE:-}" "${VERSION_ID:-}"
    )
    case $id in
        ubuntu) at_least "$ver" 24.04 && command -v apt-get >/dev/null && echo apt ;;
        debian) at_least "$ver" 13 && command -v apt-get >/dev/null && echo apt ;;
        *)
            if [[ " $id $like " == *" arch "* ]] && command -v pacman >/dev/null; then
                echo pacman
            fi ;;
    esac
    return 0
}

linux_supported() {
    [[ -n $(linux_family) ]]
}

# Whether version $1 is $2 or later.
at_least() {
    [[ -n $1 ]] && [[ $(printf '%s\n' "$2" "$1" | sort -V | head -1) == "$2" ]]
}

linux_deps() {
    local family missing=() p
    family=$(linux_family)
    if [[ $family == pacman ]]; then
        for p in "${pacman_packages[@]}"; do
            pacman -Q "$p" >/dev/null 2>&1 || missing+=("$p")
        done
    else
        for p in "${apt_packages[@]}"; do
            dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'ok installed' || missing+=("$p")
        done
    fi
    [[ ${#missing[@]} -eq 0 ]] && return
    case $family in
        apt)
            say "Installing GStreamer from the distro's packages (sudo asks for your password)"
            sudo apt-get update </dev/null || fail "apt-get update failed"
            sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${missing[@]}" </dev/null ||
                fail "Couldn't install ${missing[*]}"
            ;;
        pacman)
            say "Installing GStreamer from the distro's packages (sudo asks for your password)"
            # No -y: syncing the database without upgrading is a partial upgrade,
            # which Arch doesn't support.
            sudo pacman -S --needed --noconfirm "${missing[@]}" </dev/null ||
                fail "Couldn't install ${missing[*]}; if a download failed, run 'sudo pacman -Syu' and try again."
            ;;
        *)
            warn "This isn't Ubuntu 24.04+, Debian 13+ or Arch, so GStreamer isn't installed for you."
            warn "The node needs GStreamer 1.24 or later with its base, good, bad, ugly, libav,"
            warn "nice, ALSA, PulseAudio, GL and PipeWire plugins and rtsp-server"
            warn "(Debian names: ${missing[*]})."
            ;;
    esac
}

linux_install() {
    [[ -x "$work/capture-room/capture-room" ]] || fail "The archive has no capture-room/capture-room"
    linux_deps
    mkdir -p "$(dirname "$linux_app")"
    # The running node keeps its files open; the service restarts it from the
    # new app below.
    rm -rf "$linux_app"
    mv "$work/capture-room" "$linux_app"

    say "Starting Capture Room (the first start takes up to a minute)"
    "$linux_app/capture-room" service install ${service_args[@]+"${service_args[@]}"} </dev/null
    echo "It runs as a systemd user service (capture-room) and starts at boot."
    echo "Open it signed in here with 'capture-room' (in ~/.local/bin), or from another"
    echo "computer sign in with the access token: capture-room access-token show"
    echo "To remove it:"
}

if [[ $os == linux ]]; then
    [[ $EUID -ne 0 ]] || fail "Run this as the user the node runs as, not root: it asks for sudo when it needs it."
fi

if $uninstall; then
    if [[ $os == macos ]]; then
        app=$(installed_app)
        [[ -n $app ]] || fail "Capture Room isn't installed."
        uninstall_with "$app/Contents/MacOS/capture-room"
    else
        [[ -x $linux_app/capture-room ]] || fail "Capture Room isn't installed."
        uninstall_with "$linux_app/capture-room"
    fi
    exit 0
fi

if [[ $os == macos ]]; then
    arch=$(macos_arch)
else
    [[ $(uname -m) == x86_64 ]] || fail "Capture Room for Linux is x86_64 only, not $(uname -m)."
    arch=x86_64
    linux_supported || warn "Only Ubuntu 24.04+, Debian 13+ and Arch are tested; carrying on."
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
asset=capture-room-$os-$arch.tar.gz

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
    if [[ $os == macos ]]; then
        (cd "$work" && shasum -a 256 -c "$asset.sha256" >/dev/null) || fail "Checksum mismatch: the download is damaged."
    else
        (cd "$work" && sha256sum -c "$asset.sha256" >/dev/null) || fail "Checksum mismatch: the download is damaged."
    fi
fi
tar -xzf "$work/$asset" -C "$work"

if [[ $os == macos ]]; then macos_install; else linux_install; fi
echo "  curl -fsSL https://github.com/$repo/releases/latest/download/install.sh | bash -s -- --uninstall"
