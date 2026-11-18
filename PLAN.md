# tvpipe — Plan

A single, dependency-light Rust CLI that takes a video+audio source (a file, a
URL, or raw frames from a game) and mirrors it with the lowest achievable
latency to AirPlay and Google Cast receivers, on macOS, Linux, and Windows,
using the same code everywhere.

Status: planning. Nothing is implemented yet.

---

## 1. Goals and non-goals

### Goals

- **Sender only.** We push media to receivers; we never act as one.
- **Low latency first.** Use the real-time mirroring protocols of both
  ecosystems, not URL playback:
  - AirPlay: screen-mirroring session (stream type 110 + realtime audio).
  - Cast: Cast Streaming (the mirroring receiver app, RTP/RTCP over UDP).
- **Video + audio together** in one session, lip-synced.
- **One binary, one code path for every OS.** No Apple frameworks, no Windows
  or Android media APIs. Only sockets, threads, and an `ffmpeg` subprocess.
- **Minimal dependencies.** Hand-roll everything that is a format or a simple
  algorithm; depend on crates only for TLS and hard crypto.
- **Several receivers at once** from one encode where codecs allow
  ("replication"). Receivers will not be frame-locked to each other.
- **Two primary use cases:**
  1. Mirror a video file (or any ffmpeg-readable source) in real time.
  2. Mirror a game that feeds raw frames and PCM into `tvpipe`.

### Non-goals (for now)

- Acting as a receiver.
- URL/HLS playback ("play this URL on the TV"). It is easy to add later and
  shares discovery and pairing, but it is not low latency.
- A library API. The binary is the product. The internal module boundaries
  should still be clean enough to become a library later.
- Encoding in process (linking x264, libvpx, etc.). ffmpeg does that.
- Audio-only targets (HomePod, Cast audio groups).
- DRM-protected content.

---

## 2. Hard constraints

| Constraint | Consequence |
|---|---|
| Rust, single binary | `cargo build` produces `tvpipe`; `ffmpeg` is the only runtime requirement |
| Minimal deps | Allowed: `argument`, `rustls` (ring backend, no default features), `ring`. Anything else needs a written justification in `docs/deps.md` |
| No async runtime | `std::thread` + blocking sockets + channels. One thread per session/socket role |
| Cross-platform | Only `std::net`, `std::process`, `std::fs`. mDNS is implemented by hand, without OS services |
| Hand-rolled | AES (block, CTR, CBC), big-integer modpow for SRP, SRP-6a, mDNS/DNS, HTTP/RTSP, JSON, binary plist, TLV8, protobuf subset, Matroska demux, H.264 NAL/SPS parsing, RTP/RTCP, ALAC passthrough framing |
| From `ring` | Ed25519, X25519, ChaCha20-Poly1305, HKDF-SHA512, SHA-1/256/512, HMAC, CSPRNG |
| CLI | `argument = "0.1"` from crates.io (https://github.com/mitsuhiko/argument) |

---

## 3. Architecture

```
            ┌──────────────────── input ────────────────────┐
            │ file / URL / capture  │  game raw frames + PCM │
            └───────────┬───────────┴───────────┬────────────┘
                        │                       │ (SLRW framing on stdin,
                        ▼                       ▼  forwarded via localhost TCP)
               ┌─────────────────────────────────────────┐
               │ ffmpeg subprocess                        │
               │  H.264 (no B-frames, zerolatency, GOP≈1s)│
               │  audio track(s) per receiver family      │
               │  → Matroska (live) on stdout             │
               └────────────────────┬────────────────────┘
                                    ▼
               ┌─────────────────────────────────────────┐
               │ media::mkv demuxer → EncodedFrame stream│
               │ media::clock (sender media timeline)    │
               └────────────────────┬────────────────────┘
                                    ▼
               ┌─────────────────────────────────────────┐
               │ fanout (bounded per sink; a slow sink   │
               │ drops to the next keyframe)             │
               └───────┬─────────────────────────┬───────┘
                       ▼                         ▼
           ┌──────────────────────┐   ┌──────────────────────┐
           │ airplay::MirrorSink  │   │ cast::StreamingSink  │
           │ HAP pairing          │   │ TLS control channel  │
           │ FairPlay SAP + ekey  │   │ OFFER/ANSWER         │
           │ RTSP SETUP/RECORD    │   │ RTP (Cast format)    │
           │ TCP video (AES-CTR)  │   │ AES-CTR per frame    │
           │ RTP audio            │   │ RTCP ACK/NACK/PLI    │
           │ timing (PTP anchor / │   │ retransmission       │
           │ NTP responder)       │   │                      │
           └──────────────────────┘   └──────────────────────┘
```

### Session lifecycle

1. **Discover**: browse `_airplay._tcp` and `_googlecast._tcp`, then resolve
   the names given with `--to`.
2. **Connect + negotiate** every target *before* starting ffmpeg:
   - AirPlay: `/info` → pair-verify (or a pair-setup prompt) → FairPlay → read
     the receiver's display size, supported audio formats, and HEVC support.
   - Cast: connect → launch the mirroring app → read its capabilities.
3. **Plan the encode**: intersect the capabilities and choose resolution, fps,
   bitrate, and the audio tracks (for example ALAC for AirPlay, Opus or AAC for
   Cast).
4. **Start ffmpeg** with those parameters, then the demuxer, then fan out.
5. **Stream**: each sink paces itself on the media clock and handles its
   receiver's feedback.
6. **Teardown** on EOF, Ctrl-C, or when the receiver disconnects. Ctrl-C must
   send a proper TEARDOWN/STOP so the TV leaves mirroring mode.

### Core internal types (frozen in M0; every workstream codes against them)

```rust
pub enum TrackKind { Video, Audio(AudioFamily) }   // AudioFamily: which encode it belongs to

pub struct EncodedFrame {
    pub track: TrackKind,
    pub pts: u64,              // 90 kHz for video, sample-rate ticks for audio
    pub keyframe: bool,
    pub data: Arc<[u8]>,       // video: AVCC (length-prefixed NALs), audio: raw codec frame
}

pub struct VideoInfo { pub codec: VideoCodec, pub width: u32, pub height: u32,
                       pub fps: Ratio, pub avcc: Vec<u8> /* avcC record */ }
pub struct AudioInfo { pub codec: AudioCodec, pub sample_rate: u32, pub channels: u8,
                       pub frame_samples: u32, pub config: Vec<u8> }

pub struct Capabilities { /* max WxH@fps, video codecs, audio codecs, latency hints */ }
pub struct EncodePlan   { /* what ffmpeg must produce */ }

pub trait Sink: Send {
    fn name(&self) -> &str;
    fn capabilities(&self) -> &Capabilities;                     // after connect
    fn start(&mut self, video: &VideoInfo, audio: Option<&AudioInfo>) -> Result<()>;
    fn push(&mut self, frame: &EncodedFrame) -> Result<SinkStatus>; // never blocks long
    fn wants_keyframe(&self) -> bool;                             // e.g. PLI from receiver
    fn stop(&mut self) -> Result<()>;
}
```

### Source layout

```
src/
  main.rs                CLI entry (argument), command dispatch
  cli.rs                 specs + parse functions
  log.rs                 tiny leveled logger to stderr (--debug / -v)
  util/                  hex, byte readers/writers, ratio, time helpers
  crypto/
    aes.rs               AES-128/256 block (table-based, hand-rolled)
    modes.rs             CTR, CBC
    bigint.rs            fixed-size unsigned modpow (for SRP-3072)
    srp.rs               SRP-6a (SHA-512, 3072-bit group, RFC 5054)
    ring.rs              thin wrappers over ring (ed25519, x25519, chacha, hkdf, sha)
  codec/
    json.rs              minimal JSON value + parser/serializer
    bplist.rs            binary plist read/write (bplist00)
    tlv8.rs              HAP TLV8
    protobuf.rs          CastMessage encode/decode (only that message)
  net/
    mdns.rs              DNS packet build/parse, legacy-unicast queries, browse loop
    iface.rs             "which local IP reaches X" via UDP connect + local_addr
    http.rs              minimal HTTP/1.1 client framing (shared with RTSP)
    rtsp.rs              RTSP request/response
  media/
    ffmpeg.rs            locate ffmpeg, build argv from EncodePlan, spawn, stderr pump
    mkv.rs               EBML/Matroska live demuxer (SimpleBlock, BlockGroup, CodecPrivate)
    h264.rs              NAL iteration, SPS parse (dimensions/profile), avcC handling
    rawin.rs             game input: SLRW framing → localhost TCP into ffmpeg
    clock.rs             sender media clock (monotonic ↔ pts)
    fanout.rs            bounded per-sink queues with keyframe-skip policy
  airplay/
    discovery.rs         TXT record parsing (features bits, pk, model, srcvers)
    info.rs              GET /info (bplist), feature/capability model
    pairing.rs           pair-setup (PIN, transient) + pair-verify (HAP)
    secure.rs            encrypted control channel framing (ChaCha20-Poly1305)
    credentials.rs       persisted controller identity + paired receivers
    fairplay/            FPSAP m1/m3 + ekey wrap (see §7.3, clean-room)
    mirror.rs            SETUP/RECORD, stream keys, TCP video packetizer
    audio.rs             realtime audio stream (RTP, encryption)
    timing.rs            PTP anchor from receiver headers; NTP responder for legacy
    sink.rs              MirrorSink: Sink impl
  cast/
    channel.rs           TLS (rustls, custom verifier) + length-prefixed CastMessage
    messages.rs          CONNECT/PING/LAUNCH/GET_STATUS/OFFER/ANSWER JSON
    streaming.rs         OFFER construction, ANSWER parsing, session params
    rtp.rs               Cast RTP packetization, AES-CTR frame encryption
    rtcp.rs              SR/RR, Cast feedback (ACK/NACK), PLI, receiver log
    retransmit.rs        packet history + NACK handling, frame-ack window
    sink.rs              StreamingSink: Sink impl
  session.rs             orchestration: connect → plan → ffmpeg → fanout → teardown
```

---

## 4. CLI

```
tvpipe discover [--timeout SECS] [--json]
tvpipe pair NAME|ADDR [--pin PIN]          # interactive PIN if omitted
tvpipe unpair NAME|ADDR
tvpipe play --to NAME|ADDR [--to ...]      # repeatable: replication
               [--latency MS]                  # target playout delay (default ~150–250)
               [--size WxH] [--fps N] [--bitrate KBPS]
               [--encoder auto|x264|videotoolbox|nvenc|qsv|vaapi|mf]
               [--no-audio] [--loop] [--realtime/--no-realtime]
               [--raw-video WxH@FPS:PIXFMT] [--raw-audio RATE:CH:FMT]   # game mode, SLRW on stdin
               [--ffmpeg PATH] [-- FFMPEG_INPUT_ARGS...]                # escape hatch
               INPUT | -
tvpipe test-pattern --to ...                # ffmpeg testsrc2 + burned-in wallclock for latency measurement
```

Notes:
- `NAME` matches discovery names case-insensitively, and unique prefixes are
  accepted. `ADDR` skips discovery.
- Credentials live in the platform config dir: `$XDG_CONFIG_HOME/tvpipe`,
  `~/Library/Application Support/tvpipe`, or `%APPDATA%\tvpipe`. The
  path is computed from env vars, with no crate.
- Exit codes: 0 ok, 1 runtime error, 2 usage error (from `argument`).

---

## 5. Media pipeline

### 5.1 ffmpeg invocation (starting point; tuned in M2)

```
ffmpeg -hide_banner -nostats -loglevel warning
  [-re] -i INPUT                                       # -re for files; not for live/game input
  -map 0:v:0 -c:v libx264 -preset veryfast -tune zerolatency
     -profile:v high -pix_fmt yuv420p -bf 0
     -g {fps} -keyint_min {fps} -sc_threshold 0
     -b:v {kbps}k -maxrate {kbps}k -bufsize {kbps/fps*2}k
     -vf scale={w}:{h}:force_original_aspect_ratio=decrease,pad=...,fps={fps}
  -map 0:a:0? -c:a alac -ar 44100 -ac 2               # AirPlay family
  -map 0:a:0? -c:a libopus -ar 48000 -ac 2 -frame_duration 10 -application lowdelay   # Cast family
  -f matroska -live 1 -cluster_time_limit 0 -flush_packets 1 pipe:1
```

- **Matroska** is the pipe format. It carries every codec we need (H.264, ALAC,
  Opus, AAC), and stores H.264 as AVCC with an `avcC` CodecPrivate, which is
  exactly what AirPlay mirroring wants.
- Hardware encoders through `--encoder`; `auto` probes `ffmpeg -encoders`.
  Each has its own low-latency flag set (for example `-realtime 1` for
  VideoToolbox, `-tune ll` / `-zerolatency 1` for NVENC).
- **Keyframes on demand are impossible** with a subprocess encoder. Mitigation:
  a ~1 s GOP, and sinks wait for the next IDR after loss or when they join.
  An in-process encoder could fix this later.
- **Bitrate adaptation** (doubletake does this) would need encoder restarts
  or an in-process encoder. Out of scope until M9; use a fixed bitrate.
- `--` passes extra input args straight to ffmpeg (for example
  `-f avfoundation -i 1:0` or `-f gdigrab -i desktop`), so any capture source
  ffmpeg supports works without platform code in `tvpipe`.

### 5.2 Game input (SLRW)

The game spawns `tvpipe play --raw-video 1920x1080@60:rgba --raw-audio
48000:2:s16le -` and writes a trivial framing to its stdin:

```
"SLRW" magic once, then repeated: u8 kind (1=video,2=audio,0=end) | u64 pts_us | u32 len | payload
```

`tvpipe` opens two localhost TCP listeners, starts ffmpeg with
`-f rawvideo ... -i tcp://127.0.0.1:A -f s16le ... -i tcp://127.0.0.1:B`, and
copies the payloads across. This works the same way on every OS, without fd
passing or named pipes. `pts_us` lets us detect drift and drop late frames
before encoding. ffmpeg itself gets wallclock-based timestamps
(`-use_wallclock_as_timestamps 1`).

### 5.3 Clocks and pacing

- The demuxer emits frames as soon as they arrive. For file input `-re` paces
  them; for game input the game does.
- `media::clock` maps the first frame's pts to a monotonic `Instant`. Each sink
  converts that into its receiver's timeline:
  - AirPlay PTP: anchored on the receiver's `X-Apple-RequestReceivedTimestamp`
    response header. No local PTP stack is needed; this approach is from
    doubletake.
  - AirPlay NTP (legacy receivers): we answer the receiver's timing requests on
    our timing port.
  - Cast: RTP timestamps plus RTCP Sender Reports that map RTP to NTP time.
- The playout delay (`--latency`) is negotiated per sink. With several
  receivers, the same budget is used for all of them.

---

## 6. Discovery (hand-rolled mDNS)

- Send PTR queries for `_airplay._tcp.local`, `_raop._tcp.local`, and
  `_googlecast._tcp.local` to `224.0.0.251:5353` **from an ephemeral port**.
  Under RFC 6762 §6.7 (legacy unicast), responders then reply by unicast to that
  port. This means we never bind 5353, never conflict with
  mDNSResponder/Avahi/Windows, need no OS service, and need no multicast-join
  privileges.
- Parse PTR, SRV, TXT, A, and AAAA records, including name compression.
  Re-query every 1 s, 2 s, 4 s, and so on until the timeout.
- Send queries on every non-loopback IPv4 interface. Enumerating interfaces
  without OS APIs is awkward, so we use the default route (UDP connect trick)
  plus any `--interface ADDR` the user gives. IPv6 is optional, later.
- Combine the AirPlay and Cast records of the same device (Samsung and LG
  advertise both) into one entry with two capabilities.
- Useful TXT fields:
  - AirPlay: `features` (64-bit bitfield across `0x…,0x…`), `pk`, `model`,
    `srcvers`, `deviceid`, `flags`, `pw`.
  - Cast: `fn` (friendly name), `md` (model), `id`, `ca` (capabilities
    bitmask).

---

## 7. AirPlay mirroring sender

Primary reference for *behaviour*: doubletake (LGPL-3.0-or-later). See §10 for
how we use it without copying it. Pairing and the general AirPlay 2 control
flow can also be learned from pyatv's docs and code (MIT).

### 7.1 Pairing (HAP-style)

- **Controller identity**: a long-term Ed25519 keypair plus a UUID, generated
  once and persisted.
- **pair-setup (PIN)**, `POST /pair-setup`, TLV8 M1–M6:
  - SRP-6a with the 3072-bit group, SHA-512, username `Pair-Setup`. The TV
    shows the PIN.
  - Then exchange signed long-term keys, encrypted with ChaCha20-Poly1305
    under HKDF-SHA512 keys (`Pair-Setup-Encrypt-Salt/Info`, and so on).
  - Store the receiver's long-term public key.
- **Transient pair-setup**: M1–M4 only, PIN `3939`, flag `0x10`. Used when the
  receiver allows anyone on the network.
- **pair-verify**, `POST /pair-verify`, M1–M4: ephemeral X25519, both sides
  sign, and we get a shared secret.
- **Encrypted control channel**: after verify, every RTSP/HTTP byte on the
  control connection is framed as `u16le length | ChaCha20-Poly1305(ciphertext)
  | tag`, with separate read/write keys from HKDF (`Control-Salt`,
  `Control-Write-Encryption-Key`, `Control-Read-Encryption-Key`) and a
  little-endian counter nonce.
- **Legacy receivers** (Apple TV 3, some third-party sets): older
  `pair-setup-pin`/legacy verify. Supported only once the hardware matrix needs
  it.

### 7.2 Receiver info

`GET /info` returns a bplist with display modes, audio formats, the feature
bitfield, and the timing protocol. This drives `Capabilities`. Feature bits we
care about: screen mirroring, FairPlay SAP (FPSAP), HEVC, PTP, and the
third-party-TV family. Profiles named in doubletake's test receiver (`modern`,
`roku`, `lg`, `appletv3`, `uxplay`, `airserver`) show which quirk families
exist.

### 7.3 FairPlay SAP (the hard part)

The wire behaviour as we understand it (to be confirmed by the spec in
`docs/spec/fairplay.md`):

- `POST /fp-setup` with header `X-Apple-ET: 32`.
- **m1** (16 bytes): an `FPLY` header plus a capability byte.
- **m2** (142 bytes): the receiver picks a mode (0–3) and sends an encrypted
  128-byte receiver SAP value.
- **m3** (164 bytes):
  - the sender's own 128-byte SAP value, encrypted with the mode's message
    cipher;
  - plus a 20-byte tail computed from both SAP values by a white-box AES-like
    network and a modified MD5.
- **m4** (32 bytes): echoes the m3 tail, which we validate.
- **ekey** (72 bytes) wraps a random 16-byte key:
  - `FPLY` header, random mask, length;
  - HMAC-SHA1;
  - the AES-wrapped (key XOR mask);
  - the wrapping keys are derived from the receiver SAP and m3.
- **Stream master key** = a function of the FairPlay key and the pair-verify
  shared secret (SHA-512 based). Per-stream keys and IVs are derived from
  that plus `streamConnectionID`.

Implementation: `airplay/fairplay/` with a tiny surface:

```rust
pub struct FpSession { .. }
impl FpSession {
    pub fn new(rng) -> Self;
    pub fn m1(&self) -> [u8; 16];
    pub fn m3(&mut self, m2: &[u8]) -> Result<[u8; 164]>;
    pub fn check_m4(&self, m4: &[u8]) -> Result<()>;
    pub fn wrap_key(&self, key: [u8; 16], rng) -> Result<[u8; 72]>;
}
```

The module is fully isolated, so it can be swapped between the clean-room
implementation and an LGPL port without touching anything else (§10).

### 7.4 Mirroring session

1. RTSP `SETUP` #1 (bplist): sender identity, `ekey`, `eiv`, `et`,
   `timingProtocol` (PTP or NTP), `isScreenMirroringSession`, session UUID,
   and our timing port for NTP. The response gives the event port and
   `timingPeerInfo` (PTP clock id).
2. Connect to the event port (we keep it open but mostly ignore it).
3. RTSP `SETUP` #2: `streams: [{type: 110, streamConnectionID, …},
   {type: 96, ct, audioFormat, spf, sr, latency…}]`. The response gives the
   `dataPort` for video and the UDP ports for audio and control.
4. `RECORD` (where required), then the initial volume (third-party TVs are
   picky about this), then periodic feedback and keep-alive (`POST /feedback`).
5. **Video data channel** (TCP to `dataPort`). Each packet has a 128-byte
   header (payload size LE, payload type, NTP/PTP timestamp, …) and a payload:
   - type *codec*: the `avcC` record, plaintext; sent at start and on any SPS
     change;
   - type *video*: AVCC frame, AES-128-CTR with a continuous keystream;
   - type *heartbeat*.
6. **Audio**: RTP over UDP. ALAC (natively encodable by ffmpeg) or AAC-ELD
   (needs `libfdk_aac`), AES-CBC or ChaCha depending on the receiver. Use
   ALAC when the receiver lists it; error out with a clear message if only
   AAC-ELD is offered and our ffmpeg can't encode it.
7. **Teardown**: `TEARDOWN` for the streams, then the session.

### 7.5 Expected receiver quirks (from doubletake's compatibility notes)

- Apple TVs (3rd gen, 4K gen 2/3), Samsung TU8300, Roku, TCL, and Hisense are
  reported working by doubletake. **LG is not on its list.** LG has its own
  test-receiver profile, so expect quirks.
- Apple receivers resend volume on engagement. Third-party TVs need an
  explicit initial volume.
- Some receivers support only AAC-ELD audio.

---

## 8. Cast Streaming sender

Primary reference: Chromium's **openscreen** (`cast/streaming`,
`cast/standalone_sender`; BSD-3-Clause, so porting is allowed with
attribution).

### 8.1 Control channel

- TLS to port 8009 (rustls with a verifier that accepts the device's
  self-signed certificate; device-auth checking is optional and skipped at
  first).
- Framing: `u32be length | CastMessage protobuf`. Fields: protocol_version,
  source_id, destination_id, namespace, payload_type, payload_utf8,
  payload_binary.
- Namespaces: `tp.connection` (CONNECT/CLOSE), `tp.heartbeat` (PING/PONG every
  5 s), `receiver` (LAUNCH, GET_STATUS, STOP), and
  `urn:x-cast:com.google.cast.webrtc` (OFFER/ANSWER and status).
- Launch the mirroring receiver app (`0F5096E8` for audio+video), then CONNECT
  to its `transportId`.

### 8.2 OFFER / ANSWER

- The OFFER lists the streams. Per stream: index, type (`video_source` or
  `audio_source`), `codecName` (h264 / vp8 / opus / aac), RTP profile `cast`,
  payload type, SSRC, `targetDelay`, `aesKey`, `aesIvMask` (random 16 B each),
  `timeBase`, max fps/bitrate, and resolutions.
- The ANSWER returns `udpPort`, `sendIndexes`, `ssrcs`, and optionally receiver
  constraints and display info. Constraints feed back into `EncodePlan`.
- The video codec is H.264, so one encode can serve AirPlay and Cast. Audio is
  Opus (or AAC, if the receiver prefers it) as a separate audio track.

### 8.3 Media transport

- UDP to `udpPort`. Each encoded frame is split into packets of at most ~1400
  bytes, with the Cast RTP header extension: keyframe flag, frame id, packet
  id, max packet id, reference frame id, and optional playout-delay
  extensions. The exact layout comes from openscreen (`rtp_defines`,
  `rtp_packetizer`).
- Each frame is encrypted as a whole with AES-128-CTR. The nonce comes from
  `aesIvMask` and the frame id.
- RTCP on the same socket:
  - We send Sender Reports (RTP↔NTP mapping for A/V sync).
  - We parse Receiver Reports, Cast feedback (checkpoint ACK + NACK bitmasks
    + target-delay changes), and PLI.
- Retransmission: keep a per-stream packet history for the target-delay
  window, resend NACKed packets, and stop sending frames when too many are
  un-ACKed (openscreen's "max frames in flight").
- PLI: mark the sink as wanting a keyframe and skip forward to the next IDR
  from ffmpeg.

### 8.4 Test receivers

- openscreen's `cast_receiver` (standalone) runs on Linux and macOS. It needs
  a self-generated developer certificate, which is fine because we don't
  verify. That gives a hardware-free test loop.
- Real devices: whatever Cast-capable hardware we end up with. **Open
  question**: whether Cast built into TVs (LG from 2024, Samsung updates in
  2026) supports the mirroring receiver app. If not, a Google TV Streamer or a
  Chromecast is needed.

---

## 9. Hand-rolled components and their oracles

| Component | Oracle / test source |
|---|---|
| AES block, CTR, CBC | FIPS-197 vectors, NIST SP 800-38A vectors |
| bigint modpow | cross-checked against `python3 -c pow(a,b,m)` generated vectors |
| SRP-6a | RFC 5054 appendix B vectors (adapted to SHA-512/3072), plus a pyatv-generated transcript |
| TLV8 | HAP spec examples, round-trip fuzz |
| bplist | plists produced by `plutil`/Python `plistlib` as fixtures |
| JSON | round-trip and known-message fixtures |
| CastMessage protobuf | bytes generated with `protoc --encode` from `cast_channel.proto` |
| DNS/mDNS | captured responses (tcpdump/Wireshark) from the real TVs as fixtures |
| Matroska demux | ffmpeg-generated files and live pipes; compare against `ffprobe -show_packets` |
| H.264 SPS parse | ffmpeg-generated streams across profiles and sizes; compare to `ffprobe` |
| FairPlay | spec test vectors + black-box oracle (§10) |
| Cast RTP/RTCP | openscreen unit-test byte fixtures; openscreen standalone receiver |
| AirPlay mirror | doubletake-test-receiver as a black-box binary; real Apple TV |

---

## 10. Licensing and the FairPlay clean-room

### Licensing stance

- Intended license for `tvpipe`: **Apache-2.0** (matching `argument`),
  provided every line is written clean or ported from permissive sources.
- **Fallback**: if the clean-room FairPlay stalls, port doubletake's FairPlay
  module into `src/airplay/fairplay/` under LGPL-3.0-or-later. The distributed
  binary is then under LGPL terms. That is acceptable for now. The module
  boundary in §7.3 makes it a drop-in swap later.

### Source classification

| Class | Sources | Who may read them |
|---|---|---|
| **Permissive**: may read and port with attribution | openscreen (BSD-3), pyatv (MIT), pychromecast (MIT), node-castv2 (MIT), rust_cast (MIT), public RFCs and specs | everyone |
| **Copyleft / tainted**: dirty room only | doubletake (LGPL-3.0+), UxPlay and RPiPlay (GPL-3.0), playfair (GPL), airfry (MIT, but a port of doubletake), RottingApple (MIT, but bundles GPL playfair and a doubletake-derived helper), shairport-sync (check its license first) | spec writers only |
| **Black-box** | running doubletake, its test receiver, UxPlay, or an oracle harness as external processes | everyone (behaviour, not source) |

### Clean-room procedure for FairPlay (and AirPlay mirroring specifics)

1. **Dirty room (spec writer)** reads doubletake and playfair and writes
   `docs/spec/fairplay.md` and `docs/spec/airplay-mirroring.md`:
   - Message formats, field offsets, data flow, and algorithm *structure*, in
     prose and math (for example "9-round AES-like network: per-round byte
     substitution from table S[r][i], 4×4 byte mixing via tables M[i][j],
     XOR with round mask K[r]").
   - **No code, and no line-by-line pseudocode transliteration.**
   - Constants and tables go in `docs/spec/data/*.hex` in a **canonical
     expanded form** (for example full 256-byte substitution tables dumped by
     evaluating them), not doubletake's factored representation.
   - Test vectors (inputs and outputs, for every primitive and for full
     m1/m2/m3/m4/ekey exchanges) in `tests/vectors/fairplay/*.txt`.
2. **Oracle harness**: a small Go program, kept *outside* this repo (for
   example `~/.cache/streamlib-oracle`), that links doubletake's package and
   exposes each primitive on stdin/stdout (`sap-hash HEX`, `msg-encrypt MODE
   HEX`, `m3 LOCAL_SAP M2`, `wrap-key …`). Clean implementers may *run* it for
   differential fuzzing, but may not read it.
3. **Review**: a separate reviewer reads the spec against the source and
   strikes anything that is copied expression rather than function or fact.
4. **Clean room (implementer)** has no access to the tainted sources: run it
   in a sandbox without `~/.cache/checkouts` or network access to those repos.
   It implements from the spec, vectors, and oracle only.
5. **Provenance**: every commit in `fairplay/` and `airplay/mirror.rs` states
   which spec sections and vectors it was built from.

Honest caveats (this is not legal advice):

- The substitution tables, IVs, and constants are functional data that
  ultimately come from Apple's binaries. No clean room changes that; we only
  avoid copying *doubletake's* expression.
- LLM agents may have seen UxPlay/playfair (old and widely mirrored) or
  doubletake during training. Clean agents must not be prompted with the
  tainted code, and provenance notes are our evidence of process.
- The fully independent route would be to reverse Apple's sender code (macOS
  AirPlay/CoreUtils frameworks) directly with Ghidra or emulation, which is
  how this was originally cracked. That is much harder and is kept as a last
  resort.

---

## 11. Testing strategy

1. **Unit / known-answer tests** for every hand-rolled piece (§9). They must
   pass on all three OSes in CI.
2. **Fixture tests** from packet captures of real devices (mDNS responses,
   `/info` plists, OFFER/ANSWER JSON). Captures go into `tests/fixtures/`, with
   identifiers scrubbed.
3. **Local receiver loops** (no hardware):
   - AirPlay: `doubletake-test-receiver --profile modern|lg|appletv3` and
     UxPlay, run as external binaries in scripts under `scripts/receivers/`.
   - Cast: openscreen `cast_receiver`.
   - These run in an opt-in integration suite (`cargo test -- --ignored`, or a
     `scripts/it.sh`).
4. **Hardware matrix** (human in the loop):

   | Device | Protocols | Status |
   |---|---|---|
   | Apple TV (model/tvOS TBD) | AirPlay 2 | available |
   | LG TV (year TBD) | AirPlay 2, Cast if 2024+ and updated | available |
   | Samsung TV (year TBD) | AirPlay 2, Cast if 2023–2025 with 2026 update | available |
   | Google TV Streamer / Chromecast | Cast | to acquire if TVs lack Cast mirroring |

5. **Latency measurement**: `tvpipe test-pattern` burns a millisecond
   wallclock and a frame counter into the video. Photograph the sender's screen
   and the TV together (or point a phone at both). Record the results in
   `docs/measurements.md` per device and setting.

---

## 12. Milestones

Each milestone has an exit criterion that a human can verify.

| # | Milestone | Exit criterion |
|---|---|---|
| M0 | Scaffold: crate, CLI skeleton, core types (§3), log, CI on mac/linux/windows | `tvpipe --help` works; CI green; types reviewed and frozen |
| M1 | Foundations: AES/CTR/CBC, bigint, SRP, TLV8, bplist, JSON, protobuf, DNS codec | all KATs pass on 3 OSes |
| M2 | Discovery | `tvpipe discover` lists Apple TV, LG, Samsung, and any Cast device, merged per device |
| M3 | Media pipeline | `tvpipe debug-pipeline FILE` prints frames with pts/keyframe; ffmpeg→demux overhead measured; game SLRW input works with a sample generator |
| M4 | AirPlay pairing + secure channel | `tvpipe pair "Apple TV"` with PIN; reconnect with pair-verify; encrypted `/info` round-trip |
| M5 | FairPlay (clean-room) | all spec vectors pass; byte-identical against the oracle under fuzzing; real Apple TV accepts m3/m4 |
| M6 | AirPlay mirror video | test pattern and a file visibly mirror on the Apple TV; latency measured |
| M7 | AirPlay audio + sync | ALAC audio in sync (±40 ms) on the Apple TV; Samsung and LG tried, quirks documented |
| M8 | Cast control + Streaming video | video plays on the openscreen receiver, then on real Cast hardware |
| M9 | Cast audio, RTCP feedback, retransmit | audio+video in sync; survives 2% injected loss |
| M10 | Replication + robustness | one encode to Apple TV + Cast device at once; clean Ctrl-C teardown; reconnect; Windows/Linux/macOS verified |
| M11 | Polish | hardware encoder selection, latency tuning, docs, release builds |

Critical path: M0 → M1 (SRP, AES) → M4 → M5 → M6 → M7. Cast (M8–M9) depends
only on M0–M3 and can run in parallel with M4–M7.

---

## 13. Risks and open questions

| Risk | Impact | Mitigation |
|---|---|---|
| Clean-room FairPlay takes long or fails | blocks AirPlay | LGPL port fallback behind the same module API |
| Apple changes FairPlay/pairing in a tvOS update | AirPlay breaks | keep vectors and an oracle; follow doubletake/UxPlay issue trackers |
| LG AirPlay quirks (not in doubletake's tested list) | LG may not work | use the test-receiver `lg` profile early; capture traffic from an iPhone→LG session if needed |
| TV-integrated Cast lacks the mirroring receiver | no Cast hardware to test | buy a Google TV Streamer |
| AAC-ELD-only receivers | no audio without libfdk | detect it, warn, allow `--no-audio`; possibly add an ffmpeg build hint |
| No on-demand keyframes with an ffmpeg subprocess | slower recovery after loss | short GOP; in-process encoder later |
| Hand-rolled AES speed | CPU at high bitrates | table-based AES at ≥200 MB/s is plenty for ≤50 Mbit/s; benchmark in M1 |
| `ring` needs a C toolchain | build friction on Windows | document it; `ring` ships prebuilt asm for the main targets |
| mDNS legacy unicast ignored by some responder | device not found | fall back to binding 5353 with `SO_REUSEADDR` (needs `socket2` or a little unsafe code; decide then) and to `--to ADDR` |

| ffmpeg as a hard runtime dependency | install friction; encode latency; no on-demand IDR | passthrough for compatible inputs, engine-encoded SLRW, optional hand-rolled encoders: see "Inputs" in `ARCHITECTURE.md` |

Open questions for the human:

1. Apple TV model and tvOS version; LG and Samsung model years.
2. Is buying a Cast dongle OK if the TVs don't do Cast mirroring?
3. Default latency target: as low as each receiver allows (≈100–150 ms), or a
   safer 250 ms like doubletake?
4. Confirm the Apache-2.0 target license (with LGPL as the interim fallback).

---

## 14. References

- doubletake — https://github.com/omarroth/doubletake (LGPL-3.0+, **dirty room only**)
- airfry — https://github.com/nored/airfry (port of doubletake, **dirty room only**)
- RottingApple — https://github.com/uber6/RottingApple (bundles GPL code, **dirty room only**)
- UxPlay — https://github.com/FDH2/UxPlay (GPL-3.0, receiver; **dirty room only** / black-box)
- pyatv — https://github.com/postlund/pyatv (MIT; pairing, AirPlay 2 control, protocol docs)
- openscreen — https://chromium.googlesource.com/openscreen (BSD-3; Cast channel + Cast Streaming)
- pychromecast / node-castv2 / rust_cast (MIT; Cast v2 control channel)
- RFC 6762 / 6763 (mDNS / DNS-SD), RFC 5054 (SRP), RFC 3550 (RTP), FIPS-197 (AES), SP 800-38A
- Unofficial AirPlay 2 notes (e.g. emanuelecozzi.net "AirPlay 2 internals", openairplay spec)
