#!/bin/sh
# Tests how the installers (site/static/install.sh, install.ps1) pick the
# tvpipe release to install: the highest vX.Y.Z tag among the repository's
# releases, never a game's (NAME-vX.Y.Z) or a prerelease, from the GitHub
# API's JSON in scripts/vectors/installers/ (pretty and compact, as
# api.github.com sends either). No network.
#
# install.sh runs under every POSIX shell found (sh, dash, bash, busybox
# sh, ksh); install.ps1 under pwsh when it is installed.
#
# Usage: scripts/test-installers.sh
set -eu

cd "$(dirname "$0")/.."
vectors=scripts/vectors/installers
failed=0
ran=0

fail() { printf 'FAIL: %s\n' "$*" >&2; failed=1; }

# COMMAND's output (in SHELL, with install.sh's functions; a failure adds
# "(exit N)") must be EXPECTED.
check() { # SHELL DESCRIPTION EXPECTED COMMAND
  got=$(TVPIPE_INSTALL_SOURCED=1 $1 -c ". site/static/install.sh; $4" 2>&1) || {
    rc=$?
    got="${got:+$got }(exit $rc)"
  }
  if [ "$got" = "$3" ]; then
    ran=$((ran + 1))
  else
    fail "$1: $2: expected '$3', got '$got'"
  fi
}

for sh in sh dash bash ksh "busybox sh"; do
  command -v "${sh%% *}" >/dev/null 2>&1 || continue
  case "$sh" in busybox*) busybox sh -c true 2>/dev/null || continue ;; esac
  check "$sh" "pretty JSON" 0.10.1 "newest_version < $vectors/releases.json"
  check "$sh" "compact JSON" 0.10.1 "newest_version < $vectors/releases-compact.json"
  check "$sh" "only games" "" "newest_version < $vectors/games-only.json"
  check "$sh" "no JSON" "" "printf '' | newest_version"
  check "$sh" "an error page" "" "printf '{\"message\": \"API rate limit exceeded\"}\n' | newest_version"
  check "$sh" "tag v0.2.0" 0.2.0 "tag_version v0.2.0"
  check "$sh" "tag v10.0.1" 10.0.1 "tag_version v10.0.1"
  check "$sh" "a game's tag" "(exit 1)" "tag_version drawbluff-v0.2.0"
  check "$sh" "a prerelease tag" "(exit 1)" "tag_version v1.0.0-rc.1"
  check "$sh" "no tag" "(exit 1)" "tag_version ''"
  # latest_version with curl stubbed: the API's list, else the redirect
  # of /releases/latest (only when it names a vX.Y.Z tag).
  api_ok="curl() { cat $vectors/releases-compact.json; }"
  api_down() { # TAG the redirect names
    echo "curl() { case \"\$*\" in *api.github.com*) return 22 ;; *) printf 'https://github.com/x/y/releases/tag/$1' ;; esac; }"
  }
  check "$sh" "latest from the API" 0.10.1 "$api_ok; latest_version"
  check "$sh" "latest without the API" 0.9.12 "$(api_down v0.9.12); latest_version"
  check "$sh" "latest is a game's" "(exit 1)" "$(api_down drawbluff-v0.3.0); latest_version"
done

if command -v pwsh >/dev/null 2>&1; then
  for f in releases.json releases-compact.json games-only.json; do
    case "$f" in games-only.json) want="" ;; *) want=0.10.1 ;; esac
    got=$(TVPIPE_INSTALL_SOURCED=1 pwsh -NoProfile -NonInteractive -Command \
      ". ./site/static/install.ps1; Select-TvpipeVersion (Get-Content -Raw $vectors/$f | ConvertFrom-Json)" 2>&1) || {
      rc=$?
      got="${got:+$got }(exit $rc)"
    }
    got=$(printf '%s' "$got" | tr -d '\r') # Windows
    if [ "$got" = "$want" ]; then ran=$((ran + 1)); else fail "pwsh: $f: expected '$want', got '$got'"; fi
  done
else
  echo "pwsh not found: install.ps1 not tested"
fi

[ "$failed" = 0 ] || exit 1
echo "ok: $ran checks"
