# Cross-platform status

What has been checked on which OS, and how to repeat it from a Mac.
Last updated 2026-11-18 (branch `cross-platform`).

## Matrix

| Target | fmt | clippy -D warnings | tests | binary run |
|---|---|---|---|---|
| macOS aarch64 | yes | yes | yes (ffmpeg 8.1) | yes (discover, check-pipeline) |
| Linux aarch64 (Docker, Debian 13) | yes | yes (clippy 1.100) | yes: ffmpeg 7.1, ffmpeg 8.1, no ffmpeg | `discover` against avahi on a Docker bridge; `scripts/check-pipeline.sh` |
| Linux aarch64 (Docker, Ubuntu 24.04) | — | yes | yes (ffmpeg 6.1, the CI runner's) | — |
| Linux x86_64 (Docker, emulated) | yes | yes | yes, except `rawin::ffmpeg_end_to_end` under emulation (emulated ffmpeg starts slowly: 24 of ≥25 frames) | — |
| Windows x86_64 msvc | — | yes (cross, see below) | not run | not run |
| Windows aarch64 msvc | — | yes (cross) | not run | not run |
| Windows x86_64 gnu | — | yes (cross) | built, not run | `tvpipe.exe` links (UCRT, system DLLs only), not run |
| MSRV 1.85 | — | `cargo check --all-targets` | — | — |

No wine on the build machine, so nothing has run on Windows yet.

## Repeating it

Windows (from macOS or Linux, needs zig and the rustup targets):

```sh
scripts/cross-check.sh            # clippy for both msvc targets, gnu .exe
```

Linux in Docker (a `rust` image plus `apt-get install ffmpeg`):

```sh
docker run --rm -v "$PWD":/src:ro -e CARGO_TARGET_DIR=/target -w /work IMAGE \
  sh -c 'cp -r /src/. /work && cargo fmt --check &&
         cargo clippy --all-targets -- -D warnings && cargo test'
```

Use `--platform linux/amd64` for x86_64 (emulated, slow). The Docker
Desktop VM's network is NATed, so `tvpipe discover` with `--network host`
finds nothing on the LAN. To check mDNS on Linux, run `avahi-daemon` with a
static `_googlecast._tcp` service in one container and `tvpipe discover`
in another on the same user-defined bridge network.

## Platform differences handled in the code

- Ctrl-C: `src/util/interrupt.rs` (decision 0004), the only FFI.
- Config directory: `src/airplay/credentials.rs` (`%APPDATA%\tvpipe`).
- `ffmpeg.exe`/`ffprobe.exe`: `std::env::consts::EXE_SUFFIX` in
  `media::ffmpeg::find_in_path`.
- Windows UDP reads fail with `ConnectionReset` after an ICMP port
  unreachable; every UDP receive loop treats that as transient.
- `ConnectionReset`/`ConnectionAborted` (Windows closes) map to
  `ErrorKind::Closed` like EOF.
- Line ends: `.gitattributes` checks out LF everywhere; credentials files
  and vector files parse with CRLF too.
- Binary stdio: Rust does no newline translation; on Windows a console
  stdout rejects non-UTF-8 writes, but the SLRW back channel is only text
  (`keyframe\n`) and SLRW input is always a pipe.

## Unverified on Windows

- Multicast egress: mDNS queries go out from a socket bound to the
  default-route address (`net::iface`). Linux and macOS pick the
  interface from the bound source address; Windows may use its default
  multicast interface instead (only matters with several interfaces or
  extra `--interface` addresses). std has no `IP_MULTICAST_IF`.
- Firewall: unicast mDNS replies to a multicast query are allowed by the
  default Windows Firewall rules (3 s window); inbound AirPlay timing
  requests may need the user to allow tvpipe once.
- The console Ctrl-C handler and `q` + Enter on a real console.
- The whole test suite.
