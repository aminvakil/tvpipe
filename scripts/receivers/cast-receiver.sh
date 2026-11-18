#!/usr/bin/env bash
# Builds and runs openscreen's standalone Cast Streaming receiver
# (`cast_receiver`, BSD-3) as a local test receiver for tvpipe.
#
#   scripts/receivers/cast-receiver.sh build        # fetch + build (headless)
#   scripts/receivers/cast-receiver.sh build-av     # build with ffmpeg + SDL2
#                                                   # (decodes and shows video)
#   scripts/receivers/cast-receiver.sh run [IFACE]  # run on IFACE (default lo0
#                                                   # on macOS, lo on Linux)
#
# Then, in another terminal:
#
#   TVPIPE_CAST_ADDR=127.0.0.1:8010 TVPIPE_CAST_SECS=10 \
#       cargo test cast::receiver_loop -- --ignored --nocapture
#
# The headless build uses openscreen's dummy player, which logs every frame
# it plays out ("[SSRC n] KEY F0 at ... bytes"). The AV build needs
# Homebrew (macOS) or distro packages for ffmpeg, sdl2 and opus.
#
# Environment:
#   OPENSCREEN_ROOT  checkout parent (default ~/.cache/checkouts/chromium.googlesource.com)
#   CAST_RECEIVER_PORT  TLS port (default 8010)
#
# The first build downloads depot_tools, openscreen's DEPS (clang, gn,
# ninja, rust toolchain; about 3 GB) and takes several minutes.
set -euo pipefail

ROOT="${OPENSCREEN_ROOT:-$HOME/.cache/checkouts/chromium.googlesource.com}"
OS_DIR="$ROOT/openscreen"
DEPOT="$ROOT/depot_tools"
STATE="${XDG_STATE_HOME:-$HOME/.cache}/tvpipe/cast-receiver"
PORT="${CAST_RECEIVER_PORT:-8010}"

fetch() {
  mkdir -p "$ROOT"
  [ -d "$DEPOT" ] || git clone --depth 1 https://chromium.googlesource.com/chromium/tools/depot_tools.git "$DEPOT"
  [ -d "$OS_DIR" ] || git clone --depth 1 https://chromium.googlesource.com/openscreen "$OS_DIR"
  if [ ! -f "$ROOT/.gclient" ]; then
    cat > "$ROOT/.gclient" <<'EOF'
solutions = [
  {
    "name": "openscreen",
    "url": "https://chromium.googlesource.com/openscreen",
    "deps_file": "DEPS",
    "managed": False,
    "custom_deps": {},
  },
]
EOF
  fi
  (cd "$ROOT" && PATH="$DEPOT:$PATH" DEPOT_TOOLS_UPDATE=0 gclient sync --no-history -j8)
}

gn_gen() {
  local out="$1" args="$2"
  (cd "$OS_DIR" && buildtools/*/gn gen "$out" --args="$args")
}

build() {
  fetch
  gn_gen out/recv 'is_debug=false'
  (cd "$OS_DIR" && third_party/ninja/ninja -C out/recv cast_receiver)
}

build_av() {
  fetch
  local args='is_debug=false have_ffmpeg=true have_libsdl2=true have_libopus=true'
  if command -v brew >/dev/null; then
    local prefix cellar
    prefix="$(brew --prefix)"
    cellar="$(brew --cellar)"
    inc() { ls -d "$cellar/$1"/*/include | tail -1; }
    args="$args external_lib_dirs=[\"$prefix/lib\"]"
    args="$args ffmpeg_include_dirs=[\"$(inc ffmpeg)\"]"
    args="$args libsdl2_include_dirs=[\"$(inc sdl2)\"]"
    args="$args libopus_include_dirs=[\"$(inc opus)\"]"
    args="$args mac_deployment_target=\"$(sw_vers -productVersion | cut -d. -f1).0\""
  fi
  gn_gen out/recv-av "$args"
  (cd "$OS_DIR" && third_party/ninja/ninja -C out/recv-av cast_receiver)
}

run() {
  local iface="${1:-}"
  if [ -z "$iface" ]; then
    if [ "$(uname)" = Darwin ]; then iface=lo0; else iface=lo; fi
  fi
  local bin="$OS_DIR/out/recv-av/cast_receiver"
  [ -x "$bin" ] || bin="$OS_DIR/out/recv/cast_receiver"
  [ -x "$bin" ] || { echo "cast_receiver not built; run: $0 build" >&2; exit 1; }
  mkdir -p "$STATE"
  cd "$STATE"
  if [ ! -f generated_root_cast_receiver.key ]; then
    "$bin" -g "$iface"
  fi
  echo "cast_receiver on $iface port $PORT (state in $STATE)" >&2
  exec "$bin" -x -v -r "$PORT" \
    -p generated_root_cast_receiver.key \
    -d generated_root_cast_receiver.crt "$iface"
}

case "${1:-}" in
  build) build ;;
  build-av) build_av ;;
  run) shift; run "$@" ;;
  *) sed -n '2,24p' "$0"; exit 2 ;;
esac
