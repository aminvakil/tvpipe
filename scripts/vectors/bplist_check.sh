#!/bin/sh
# Checks that plutil (macOS) and Python plistlib accept our bplist writer's
# output and read back the same values as from the original fixtures.
set -eu
cd "$(dirname "$0")/../.."
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
TVPIPE_BPLIST_OUT="$out" cargo test -q codec::bplist::tests::write_reencodings_for_external_check >/dev/null
plutil -lint "$out"/*.bplist
for f in "$out"/*.bplist; do
  orig=tests/fixtures/codec/bplist/$(basename "$f")
  a=$(plutil -convert xml1 -o - "$f")
  b=$(plutil -convert xml1 -o - "$orig")
  [ "$a" = "$b" ] || { echo "plutil value mismatch: $(basename "$f")"; exit 1; }
done
python3 - "$out" <<'PY'
import os, plistlib, sys
d = sys.argv[1]
for f in sorted(os.listdir(d)):
    with open(os.path.join(d, f), "rb") as a, open(os.path.join("tests/fixtures/codec/bplist", f), "rb") as b:
        assert plistlib.load(a) == plistlib.load(b), f
print("plistlib: all equal")
PY
