# W5 AirPlay pairing + secure channel (M4)

Status: done (paired with the Apple TV; re-verify + encrypted /info checked on hardware). Owns `src/airplay/{pairing,secure,credentials,info}.rs`,
`src/airplay/pair.rs` (command bodies), `tests/vectors/airplay/**`,
`tests/fixtures/airplay/**`. `airplay/discovery.rs` belongs to W3; read it,
ask before changing it.

Builds on: `crypto::{srp, ring, bigint}` (W1, merged), `codec::{tlv8,
bplist}` (W2, merged), `net::{discover, device, iface}` (W3, merged) and
`net::{http, rtsp}` (W3, landing soon; start with the parts that don't need
it, or with a minimal private HTTP helper you delete once W3's lands).

- [x] `credentials.rs`: controller identity (Ed25519 seed + pairing UUID,
      generated once) and paired receivers (receiver id → long-term public
      key, name, last address) persisted in the config dir (PLAN §4; path
      from env vars, `cfg(target_os)` allowed here only). Simple line-based
      text file, written atomically (temp + rename), permissions 0600 on
      unix where possible via std only.
- [x] `pairing.rs`: HAP pair-setup over `POST /pair-setup` (TLV8 M1–M6,
      SRP-6a 3072/SHA-512, `Pair-Setup`, HKDF salts/infos, ChaCha20-Poly1305
      with `PS-Msg05`/`PS-Msg06` nonces, Ed25519 signatures over
      `iOSDeviceX`/`AccessoryX`); PIN flow including `POST
      /pair-pin-start` to make the TV show the PIN; transient pair-setup
      (M1–M4, PIN 3939, Flags 0x10); pair-verify (`POST /pair-verify`,
      X25519, `PV-Msg02`/`PV-Msg03`). Headers the receiver expects
      (`Content-Type: application/octet-stream`, `X-Apple-HKP: 3|4`, user
      agent like `AirPlay/xxx`) — take them from pyatv, cite it.
- [x] `secure.rs`: encrypted control channel over a `TcpStream`
      (`u16le len | ciphertext | tag`, ≤1024-byte frames, LE counter
      nonces, `Control-Salt` / `Control-Write-Encryption-Key` /
      `Control-Read-Encryption-Key`), exposing `Read + Write` so
      `net::http`/`net::rtsp` run on it unchanged. Expose the pair-verify
      shared secret (W7 needs it for stream keys).
- [x] `info.rs`: `GET /info` (plain before verify, and over the secure
      channel after), parse the bplist into a struct with the fields we
      need (features, model, display modes/size, audio formats, timing
      protocol, pk) and map to `types::Capabilities`. Save a scrubbed real
      `/info` from the Apple TV as a fixture.
- [x] `pair.rs`: command bodies `airplay::pair::pair(target, pin)` and
      `airplay::pair::unpair(target)`, resolving the target with
      `net::discover::resolve`. Lead wires them into `session.rs`.
- [x] Local tests: a full pair-setup + pair-verify + secure channel
      round-trip against an in-process fake receiver built on
      `crypto::srp::SrpServer`, so CI covers the protocol without hardware.

Done when: `tvpipe pair "Livingroom (2)"` pairs with the Apple TV
(192.168.1.34, AppleTV6,2) with a human typing the PIN; a later run
re-verifies with stored credentials and fetches `/info` over the encrypted
channel; the local fake-receiver loop is green in `cargo test`.

Hardware/human: you can talk to the Apple TV freely (`/info`, transient
pairing). For PIN pairing, the human has to read the PIN off the TV: send
the lead an [asks] message saying exactly what command to run, and either
the human runs it or relays the PIN. Prefer `--debug` logs over guesses.

Sources allowed: HAP specification (public), pyatv (MIT:
`pyatv/auth/hap_*.py`, `pyatv/protocols/airplay/auth/*`), RFCs. Never
doubletake/UxPlay/RPiPlay/playfair/airfry/RottingApple.

## Notes for integration

- Wiring (lead, `session.rs`): `Command::Pair { target, pin } =>
  crate::airplay::pair::pair(&target, pin.as_deref())`, `Command::Unpair
  { target } => crate::airplay::pair::unpair(&target)`.
- W7 entry point: `airplay::pair::resolve(name)` → `Target`, then
  `airplay::pair::connect(&target, &Store::open_default()?)` → `Control
  { client: RtspClient<SecureStream<TcpStream>>, shared, addr, auth }`.
  `connect` pair-verifies with stored credentials, else tries transient
  pair-setup, else fails with an Auth error telling the user to run
  `tvpipe pair`. `control.shared.secret()` is the pair-verify X25519
  secret (32 bytes) or the transient SRP key (64 bytes) for stream keys;
  `airplay::secure::Keys::derive(secret, salt, write_info, read_info)`
  derives other channel keys (events, data stream).
- `airplay::info::get_info(&mut control.client)` → `Info`;
  `info.capabilities()` → `types::Capabilities` (audio formats are a
  placeholder, ALAC 44.1 kHz stereo, until S2 specifies `audioFormats`).
- Credentials: `credentials.txt` in the config dir; `TVPIPE_CONFIG_DIR`
  overrides the directory.
- `unpair` only forgets the local record; removing the pairing on the
  receiver (HAP remove-pairing) is not implemented (no public reference
  for AirPlay's endpoint).
- Hardware: the Apple TV refuses transient pair-setup (470) and needs
  PIN pairing; pairing requests work as RTSP/1.0 on the control port.
  PIN pairing (human typed the PIN) and a later pair-verify with stored
  credentials plus `GET /info` over the encrypted channel both work.
- The Apple TV's `/info` is identical before and after verify and has no
  `displays`, `audioFormats` or `audioLatencies`, so `capabilities()`
  falls back to 1920x1080@60, H.264 (+HEVC via feature bit 42) and ALAC
  44.1 kHz stereo. Real display size and audio formats have to come from
  elsewhere (S2/W7). `Info::supported_formats` keeps the per-stream
  bitmasks raw.
  Ignored tests `hardware_*` in `airplay/pair.rs` (env `TVPIPE_ATV`).
