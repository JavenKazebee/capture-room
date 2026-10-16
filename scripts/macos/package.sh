#!/usr/bin/env bash
# Build "Capture Room.app" for one architecture, with the official GStreamer
# framework inside, and pack it as dist/capture-room-macos-<arch>.tar.gz
# (+ .sha256).
#
# Usage: scripts/macos/package.sh <aarch64|x86_64>
# Env:   GST_VERSION       GStreamer release to bundle (default below)
#        GST_DIR           where gstreamer.sh unpacks it (default target/gstreamer-<version>)
#        CODESIGN_IDENTITY signing identity (default "-": ad hoc)
#        SKIP_UI=1         use the existing ui/dist
set -euo pipefail

arch=${1:?architecture: aarch64 or x86_64}
case $arch in
    aarch64) lipo_arch=arm64 ;;
    x86_64) lipo_arch=x86_64 ;;
    *) echo "unknown architecture $arch" >&2; exit 1 ;;
esac

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
gst_version=${GST_VERSION:-1.28.8}
gst_dir=${GST_DIR:-$repo/target/gstreamer-$gst_version}
identity=${CODESIGN_IDENTITY:--}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' node/Cargo.toml | head -1)
target=$arch-apple-darwin

scripts/macos/gstreamer.sh "$gst_version" "$gst_dir"

if [[ -z ${SKIP_UI:-} ]]; then
    pnpm install --frozen-lockfile
    pnpm build:ui
fi

# Link against the SDK's GStreamer, found at run time inside the bundle.
sdk_lib=$gst_dir/sdk/GStreamer.framework/Versions/1.0/lib
PKG_CONFIG_PATH=$sdk_lib/pkgconfig \
    PKG_CONFIG_ALLOW_CROSS=1 \
    MACOSX_DEPLOYMENT_TARGET=11.0 \
    RUSTFLAGS="-C link-arg=-Wl,-rpath,@executable_path/../Frameworks/GStreamer.framework/Versions/1.0/lib" \
    cargo build --release --locked -p capture-room --target "$target"

stage=target/package/$arch
app="$stage/Capture Room.app"
rm -rf "$stage"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks"
cp "target/$target/release/capture-room" "$app/Contents/MacOS/"
sed "s/@VERSION@/$version/g" scripts/macos/Info.plist >"$app/Contents/Info.plist"
ditto --arch "$lipo_arch" "$gst_dir/runtime/GStreamer.framework" "$app/Contents/Frameworks/GStreamer.framework"

# The binary must not load any library from outside the bundle or the OS.
if otool -L "$app/Contents/MacOS/capture-room" | tail -n +2 | grep -vE '^\s+(@rpath|/usr/lib|/System)/'; then
    echo "capture-room links libraries outside the bundle (above)" >&2
    exit 1
fi

# Thinning changed every library, so sign each one again, innermost first.
# Ad hoc signing until there's a Developer ID; no hardened runtime until
# notarization needs it (it would refuse ad hoc signed plugins). The
# framework's own binary is left to the framework's signature: signing it
# signs the whole framework, which races the others.
echo "Signing with identity '$identity'"
find "$app/Contents/Frameworks" -type f -print0 |
    xargs -0 file | grep 'Mach-O' | cut -d: -f1 | sort -u | grep -v '/Versions/1.0/GStreamer$' |
    tr '\n' '\0' | xargs -0 -P 8 -n 32 codesign --force --sign "$identity" --timestamp=none
codesign --force --sign "$identity" --timestamp=none "$app/Contents/Frameworks/GStreamer.framework"
codesign --force --sign "$identity" --timestamp=none "$app"
codesign --verify --deep --strict "$app"

mkdir -p dist
archive=dist/capture-room-macos-$arch.tar.gz
tar -C "$stage" -czf "$archive" "Capture Room.app"
(cd dist && shasum -a 256 "$(basename "$archive")" >"$(basename "$archive").sha256")
echo "Built $archive ($(du -h "$archive" | cut -f1)), version $version"
