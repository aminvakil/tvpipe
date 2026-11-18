# 0001: Core types and conventions (M0)

Status: accepted by lead, pending human review (checkpoint 1).

Frozen interfaces (changes need a new decision record; amended by 0010
`Sink::set_clock` and 0013 `Capabilities::display_aspect`):

- `src/types.rs`: `TrackKind`, `AudioFamily`, `VideoCodec`, `AudioCodec`,
  `EncodedFrame`, `VideoInfo`, `AudioInfo`, `AudioFormat`, `Capabilities`,
  `Encoder`, `VideoPlan`, `AudioPlan`, `EncodePlan`, `SinkStatus`, `Sink`.
- `src/error.rs`: one crate-wide `Error { kind, msg, source }` with
  `ErrorKind` and `Result<T>`. Modules may use private error enums but convert
  at their public boundary. Malformed input → `ErrorKind::Malformed`, message
  prefixed with the format name (`"dns: ..."`).
- `src/util/bytes.rs`: `Reader` (bounds-checked, never panics) and `Writer`.
  Every codec parses through `Reader`.
- `src/util/vectors.rs`: test-vector files (`key = value` blocks, blank-line
  separated, `#` comments, indented continuation lines) under
  `tests/vectors/<module>/`; binary fixtures under `tests/fixtures/<module>/`.
- Logging: `error!`/`warn!`/`info!`/`debug!`/`trace!` macros from `src/log.rs`.

Conventions:

- Timestamps: video pts in 90 kHz ticks, audio pts in sample ticks.
- Video payloads are AVCC (4-byte length prefixes) everywhere.
- Each workstream owns its directory's `mod.rs`; top-level modules are
  already declared in `main.rs`, so workstreams never need to edit it.
- `#![allow(dead_code)]` stays in `main.rs` until M10 wiring.
