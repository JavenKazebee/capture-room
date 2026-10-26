#!/usr/bin/env bash
# Install the GStreamer packages the node builds and tests against on an
# Ubuntu runner: the -dev packages (plugins-bad has WebRTC, which
# gst-plugin-webrtc links), libssl for reqwest's native TLS, and the plugins
# the tests' pipelines use.
# Usage: gstreamer.sh [archives]. With <archives>, apt keeps the .debs there
# for actions/cache to save and restore.
set -euo pipefail

archives=${1:-}
packages=(
  pkg-config libssl-dev
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
  libgstreamer-plugins-bad1.0-dev libgstrtspserver-1.0-dev
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad
  gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-nice
)
# plugins-bad's -dev package drags in OpenCV, VTK and Qt, about 240 MB. The
# runner's Azure mirror usually serves that in seconds but sometimes crawls
# (one run spent 48 minutes), so a slow download is cut off and resumed from
# another mirror. Any mirror will do: apt checks the packages' signatures.
mirrors=(azure.archive.ubuntu.com archive.ubuntu.com mirrors.edge.kernel.org)
attempt_timeout=4m

# Rebuilding the man-db index can take minutes and nothing here reads it.
sudo rm -f /var/lib/man-db/auto-update

export DEBIAN_FRONTEND=noninteractive
apt=(sudo -E apt-get -y --no-install-recommends -o Acquire::Retries=3)
if [[ -n "$archives" ]]; then
  mkdir -p "$archives/partial"
  # The runner owns the folder, so apt's _apt user can't write to it.
  apt+=(-o Dir::Cache::Archives="$archives" -o APT::Sandbox::User=root)
fi

current=${mirrors[0]}
downloaded=
for mirror in "${mirrors[@]}"; do
  if [[ "$mirror" != "$current" ]]; then
    echo "::warning::Package download too slow from $current; trying $mirror"
    for list in /etc/apt/sources.list /etc/apt/sources.list.d/*; do
      if [[ -f "$list" ]]; then
        sudo sed -i "s#://$current/#://$mirror/#" "$list"
      fi
    done
    current=$mirror
  fi
  # Whatever an attempt fetched stays in the archives; the next resumes.
  if timeout "$attempt_timeout" "${apt[@]}" update &&
    timeout "$attempt_timeout" "${apt[@]}" install --download-only "${packages[@]}"; then
    downloaded=1
    break
  fi
done
if [[ -z "$downloaded" ]]; then
  echo "::error::Couldn't download the GStreamer packages from any mirror"
  exit 1
fi
"${apt[@]}" install "${packages[@]}"

if [[ -n "$archives" ]]; then
  # apt leaves a root-only lock file, which actions/cache's tar can't read:
  # without this the cache is never saved.
  sudo rm -rf "$archives/lock" "$archives/partial"
  sudo chown -R "$(id -u):$(id -g)" "$archives"
fi
