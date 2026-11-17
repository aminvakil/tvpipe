# Task board

One file per workstream. The owner updates the `Status` line and the
checklist as it lands pieces on `main`.

| Card | Owner | Branch / worktree | Status |
|---|---|---|---|
| [W1 crypto](W1-crypto.md) | subagent `w1-crypto` | `w1-crypto` / `../streamlib-w1` | done |
| [W2 codecs](W2-codecs.md) | subagent `w2-codecs` | `w2-codecs` / `../streamlib-w2` | done |
| [W3 net](W3-net.md) | subagent `w3-net` | `w3-net` / `../streamlib-w3` | done (LG/Samsung/Cast captures pending) |
| [W4 media](W4-media.md) | subagent `w4-media` | `w4-media` / `../streamlib-w4` | done |
| [W5 AirPlay pairing](W5-airplay-pairing.md) | subagent `w5-pairing` | `w5-pairing` / `../streamlib-w5` | done (paired + re-verified on Apple TV) |
| W6 FairPlay (clean) | — | — | waits for spec S1 |
| W7 AirPlay mirror | — | — | waits for W4+W5+W6+S2 |
| [W8 Cast](W8-cast.md) | subagent `w8-cast` | `w8-cast` / `../streamlib-w8` | done on openscreen receiver; real hardware pending |
| S1/S2 specs (dirty) | — | — | not started; needs human go-ahead |

## Notes for later workstreams

- W7: ffmpeg's ALAC encoder always emits 4096-sample frames. Plan AirPlay
  audio as `PcmS16le` with `frame_samples = 352` (ffmpeg cuts exact packets)
  and wrap PCM as uncompressed ALAC frames in `airplay/audio.rs`.
- Lead (session.rs): ffprobe the input before planning so inputs without
  audio don't get `0:a:0?` maps (avoids ffmpeg warnings). Done (play-cast).
- Lead: switch `cast/annexb.rs` and the private clock in `cast/sink.rs` to
  `media::h264` / `media::clock` (both on main now). Done (play-cast); the
  Cast sink keeps its own anchoring rule on `media::clock` (early frames
  keep future reference times; ffmpeg delivers in bursts).
- `play --to` forms: `NAME` (discovery), `IP` (Cast on 8009), `IP:PORT`
  (Cast, except 7000 = AirPlay), `cast:…` / `airplay:…` to force one.
  Devices with both protocols go over Cast until AirPlay mirroring lands.
- Ctrl-C: `util::interrupt` (decision 0004). SIGINT/SIGTERM/`q` stop the
  session without draining; a second Ctrl-C exits at once (130).
- `-re` inputs: ffmpeg's 0.5 s initial read burst overruns the Cast
  in-flight window; `media::ffmpeg` now passes `-readrate_initial_burst
  0.001` (0 means "default" in ffmpeg 8).
