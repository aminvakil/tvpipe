# 0010: Replication: one timeline, one budget, partial failures (M10)

Status: proposal for the lead (branch `multi-receiver`). Changes one
frozen interface: `Sink` gains a defaulted method (below).

## Shared timeline

Before, every sink anchored its own `MediaClock` on the first frame it
pushed, so two receivers could be apart by however late each sink thread
got its first frame (queueing, a slow first push, a keyframe wait): by
construction tens to hundreds of ms.

Now the fanout owns one `media::clock::MediaClock` for the session:

- `Fanout::push` schedules every frame on it as it leaves the input
  (`MediaClock::schedule`, the rule both sinks had: the first frame
  anchors; early frames keep their future instants; more than 100 ms late
  (a stalled source) or 1 s early (a jump) re-anchors to now), before any
  per-sink queueing. The instant goes into the sink's queue with the frame.
- Each sink gets a `SessionClock` (a view of that clock) through
  `Sink::set_clock` before `start()`. Right before the sink thread pushes a
  frame, it pins the frame's scheduled instant on the view, so a sink gets
  the instant decided for everyone even when the anchor moved meanwhile
  (a stall seen while this sink still had older frames queued).
- The sinks map pts through `SinkClock` (`media::clock`): the session's
  instants when they have a view (read only: a sink that falls behind
  does not move everyone's timeline; its frames just have less headroom),
  else a private clock with the old rule (sinks used on their own, unit
  tests). AirPlay maps the instant to PTP/NTP, Cast to RTP + Sender
  Reports, as before.

Interface change in `src/types.rs`:

```rust
fn set_clock(&mut self, clock: crate::media::clock::SessionClock) {}
```

Defaulted, so other sinks compile unchanged; wrappers must forward it
(`AnyMirrorSink` does).

Receivers are still not frame-locked: each has its own decode/display
pipeline and clock mapping error (PTP header anchoring, NTP wall clock).

## One budget (`--latency`)

PLAN §5.3: the same playout budget for every receiver. The budget is the
delay from the shared capture instant to presentation:

| Session | Cast target delay | AirPlay leads (video / audio) |
|---|---|---|
| `--latency L` | L | L / L (spec §2.7 explicit profile) |
| Cast only, no `--latency` | 300 ms | — |
| AirPlay only, no `--latency` | — | automatic "wireless" profile, 100 / 170 ms |
| Both families, no `--latency` | 300 ms | 300 / 300 ms |

- `L` is clamped to 5–2000 ms (what both protocols accept; a warning
  says so), so one value is valid everywhere.
- Default 300 ms instead of openscreen's 200 ms: w8-cast's 5-minute
  soaks on the Google TV over Wi-Fi saw ACK/feedback gaps of ~220 ms and
  4–7 `MaxDurationInFlight` drops per 5 minutes at 200 ms, each costing
  up to a GOP waiting for a keyframe (less with the built-in encoder,
  which makes IDRs on demand). 300 ms keeps ~80 ms of margin; wired or
  quiet networks can go lower with `--latency` (e.g. `--latency 150`).
- AirPlay alone keeps the automatic profile (measured ≈250 ms
  glass-to-glass on the Apple TV, `docs/latency.md`; MediaDataControl only
  runs with automatic leads). Mixed with Cast it would show every frame
  200 ms before the Cast TV, so it takes the shared budget instead; 300 ms
  is above the wireless profile's 170 ms audio lead, so the audio headroom
  that profile was chosen for is kept.

## Partial failures

- A `--to` that cannot be resolved or connected (TV off, wrong name,
  refused) is logged as `NAME: error` and left out; the session goes on
  with the others (`warn: 1 of 2 receivers connected; going on without the
  others`). It fails only when none is left. Targets resolve and connect
  in parallel.
- A sink failing in `start()` or mid-stream, or a receiver ending the
  session (stop mirroring on the TV, closed connection), removes that sink;
  the others continue (as before, in the fanout). The session ends when
  every receiver is gone, and fails only if every sink failed.
- Ctrl-C / SIGTERM / `q`: no draining, every sink's `stop()` (Cast STOP +
  CLOSE, AirPlay TEARDOWN). `play_with` takes the stop flag as a function
  so the tests exercise this without the process-wide flag.

## Tests

`src/session/multi_tests.rs`: a fake Cast receiver and a fake AirPlay
mirror receiver in one session (built-in test pattern, no ffmpeg):

- same frames, and for every frame both sinks used the same instant,
  although their first pushes were delayed 80 and 150 ms; AirPlay
  timestamps = instant + 300 ms on the NTP timeline; Cast RTP = pts and
  every Sender Report maps RTP onto the schedule; both offered/set up with
  the 300 ms budget;
- one unreachable receiver of three is left out; all unreachable fails;
- the AirPlay receiver leaving mid-stream, then the Cast one: the other
  keeps streaming to the end;
- a stop request tears both down at once.
