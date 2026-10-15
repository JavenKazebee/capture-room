#!/usr/bin/env bash
# Replay buffer picture quality (ROADMAP #12): each replay codec, with the
# encoder settings of `ReplayLeg::build` (node/src/pipeline/replay.rs), scored
# against the footage it was given, as a channel shows it: the replay player's
# software decoder, then the program's I420 BT.709.
#
#   node/examples/replay_quality.sh FOOTAGE [DIR] [START_S…]
#
# FOOTAGE is any video file; 10 s from each START (default 170 735 1040) is
# scaled to 1080p as the reference. Prints data rate and VMAF (mean, worst 1%,
# worst frame), PSNR-Y and SSIM per clip. Keep `variant` in step with
# replay.rs. Needs gst-launch-1.0, and ffmpeg built with libvmaf.
set -euo pipefail

footage=$1
dir=${2:-${TMPDIR:-/tmp}/replay-quality}
shift $(($# < 2 ? $# : 2))
if [ $# -gt 0 ]; then starts=("$@"); else starts=(170 735 1040); fi
mkdir -p "$dir"
cd "$dir"

VT="vtenc_h264 realtime=true allow-frame-reordering=false max-keyframe-interval=1"
CODECS="prores h264-vt h264-x264 mjpeg"
# encoder|decoder for a codec
variant() {
  case $1 in
    prores) echo "vtenc_prores ! video/x-prores,variant=proxy|avdec_prores" ;;
    h264-vt) echo "$VT quality=0.75 ! h264parse|h264parse ! avdec_h264" ;;
    h264-x264) echo "x264enc key-int-max=1 speed-preset=superfast pass=qual quantizer=18 bitrate=2048000 ! h264parse|h264parse ! avdec_h264" ;;
    mjpeg) echo "video/x-raw,format=I420,colorimetry=1:4:0:0 ! jpegenc quality=85|jpegdec" ;;
  esac
}

for t in "${starts[@]}"; do
  # The reference is what the encoders get: read back through the same demux,
  # so frames line up one for one.
  [ -f "ref_$t.y4m" ] || {
    ffmpeg -v error -ss "$t" -i "$footage" -t 10 -an \
      -vf "scale=1920:1080:flags=lanczos,format=yuv420p" -c:v ffv1 -y "ref_$t.mkv"
    gst-launch-1.0 -q filesrc location="ref_$t.mkv" ! matroskademux ! avdec_ffv1 \
      ! y4menc ! filesink location="ref_$t.y4m"
  }
done

printf '%-6s %-10s %6s %10s %6s %6s %6s %7s %7s\n' \
  clip codec Mb/s 'GB/h@60p' VMAF 1% min PSNR-Y SSIM
for t in "${starts[@]}"; do
  for v in $CODECS; do
    IFS='|' read -r enc dec <<<"$(variant "$v")"
    gst-launch-1.0 -q filesrc location="ref_$t.mkv" ! matroskademux ! avdec_ffv1 \
      ! videoconvert ! $enc ! qtmux ! filesink location="$t-$v.mov" 2>/dev/null || {
      printf '%-6s %-10s unavailable\n' "$t" "$v"
      continue
    }
    gst-launch-1.0 -q filesrc location="$t-$v.mov" ! qtdemux ! $dec ! videoconvert \
      ! video/x-raw,format=I420,colorimetry=bt709 ! y4menc ! filesink location="$t-$v.y4m"
    ffmpeg -v error -i "$t-$v.y4m" -i "ref_$t.y4m" -lavfi \
      "[0:v]setpts=N/30/TB[d];[1:v]setpts=N/30/TB[r];[d][r]libvmaf=log_fmt=json:log_path=$t-$v.json:n_threads=4:feature=name=psnr|name=float_ssim" \
      -f null - 2>/dev/null
    rm -f "$t-$v.y4m"
    python3 - "$t" "$v" <<'EOF'
import json, os, sys
t, v = sys.argv[1:]
d = json.load(open(f"{t}-{v}.json"))
frames = d["frames"]
vmaf = sorted(f["metrics"]["vmaf"] for f in frames)
# 10 s of footage, whatever its rate: per-frame size, at 59.94 fps.
mbps = os.path.getsize(f"{t}-{v}.mov") * 8 / 10 / 1e6
gbh = os.path.getsize(f"{t}-{v}.mov") / len(frames) * 59.94 * 3600 / 1e9
p = d["pooled_metrics"]
print(f"{t:6} {v:10} {mbps:6.1f} {gbh:10.0f} {p['vmaf']['mean']:6.2f} "
      f"{vmaf[len(vmaf) // 100]:6.2f} {vmaf[0]:6.2f} {p['psnr_y']['mean']:7.2f} "
      f"{p['float_ssim']['mean']:7.4f}")
EOF
  done
done
