#!/usr/bin/env bash
# Smoke-test a packaged macOS or Linux archive the way a user's machine first
# meets it: unpack it into a clean folder, run --version, start the node on a
# temp database and port with a bare environment, and request
# /api/v1/auth/identity. A library, framework or GStreamer plugin missing from
# the package fails here rather than on a user's machine. (Linux archives
# don't bundle GStreamer: there the distro's, as install.sh installs it.)
#
# Usage: scripts/smoke-test.sh <archive>, e.g.
#        scripts/smoke-test.sh dist/capture-room-linux-x86_64.tar.gz
# Env:   SMOKE_PORT  the port to start the node on (default 47700)
set -euo pipefail

archive=${1:?usage: scripts/smoke-test.sh <archive>}
port=${SMOKE_PORT:-47700}
repo=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/node/Cargo.toml" | head -1)

work=$(mktemp -d)
pid=
cleanup() {
    if [[ -n $pid ]]; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT

tar -C "$work" -xzf "$archive"
case $(uname -s) in
Darwin) exe="$work/Capture Room.app/Contents/MacOS/capture-room" ;;
Linux) exe="$work/capture-room/capture-room" ;;
*) echo "no smoke test for $(uname -s)" >&2; exit 1 ;;
esac

# No GST_*, DYLD_* or PKG_CONFIG_* from the build: only what the package
# brings and the OS has.
bare=(env -i "HOME=$HOME" "PATH=/usr/bin:/bin" "TMPDIR=${TMPDIR:-/tmp}")

reported=$("${bare[@]}" "$exe" --version)
echo "$reported"
[[ $reported == "capture-room $version" ]] || { echo "expected capture-room $version" >&2; exit 1; }

log=$work/node.log
"${bare[@]}" "$exe" --port "$port" --db "$work/smoke.db" >"$log" 2>&1 &
pid=$!

# The first start scans every GStreamer plugin, which can take a minute.
url=http://127.0.0.1:$port/api/v1/auth/identity
deadline=$((SECONDS + 180))
until identity=$(curl -fsS --max-time 5 "$url" 2>/dev/null); do
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" && code=0 || code=$?
        pid=
        echo "::error::The node exited ($code) before it answered $url" >&2
        cat "$log" >&2
        exit 1
    fi
    if ((SECONDS > deadline)); then
        echo "::error::The node didn't answer $url within 180 s" >&2
        cat "$log" >&2
        exit 1
    fi
    sleep 1
done
echo "$identity"
[[ $identity == *"\"version\":\"$version\""* ]] || { echo "expected version $version" >&2; exit 1; }
echo "Smoke test passed: $(basename "$archive")"
