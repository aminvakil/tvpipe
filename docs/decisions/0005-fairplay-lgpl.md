# 0005: FairPlay as an LGPL port (for now)

Status: accepted by the human (2026-11-18).

The human chose the PLAN §10 fallback: port doubletake's FairPlay SAP
implementation into `src/airplay/fairplay/` under LGPL-3.0-or-later instead
of a clean-room reimplementation.

- Scope: `src/airplay/fairplay/**`, `tests/vectors/fairplay/**`, and the
  LGPL license text. Public surface as in PLAN §7.3 (`FpSession`: `m1`,
  `m3`, `check_m4`, `wrap_key`), plus the stream-key derivation from the
  FairPlay key and the pair-verify secret, so the module is a drop-in
  swap for a later clean-room version.
- Every file in that directory carries an LGPL-3.0-or-later SPDX header
  and attribution to doubletake. Commit bodies say
  `Sources: doubletake <path> (LGPL-3.0-or-later)`.
- The rest of the crate stays Apache-2.0 and must not copy from it. The
  distributed binary is under LGPL terms while this module is in it
  (`Cargo.toml` license: `Apache-2.0 AND LGPL-3.0-or-later`).
- The porter is a separate, single-purpose agent and never works on other
  modules.
- AirPlay mirroring itself (`mirror.rs`, `audio.rs`, `timing.rs`) stays on
  the clean-room path: a dirty-room spec writer produces
  `docs/spec/airplay-mirroring.md`, and a clean implementer (W7) works from
  it.
