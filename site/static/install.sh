#!/bin/sh
# Installs tvpipe from its GitHub Releases on macOS and Linux.
#
#   curl -fsSL https://tvpipe.party/install.sh | sh
#
# Environment:
#   TVPIPE_VERSION      version to install, e.g. 0.1.0 (default: the latest)
#   TVPIPE_INSTALL_DIR  directory for the binary (default: ~/.local/bin)
#
# Downloads tvpipe-VERSION-TARGET.tar.gz and SHA256SUMS from the release,
# checks the archive's SHA-256 and copies the tvpipe binary into the
# install directory. Nothing else is changed.

set -eu

REPO="mitsuhiko/tvpipe"

say() { printf '%s\n' "$*"; }
err() { printf 'tvpipe install: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

download() { # URL FILE
  if has curl; then
    curl -fsSL --proto '=https' --retry 3 -o "$2" "$1"
  elif has wget; then
    wget -q -O "$2" "$1"
  else
    err "needs curl or wget"
  fi
}

# The repository's releases (the GitHub API's JSON) on stdin; prints the
# highest version among the tags vX.Y.Z, nothing when there is none. Games
# have their own releases in the repository (NAME-vX.Y.Z), so the newest
# release or GitHub's "latest" need not be tvpipe's.
newest_version() {
  grep -o '"tag_name": *"[^"]*"' |
    sed 's/^"tag_name": *"\(.*\)"$/\1/' |
    grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' |
    sed 's/^v//' |
    sort -t . -k1,1n -k2,2n -k3,3n |
    tail -n 1
}

# A tag vX.Y.Z as X.Y.Z; fails for any other tag.
tag_version() { # TAG
  v=${1#v}
  case "$1" in
    v[0-9]*) ;;
    *) return 1 ;;
  esac
  case "$v" in
    *[!0-9.]* | *..* | .* | *.) return 1 ;;
  esac
  printf '%s\n' "$v"
}

latest_version() {
  api="https://api.github.com/repos/$REPO/releases"
  if has curl; then
    json=$(curl -fsSL --proto '=https' --retry 3 -H 'Accept: application/vnd.github+json' \
      "$api?per_page=100" 2>/dev/null) || json=
  else
    json=$(wget -q -O - "$api?per_page=100" 2>/dev/null) || json=
  fi
  version=$(printf '%s\n' "$json" | newest_version)
  if [ -n "$version" ]; then
    printf '%s\n' "$version"
    return 0
  fi
  # The API failed (it allows 60 requests an hour without a token): ask
  # for GitHub's latest release, which the release workflow marks.
  if has curl; then
    # The latest release's page redirects to .../releases/tag/vX.Y.Z.
    url=$(curl -fsSLI --proto '=https' --retry 3 -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest") || return 1
    tag=${url##*/}
  else
    tag=$(wget -q -O - "$api/latest" |
      sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1) || return 1
  fi
  tag_version "$tag"
}

sha256() { # FILE
  if has sha256sum; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif has shasum; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  elif has openssl; then
    openssl dgst -sha256 "$1" | sed 's/.*= *//'
  else
    err "needs sha256sum, shasum or openssl to check the download"
  fi
}

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) err "no prebuilt tvpipe for the $arch architecture (build it from source: https://github.com/$REPO)" ;;
  esac
  case "$os" in
    Darwin)
      # A shell under Rosetta reports x86_64 on Apple silicon.
      if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
        arch=aarch64
      fi
      printf '%s\n' "$arch-apple-darwin"
      ;;
    Linux) printf '%s\n' "$arch-unknown-linux-musl" ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT)
      err "on Windows, run in PowerShell: irm https://tvpipe.party/install.ps1 | iex" ;;
    *) err "no prebuilt tvpipe for $os (build it from source: https://github.com/$REPO)" ;;
  esac
}

main() {
  has tar || err "needs tar"
  has uname || err "needs uname"
  target=$(detect_target)

  version=${TVPIPE_VERSION:-}
  version=${version#v}
  if [ -z "$version" ]; then
    version=$(latest_version) || err "could not find the latest release of $REPO"
  fi

  if [ -n "${TVPIPE_INSTALL_DIR:-}" ]; then
    dir=$TVPIPE_INSTALL_DIR
  else
    [ -n "${HOME:-}" ] || err "HOME is not set; set TVPIPE_INSTALL_DIR"
    dir=$HOME/.local/bin
  fi

  name="tvpipe-$version-$target"
  base="https://github.com/$REPO/releases/download/v$version"
  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t tvpipe)
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 1' HUP INT TERM

  say "Installing tvpipe $version ($target) into $dir"
  download "$base/$name.tar.gz" "$tmp/$name.tar.gz" ||
    err "could not download $base/$name.tar.gz"
  download "$base/SHA256SUMS" "$tmp/SHA256SUMS" ||
    err "could not download $base/SHA256SUMS"

  expected=$(awk -v f="$name.tar.gz" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
  [ -n "$expected" ] || err "SHA256SUMS has no entry for $name.tar.gz"
  actual=$(sha256 "$tmp/$name.tar.gz")
  [ "$expected" = "$actual" ] ||
    err "checksum mismatch for $name.tar.gz (expected $expected, got $actual)"

  tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
  [ -f "$tmp/$name/tvpipe" ] || err "the archive has no $name/tvpipe"

  mkdir -p "$dir" || err "could not create $dir"
  # Copy next to the target and rename, so a running tvpipe is not hit.
  cp "$tmp/$name/tvpipe" "$dir/.tvpipe.new.$$" || err "could not write to $dir"
  chmod 755 "$dir/.tvpipe.new.$$"
  mv -f "$dir/.tvpipe.new.$$" "$dir/tvpipe" || err "could not write $dir/tvpipe"

  say "Installed $dir/tvpipe"
  "$dir/tvpipe" --version 2>/dev/null || true
  say ""

  case ":${PATH:-}:" in
    *":$dir:"*) cmd=tvpipe ;;
    *)
      cmd="$dir/tvpipe"
      say "$dir is not in your PATH. Add it, e.g. in ~/.profile or your shell's rc file:"
      say ""
      say "    export PATH=\"$dir:\$PATH\""
      say ""
      ;;
  esac

  say "Next steps:"
  say "  Find AirPlay and Cast receivers:  $cmd discover"
  say "  Check picture and latency:        $cmd test-pattern --to \"Living Room\""
  say ""
  if has ffmpeg; then
    say "ffmpeg found: $(command -v ffmpeg)"
  else
    say "Optional: install ffmpeg 6 or newer to mirror files, URLs and capture devices that"
    say "tvpipe cannot send as they are (macOS: brew install ffmpeg; Linux: your package manager)."
  fi
  say "Documentation: https://tvpipe.party/"
}

# TVPIPE_INSTALL_SOURCED=1: only define the functions (scripts/test-installers.sh).
if [ "${TVPIPE_INSTALL_SOURCED:-}" != 1 ]; then
  main "$@"
fi
