# 0011: Windows timer resolution (second platform FFI exception)

Status: accepted by the lead (2026-11-18, branch `windows-vm`), as the
fallback after the std-only route measured too coarse.

## Problem

tvpipe paces and polls with short timed waits: the Cast sender sends a
burst every 10 ms (`Condvar::wait_timeout` until the next burst), every
UDP receive loop polls with a 10-100 ms `SO_RCVTIMEO`, the AirPlay writer
waits on a condvar for heartbeats. On Windows these waits end on the
system timer tick (15.6 ms by default), not when they are due.

Measured on Windows 11 ARM64 (build 26200, Parallels VM, 4 vCPUs, idle;
100 waits each, `target/exp` probe, release build):

| wait | default timer: avg / p50 / p90 / max | `timeBeginPeriod(1)`: avg / p50 / p90 / max |
|---|---|---|
| `thread::sleep(1 ms)` | 3.0 / 2.8 / 5.3 / 6.5 ms | 4.4 / 4.4 / 7.4 / 7.7 ms |
| `thread::sleep(10 ms)` | 17.9-21.4 / 15.9-16.5 / 24-43 / 48-50 ms | 10.7 / 10.5 / 11.5 / 13.2 ms |
| `Condvar::wait_timeout(1 ms)` | 19.6-24.7 / 16.7 / 19.7-57 / 73-77 ms | 2.5 / 2.1 / 4.2 / 9.3 ms |
| `Condvar::wait_timeout(10 ms)` | 25.6-26.7 / 17.0 / 47-63 / 74-78 ms | 10.6 / 10.8 / 11.0 / 11.5 ms |
| UDP read timeout 1 ms | 24.5-24.8 / 16.8 / 54-69 / 74-79 ms | 1.6 / 1.9 / 2.0 / 2.4 ms |
| UDP read timeout 10 ms | 23.2 / 16.8 / 48 / 70-76 ms | 10.7 / 10.7 / 11.5 / 12.5 ms |

(macOS for comparison: every wait within ~0.3 ms of due, 1.3 ms for
1 ms, 11-12.5 ms for 10 ms.)

The effect in tvpipe: Cast bursts and the fake receiver's feedback came
tens of ms late, the sender's in-flight window (target delay 100 ms)
filled, audio frames were dropped (`MaxDurationInFlight`), and
`session::tests::{passthrough,encoded_slrw}_to_cast_without_ffmpeg`
failed about one run in five. On a real receiver the same lateness makes
the stream bursty.

## Std-only route (tried first)

Recent std's `thread::sleep` on Windows uses a high-resolution waitable
timer, so pacing with `sleep` instead of condvar/socket timeouts looked
like an option. Measured, it is not precise enough: `sleep(10 ms)` still
takes 16 ms at the median and up to ~50 ms; only `sleep(1 ms)` is close
(~3 ms). Pacing with chains of 1 ms sleeps would mean rewriting every
pacing and polling loop (Cast sender, AirPlay writer, every receive loop,
the fakes) into busy-ish sleep loops, and would still overshoot by a few
ms per wait. Rejected as too invasive for an imprecise result.

## Decision

`src/util/timer.rs` is the second platform-specific module (after
`util/interrupt.rs`, decision 0004):

- `raise_resolution()`: `cfg(windows)` calls winmm `timeBeginPeriod(1)`
  once per process (`std::sync::Once`); elsewhere a no-op. winmm is a
  system DLL; no crate. The one `unsafe` call has a SAFETY comment.
- Called at the start of `main`, of `session::play_with` (so tests that
  skip `main` get it) and of the Cast/AirPlay fakes.
- No `timeEndPeriod`: since Windows 10 2004 the resolution applies to the
  calling process only and ends with it.

Result: the same waits as above take ~1-2 ms (1 ms) and ~10.7 ms
(10 ms); the two session tests passed 60 of 60 runs (before: ~1 in 5
failed), the whole suite 4 of 4.

## Costs

A higher timer interrupt rate costs some power while tvpipe runs; it is a
streaming tool that runs while streaming. On Windows 11 a process whose
windows are all hidden may not get the resolution guaranteed; tvpipe is
a console process and the measurements above were taken without a
console window (`prlctl exec`).
