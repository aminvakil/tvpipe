# airplay fixtures

- `appletv-info-verified.bplist`: body of `GET /info RTSP/1.0` sent over the
  encrypted control channel right after pair-verify, from the dev Apple TV 4K
  (AppleTV6,2, tvOS 26.6.1, AirTunes/960.13.1), 2026-11. Scrubbed with
  `scrub_info.py` (same-length replacements, so the bplist layout is as
  received): device id `AA:BB:CC:00:00:0A`, MAC `AA:BB:CC:00:00:99`, fake
  `pi`/`psi`, `pk` = bytes 00..1f, sender address `192.168.1.101:66666`.
  Same keys as the unpaired `tests/fixtures/net/appletv-info.rtsp`: the
  Apple TV sends no `displays`/`audioFormats` in `/info`.
