#!/usr/bin/env bash
# Checks `tvpipe debug-pipeline --raw` against `ffprobe -show_packets`.
#
# For several ffmpeg-generated files, the frames our demuxer reports
# (stream, pts in ms, keyframe flag, size) must equal ffprobe's packets of
# the same file. --raw stream-copies the input through ffmpeg's live
# Matroska muxer, so this checks the muxer settings and our demuxer.
# Then it runs the encoding pipeline on the same files and checks frame
# counts, keyframe spacing and monotonic timestamps, and SLRW game input
# end to end with crates/tvpipe/examples/slrw-gen.
#
# Usage: scripts/check-pipeline.sh [path/to/tvpipe]
# Requires ffmpeg, ffprobe and python3.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
if [[ $# -ge 1 ]]; then
  bin="$1"
else
  (cd "$root" && cargo build --quiet --all-targets)
  bin="$root/target/debug/tvpipe"
fi
gen="$root/target/debug/examples/slrw-gen"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
src=(-f lavfi -i "testsrc2=size=320x240:rate=30" -f lavfi -i "sine=frequency=440:sample_rate=48000" -t 3)
x264=(-c:v libx264 -preset veryfast -bf 0 -g 30 -pix_fmt yuv420p)

ff "${src[@]}" -map 0:v -map 1:a -map 1:a "${x264[@]}" \
  -c:a:0 alac -ar:a:0 44100 -c:a:1 libopus -frame_duration:a:1 10 "$tmp/alac-opus.mkv"
ff "${src[@]}" -map 0:v -map 1:a "${x264[@]}" -c:a aac -b:a 128k "$tmp/aac.mp4"
ff "${src[@]}" -map 0:v -map 1:a -c:v libx264 -preset veryfast -g 50 -pix_fmt yuv420p \
  -c:a libopus "$tmp/bframes.mkv"
ff -f lavfi -i "testsrc2=size=1920x1080:rate=60" -t 1 -c:v libx264 -preset ultrafast \
  -bf 0 -pix_fmt yuv420p "$tmp/1080p60.mkv"

compare() {
  local file="$1"
  "$bin" debug-pipeline --raw "$file" >"$tmp/ours.txt"
  ffprobe -v error -show_entries packet=stream_index,pts_time,flags,size -of csv=p=0 \
    "$file" >"$tmp/probe.txt"
  python3 - "$tmp/ours.txt" "$tmp/probe.txt" "$file" <<'PY'
import sys
ours, probe = [], []
for line in open(sys.argv[1]):
    if not line.startswith("frame "):
        continue
    f = dict(kv.split("=", 1) for kv in line.split()[1:])
    ours.append((int(f["track"]) - 1, int(f["mkv_pts"]), f["key"] == "1", int(f["size"])))
for line in open(sys.argv[2]):
    p = line.strip().split(",")
    idx, pts, size, flags = int(p[0]), p[1], int(p[2]), p[3]
    ms = round(float(pts) * 1000)
    probe.append((idx, ms, flags.startswith("K"), size))
# ffmpeg's muxer shifts the whole timeline so nothing is negative (e.g.
# AAC priming in MP4): compare relative to the earliest packet.
shift = min(o[1] for o in ours) - min(p[1] for p in probe)
ours = sorted((s, t - shift, k, n) for s, t, k, n in ours)
probe.sort()
name = sys.argv[3].rsplit("/", 1)[-1]
if len(ours) != len(probe):
    sys.exit(f"{name}: {len(ours)} frames, ffprobe has {len(probe)} packets")
bad = [(o, p) for o, p in zip(ours, probe) if o != p]
# MP4 timestamps are rescaled to Matroska's 1 ms ticks: allow 1 ms.
bad = [(o, p) for o, p in bad if (o[0], o[2], o[3]) != (p[0], p[2], p[3]) or abs(o[1] - p[1]) > 1]
if bad:
    for o, p in bad[:10]:
        print(f"  ours {o} ffprobe {p}")
    sys.exit(f"{name}: {len(bad)} mismatches")
print(f"ok   {name}: {len(ours)} packets match ffprobe")
PY
}

encode_check() {
  local file="$1" expect_frames="$2"
  shift 2
  "$bin" debug-pipeline "$@" "$file" >"$tmp/enc.txt"
  python3 - "$tmp/enc.txt" "$expect_frames" "$file" <<'PY'
import sys
frames = {}
for line in open(sys.argv[1]):
    if line.startswith("frame "):
        f = dict(kv.split("=", 1) for kv in line.split()[1:])
        frames.setdefault(f["kind"], []).append((int(f["pts"]), f["key"] == "1"))
name = sys.argv[3].rsplit("/", 1)[-1]
video = frames.get("video", [])
lo, hi = [int(x) for x in sys.argv[2].split(":")]
if not lo <= len(video) <= hi:
    sys.exit(f"{name}: {len(video)} video frames, expected {lo}..{hi}")
keys = [i for i, (_, k) in enumerate(video) if k]
if keys[:1] != [0] or any(b - a != 30 for a, b in zip(keys, keys[1:])):
    sys.exit(f"{name}: keyframes at {keys}, expected every 30 frames from 0")
for kind, fs in frames.items():
    pts = [p for p, _ in fs]
    if any(b <= a for a, b in zip(pts, pts[1:])):
        sys.exit(f"{name}: {kind} pts not increasing")
step = {"audio-airplay": 352, "audio-cast": 480}
for kind, n in step.items():
    pts = [p for p, _ in frames.get(kind, [])]
    if any(b - a != n for a, b in zip(pts, pts[1:])):
        sys.exit(f"{name}: {kind} pts do not advance by {n}")
counts = ", ".join(f"{k}={len(v)}" for k, v in sorted(frames.items()))
print(f"ok   {name} encoded: {counts}, keyframes every 30")
PY
}

for f in alac-opus.mkv aac.mp4 bframes.mkv 1080p60.mkv; do
  compare "$tmp/$f"
done
encode_check "$tmp/alac-opus.mkv" 89:91
encode_check "$tmp/aac.mp4" 89:91
encode_check "$tmp/1080p60.mkv" 29:31

# SLRW game input end to end.
"$gen" --size 320x180 --fps 30 --seconds 3 |
  "$bin" debug-pipeline --raw-video 320x180@30:rgba --raw-audio 48000:2:s16le - >"$tmp/slrw.txt"
python3 - "$tmp/slrw.txt" <<'PY'
import sys
kinds = {}
for line in open(sys.argv[1]):
    if line.startswith("frame "):
        f = dict(kv.split("=", 1) for kv in line.split()[1:])
        kinds[f["kind"]] = kinds.get(f["kind"], 0) + 1
v, cast = kinds.get("video", 0), kinds.get("audio-cast", 0)
# 3 s at 30 fps; the fps filter may drop or repeat a frame at the edges.
if not 85 <= v <= 92 or not 295 <= cast <= 301:
    sys.exit(f"slrw: video={v} opus={cast}, expected ~90 and ~300")
print(f"ok   slrw-gen: video={v} audio-cast={cast} audio-airplay={kinds.get('audio-airplay', 0)}")
PY
echo "all pipeline checks passed"
