# 0004: Ctrl-C handling (platform FFI exception)

Status: accepted by the human (2026-11-18). A Ctrl-C/SIGTERM stop exits with status 130 after teardown; `q` + Enter exits with 0.
code only; `cfg(target_os)` only for config paths"). **The human has the
final say**; dropping this means Ctrl-C ends tvpipe without STOP or
TEARDOWN.

## Problem

PLAN §3 requires Ctrl-C to send a proper STOP (Cast) / TEARDOWN (AirPlay)
so the TV leaves mirroring mode. Ctrl-C in a terminal sends SIGINT to the
whole foreground process group (Windows: CTRL_C_EVENT to every process on
the console). The default action kills the process immediately. std has no
API to install a handler or ignore the signal, so a std-only program cannot
run any teardown on Ctrl-C. The suggested fallback ("ffmpeg gets SIGINT,
exits, and we tear down on its EOF") does not work: tvpipe receives the
same signal and dies at the same moment.

## Decision

`src/util/interrupt.rs` is the only platform-specific code outside the
config-directory paths:

- `cfg(unix)`: `signal(SIGINT/SIGTERM, handler)` from the C library std
  links anyway. The handler only does an atomic swap (async-signal-safe);
  a second signal calls `_exit(130)`. One code path for every unix (the
  signal numbers and `signal()`'s BSD semantics are the same on Linux,
  macOS and the BSDs); no per-OS branches.
- `cfg(windows)`: kernel32 `SetConsoleCtrlHandler`. The handler runs on its
  own thread, sets the flag and returns TRUE; a second Ctrl-C returns FALSE
  so the default handler ends the process.
- Safe API: `interrupt::install()`, `interrupt::requested()`; every
  `unsafe` block has a SAFETY comment. No crate is added.
- Std-only addition: `interrupt::watch_stdin()` stops on `q` + Enter when
  stdin is a terminal (not used when stdin carries SLRW input).

## How `play` uses it

ffmpeg is in our process group, so it gets SIGINT too and ends its
output; the session sees EOF (or polls `requested()` between frames) and
tears down without draining: fanout closed, every sink's `stop()` (STOP +
CLOSE for Cast), ffmpeg killed and reaped. A watcher thread kills ffmpeg if
it keeps running after a stop request (SIGTERM, `q`, Windows), so a
blocked read on its stdout always ends.

## Alternatives considered

- A signal crate (`ctrlc`, `signal-hook`): a new dependency for ~20 lines.
- `libc` crate: same FFI with a dependency.
- No handler: receivers notice the closed TLS connection / missing
  heartbeats eventually; the openscreen receiver ends the session on
  disconnect, real TVs may linger in mirroring mode.
