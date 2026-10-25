#!/usr/bin/env bash
# Build the node for Linux against the distro's GStreamer and pack it as
# dist/capture-room-linux-x86_64.tar.gz (+ .sha256). GStreamer isn't bundled:
# install.sh installs the distro's packages. Build on the oldest supported
# distro (Ubuntu 24.04), so the binary needs no newer glibc or GStreamer than
# every supported distro has.
#
# The archive holds one folder, capture-room/, with the binary and licenses/.
# install.sh moves it to ~/.local/share/capture-room/app, and the updater
# (node/src/update.rs) swaps it in there.
#
# Usage: scripts/linux/package.sh
# Env:   MINISIGN_KEY_FILE release key: signs the archive (.minisig), which
#                          the updater requires
#        SKIP_UI=1         use the existing ui/dist
# Needs the -dev packages ci.yml's Linux job installs, and cargo-about
# (`cargo install cargo-about --locked --features cli`) for the Rust crates'
# licenses.
set -euo pipefail

arch=$(uname -m)
[[ $arch == x86_64 ]] || { echo "Linux builds are x86_64 only, not $arch" >&2; exit 1; }

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' node/Cargo.toml | head -1)

command -v cargo-about >/dev/null || { echo "needs cargo-about: cargo install cargo-about --locked --features cli" >&2; exit 1; }

if [[ -z ${SKIP_UI:-} ]]; then
    pnpm install --frozen-lockfile
    pnpm build:ui
fi

cargo build --release --locked -p capture-room

stage=target/package/linux-$arch
app=$stage/capture-room
rm -rf "$stage"
mkdir -p "$app"
cp target/release/capture-room "$app/"

# The updater reads the version the same way before swapping it in.
built=$("$app/capture-room" --version | cut -d' ' -f2)
[[ $built == "$version" ]] || { echo "the binary says $built, node/Cargo.toml $version" >&2; exit 1; }

# Licenses of the app and of what's built into it (THIRD_PARTY_NOTICES.md
# lists them). GStreamer comes from the distro, under its packages' licenses.
licenses=$app/licenses
mkdir -p "$licenses"
cp LICENSE THIRD_PARTY_NOTICES.md "$licenses/"
cp ui/dist/third-party-licenses.txt "$licenses/ui-packages.txt"
cargo about generate --locked about.hbs -o "$licenses/rust-crates.txt"
cat >"$licenses/GStreamer.txt" <<'NOTE'
GStreamer isn't included. Capture Room uses your Linux distribution's
GStreamer, installed from its packages (install.sh installs them on Ubuntu
and Debian). Those packages are under their own licenses, mostly
LGPL-2.1-or-later, with some GPL plugins such as x264 and x265; on Debian and
Ubuntu the texts are in /usr/share/doc/<package>/copyright.

The NDI, fallbackswitch and WebRTC plugins (gst-plugins-rs, MPL-2.0) are
built into the capture-room binary; their licenses are in rust-crates.txt.
NOTE

# For the build log: the libraries it needs from the system.
echo "capture-room links:"
ldd "$app/capture-room" | sed 's/^/  /'

mkdir -p dist
archive=dist/capture-room-linux-$arch.tar.gz
tar -C "$stage" -czf "$archive" capture-room
(cd dist && sha256sum "$(basename "$archive")" >"$(basename "$archive").sha256")
if [[ -n ${MINISIGN_KEY_FILE:-} ]]; then
    minisign -S -s "$MINISIGN_KEY_FILE" -m "$archive" -t "capture-room $version linux-$arch" >/dev/null
fi
# Only so dist/ can be served as CAPTURE_ROOM_RELEASES to test the updater
# locally; release.yml drops it and its publish job writes the release's.
printf '{"version": "%s"}\n' "$version" >dist/latest.json
echo "Built $archive ($(du -h "$archive" | cut -f1)), version $version"
