#!/usr/bin/env bash
# Checks the Windows targets from macOS or Linux, without a Windows SDK:
#
# - cargo clippy --all-targets -D warnings for x86_64-pc-windows-msvc and
#   aarch64-pc-windows-msvc (what CI's windows-latest builds),
# - cargo build --all-targets for x86_64-pc-windows-gnu, which links real
#   .exe files (tvpipe.exe and the test binary) for a Windows machine.
#
# ring compiles C, so this needs zig (https://ziglang.org): `zig cc` with
# its mingw headers is the C compiler, `zig ar`/`zig lib` the archivers and
# `zig cc` the gnu linker. For the msvc targets the C objects are only
# archived for type checking, never linked, so the gnu ABI of the objects
# does not matter there.
#
# Usage: scripts/cross-check.sh [path/to/zig]
# Needs: rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc \
#        x86_64-pc-windows-gnu
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
zig="${1:-$(command -v zig || echo "$HOME/.local/bin/zig")}"
[[ -x "$zig" ]] || { echo "zig not found (pass its path)" >&2; exit 1; }
wrap="$root/target/xcc"
mkdir -p "$wrap"

# zig cc for <arch>-windows-gnu; drops the flags rustc passes for a mingw
# gcc that zig's clang/lld does not take, and swaps libgcc_eh for zig's
# libunwind.
for arch in x86_64 aarch64; do
  cat >"$wrap/cc-$arch" <<EOF
#!/bin/sh
n=\$#; i=0
while [ \$i -lt \$n ]; do
  a="\$1"; shift; i=\$((i+1))
  case "\$a" in
    -lgcc_eh) set -- "\$@" -lunwind ;;
    --target=*|-lgcc|-lgcc_s|-l:libpthread.a|-lmsvcrt|-lmingwex|-lmingw32) ;;
    -nodefaultlibs|-fno-use-linker-plugin|-Wl,--disable-auto-image-base) ;;
    -Wl,--dynamicbase|-Wl,--nxcompat|-Wl,--high-entropy-va|-Wl,--large-address-aware) ;;
    *) set -- "\$@" "\$a" ;;
  esac
done
exec "$zig" cc -target $arch-windows-gnu "\$@"
EOF
  chmod +x "$wrap/cc-$arch"
done

cd "$root"
for arch in x86_64 aarch64; do
  t="${arch}_pc_windows_msvc"
  echo "== clippy $arch-pc-windows-msvc"
  env "CC_$t=$wrap/cc-$arch" "AR_$t=$zig lib" \
    cargo clippy --quiet --target "$arch-pc-windows-msvc" --all-targets -- -D warnings
done

echo "== build x86_64-pc-windows-gnu"
env "CC_x86_64_pc_windows_gnu=$wrap/cc-x86_64" "AR_x86_64_pc_windows_gnu=$zig ar" \
  "CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=$wrap/cc-x86_64" \
  cargo build --quiet --target x86_64-pc-windows-gnu --all-targets
ls target/x86_64-pc-windows-gnu/debug/tvpipe.exe
echo "windows cross-checks passed"
