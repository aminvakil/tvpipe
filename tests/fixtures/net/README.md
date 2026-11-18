# net fixtures

Captured on the dev LAN on 2026-11-17. Identifiers (MACs, device ids, pairing
ids, public keys, serials) are replaced by fakes of the same length, so the
wire layout is unchanged; the fake device id of the Apple TV is
`AA:BB:CC:00:00:0A` in every file.

- `lan-airplay.txt`: mDNS legacy-unicast responses (vector format, `from` +
  `msg`), produced by `tvpipe --debug discover` and `scrub.py`.
- `lan-airplay.table`: expected `discover` table for them (regenerate with
  `TVPIPE_UPDATE_FIXTURES=1 cargo test`).
- `appletv-info.rtsp`: raw response of an Apple TV 4K (AppleTV6,2, tvOS
  26.6.1, AirTunes/960.13.1) to `GET /info RTSP/1.0` without pairing; the
  body is a binary plist (`pk` replaced by bytes 00..1f).
- `appletv-options-403.rtsp`: the same Apple TV answering `OPTIONS *`.
- `sonos-info.rtsp`: a Sonos One (AirTunes/366.0) answering `GET /info`.
