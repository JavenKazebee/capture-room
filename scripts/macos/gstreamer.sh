#!/usr/bin/env bash
# Fetch the official GStreamer macOS packages and unpack them, without
# installing anything, into two frameworks:
#   <out>/sdk/GStreamer.framework      runtime + devel: what the node builds against
#   <out>/runtime/GStreamer.framework  what the app bundle ships
# Usage: gstreamer.sh <version> <out>. Does nothing if <out> is already built.
set -euo pipefail

version=${1:?version, e.g. 1.28.8}
out=${2:?output folder}
[[ -f "$out/.done" ]] && exit 0

# Packages the node never uses. Their plugins come with them.
skip='python|gtk|editing|visualizers|dvd-gpl|devtools'
# Plugins the node links in itself (gst-plugins-rs, statically): the
# framework's copies would clash with them.
linked_in='libgstndi|libgstfallbackswitch|libgstrswebrtc'

rm -rf "$out"
mkdir -p "$out/pkg"
base=https://gstreamer.freedesktop.org/data/pkg/osx/$version
for kind in "" devel-; do
    pkg=gstreamer-1.0-$kind$version-universal.pkg
    echo "Downloading $pkg"
    curl -fL --retry 3 -o "$out/pkg/$pkg" "$base/$pkg"
    pkgutil --expand-full "$out/pkg/$pkg" "$out/pkg/${kind:-runtime-}x"
done

# One framework from a set of component packages: the framework skeleton
# installs at the framework's root, the rest at Versions/1.0.
assemble() {
    local framework=$1
    shift
    mkdir -p "$framework"
    for component in "$@"; do
        if [[ $(basename "$component") == osx-framework-* ]]; then
            ditto "$component/Payload" "$framework"
        else
            ditto "$component/Payload" "$framework/Versions/1.0"
        fi
    done
}

assemble "$out/sdk/GStreamer.framework" "$out"/pkg/runtime-x/*.pkg "$out"/pkg/devel-x/*.pkg

runtime_parts=()
for component in "$out"/pkg/runtime-x/*.pkg; do
    [[ $(basename "$component") =~ $skip ]] || runtime_parts+=("$component")
done
assemble "$out/runtime/GStreamer.framework" "${runtime_parts[@]}"
lib=$out/runtime/GStreamer.framework/Versions/1.0
root=$out/runtime/GStreamer.framework
rm -rf "$lib/include" "$lib/Headers" "$lib/lib/pkgconfig" "$lib/bin" "$lib/Commands" \
    "$root/Headers" "$root/Commands"
# A framework's root may hold only its standard links, or it can't be signed.
rm -f "$root/.gitignore" "$root/Libraries"
find "$lib" \( -name '*.a' -o -name '*.la' \) -delete
find "$lib/lib/gstreamer-1.0" -maxdepth 1 -name '*.dylib' | grep -E "/($linked_in)\.dylib$" | xargs rm -f

rm -rf "$out/pkg"
touch "$out/.done"
echo "GStreamer $version: $(du -sh "$out/runtime" | cut -f1) runtime in $out"
