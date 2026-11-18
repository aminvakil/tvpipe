# Architecture

This document explains how `tvpipe` works and why it works that way.

## Scope

`tvpipe` is a **sender** only. It pushes video and audio to receivers over
their **real-time mirroring** protocols: AirPlay screen mirroring, and Cast
Streaming (the Chromecast mirroring app, RTP/RTCP over UDP). It does not use
URL playback, because that path cannot reach low latency.

One more receiver lives on the same machine: `--to browser` serves a
player page on 127.0.0.1 that decodes the stream with WebCodecs and plays
it in a browser tab, on the same timeline as the TVs (see
[Browser Player](#browser-player)).

## One Code Path for Every OS

`tvpipe` uses only `std::net`, `std::process` and `std::fs`. It uses no OS
media frameworks, no discovery services and no interface APIs.

- **Capture** is delegated to ffmpeg.
- **Discovery** uses our own mDNS client.
- **Encoding** is done by ffmpeg or by our own encoders.

As a result, the binary behaves the same on macOS, Linux and Windows and should enable a consistent experience across all of them.

Formats and protocols are written by hand and tested against oracles. Only
the hard cryptography comes from `ring`, and TLS from `rustls`.

## Session Lifecycle

A session runs in five steps:

1. **Probe the input.** MP4/MOV/MKV files are probed with our own demuxers,
   anything else with `ffprobe`.
2. **Connect every receiver before any input starts.** AirPlay runs as far
   as the audio SETUP, and Cast through OFFER/ANSWER. At that point every
   receiver's capabilities are final.
3. **Choose the input** (see [Inputs](#inputs)). If an encode is needed,
   plan it from the minimum size, frame rate and bitrate across all
   receivers, with one audio plan per family.
4. **Start the sinks, then pull the input.** Live sources wait until every
   sink is ready, so the first frames are not stale.
5. **Stream, then tear down.** Teardown happens on EOF (after one playout
   delay), on Ctrl-C, or when every receiver has left. Every sink sends
   TEARDOWN or STOP.

A device that advertises both protocols goes over Cast unless the user
writes `airplay:`.

## Inputs

**Passthrough.** A file is sent unchanged, without ffmpeg, when it meets all
of these:

- H.264,
- no B-frames,
- a keyframe at least every 4 s,
- a size the receivers accept, and
- compatible audio.

B-frames are refused because Cast's in-flight window breaks on frames that
go back in time. The openscreen receiver only tolerates frames stamped with
decode time because it maps pictures FIFO. Long GOPs are refused because a
single drop costs a whole GOP. `-v` logs why a file did not pass.

**ffmpeg** handles everything else. It runs as a subprocess that writes
**live Matroska** to a pipe, which we demux ourselves.

- Matroska carries every codec we need and stores H.264 as AVCC.
- Flags: `-bf 0`, a fixed GOP, and low-latency flags for each encoder.
- The GOP is 1 s, or 0.5 s when an AirPlay receiver is present.
- A subprocess cannot produce keyframes on demand, so the short GOP bounds
  how long recovery takes.
- `-readrate_initial_burst 0.001` stops ffmpeg's start-up burst from
  overrunning Cast's window.
- Arguments after `--` go straight to ffmpeg, which is how capture works
  without platform code in `tvpipe`.

**Built-in encoders.** These are used for the test pattern and raw game
frames, with `--encoder builtin` or when ffmpeg is missing.

- The H.264 encoder is Constrained Baseline:
  - CAVLC entropy coding,
  - I and P pictures,
  - rate-distortion mode decisions,
  - deblocking and VBV rate control,
  - one slice per thread.
- Its purpose is **keyframes on demand** with one picture of latency, which
  no subprocess can offer.
- The AAC-LC encoder lets PCM reach Cast receivers without ffmpeg.
- Both were written from the ITU/ISO specs. Copyleft encoders (x264,
  ffmpeg, FAAC, fdk-aac) were deliberately not read. ffmpeg is used only as
  a black-box decoder in tests.

**Game input (SLRW)** is the format a game writes to `tvpipe`'s stdin: the
magic `"SLRW"`, then records of `u8 kind | u64le pts_us | u32le len |
payload`.

- **Raw frames** go to the built-in encoder, or to ffmpeg through two
  localhost TCP listeners. That works the same on every OS, with no fd
  passing and no named pipes.
- **Encoded H.264** is for engines with their own hardware encoder. When a
  receiver needs an IDR, `tvpipe` writes `keyframe\n` to its own stdout,
  which the game already owns. No port is needed. Requests are coalesced
  and rate-limited.

## Timing and Latency

- **A media clock** maps presentation timestamps to wall-clock instants,
  anchored on the first frame. The anchor only moves earlier, never later. Sinks re-anchor when frames are
  more than 100 ms late.
- **AirPlay PTP** runs without a PTP stack. The receiver's
  `X-Apple-RequestReceivedTimestamp` response header is on its PTP
  timeline. Every `/feedback` response (every 2 s) re-anchors our mapping.
  Legacy receivers get an NTP responder instead.
- **Cast** derives RTP timestamps from the media clock. Sender Reports map
  RTP time to NTP time for A/V sync.
- **Defaults:**
  - Cast: a 200 ms target delay.
  - AirPlay: leads of 100 ms (video) and 170 ms (audio). The spec's 85 ms
    audio lead glitched on the Apple TV.
  - `--latency` overrides both.

Measured glass to glass: ~250 ms on an Apple TV 4K over Wi-Fi.

## Browser Player

`--to browser` (and `tvpipe preview`) is a sink like the others. It
serves a page on 127.0.0.1 behind a random token and a loopback `Host`
check, and sends each tab the decoder configuration, then one SLRW record
per WebSocket message: H.264 access units and PCM, each stamped with the
frame's instant on the session clock. The page learns tvpipe's clock by
ping/pong, decodes with `VideoDecoder`, and presents video and audio
(Web Audio) one playout delay after that instant, like the TVs. Each tab
has its own queue and writer thread, so a slow tab only drops its own
frames. Protocol: `docs/player-protocol.md`.

## Fanout

Each sink gets a bounded queue and its own thread, so a slow receiver never
stalls the others. The rules:

- New sinks start by waiting for a keyframe.
- When a queue overflows, the frame is dropped. If it was video, the sink
  waits for the next keyframe again.
- When a sink needs a keyframe (after a PLI, loss, or a full writer), it
  drops non-key video until the next IDR and forwards the request to the
  input.
- A failed sink is removed and the others continue.

## Discovery

`tvpipe` uses **legacy-unicast mDNS** (RFC 6762 §6.7): queries go to
`224.0.0.251:5353` from an ephemeral port, and responders reply by unicast.
We never bind 5353, never clash with mDNSResponder, Avahi or Windows, and
need no OS service.

To find the local address without interface APIs, we connect a UDP socket
(which sends nothing) and read back the address the OS picked. AirPlay, RAOP and Cast records of the
same TV are merged into one device.

## AirPlay

The sender is built from a clean-room spec, plus pyatv (MIT) for pairing.

- **Pairing:**
  - HAP PIN pair-setup uses SRP-6a (3072-bit group, SHA-512).
  - Transient pair-setup uses PIN 3939 for open receivers.
  - Pair-verify uses X25519 and Ed25519.
  - Afterwards the control channel is ChaCha20-Poly1305 framed. It
    implements `Read + Write`, so the RTSP code runs on it unchanged.
  - Credentials live in a small text file.
- **FairPlay** is an **LGPL port of doubletake**, isolated behind a tiny
  API so it can be replaced later.
- **Session setup** runs everything up to the audio SETUP while connecting,
  so the canvas is known before encoding starts. The video SETUP happens at
  stream start. The Apple TV tolerates the gap between them.
- **Video** goes over TCP: 128-byte headers, a codec packet, encrypted
  frames, and heartbeats.
  - A writer thread owns the socket, so `push` never blocks.
  - HAP sessions encrypt with ChaCha20-Poly1305.
  - Legacy sessions use AES-CTR from the FairPlay key.
- **Audio** is *verbatim ALAC*: PCM wrapped in ALAC frames, 44.1 kHz, 352
  samples per frame, sent as encrypted RTP. Sync packets and retransmits
  are handled. No audio encoder is needed.
- **Receiver quirks** are handled in one place: PTP falling back to NTP,
  audio format, latency leads, and initial volume.

## Cast

The sender is ported from Chromium's **openscreen** (BSD-3), with
attribution.

- **Control.**
  - TLS on port 8009 through rustls. The verifier accepts self-signed device
    certificates but still checks the handshake signature. Device auth is
    skipped.
  - Messages are protobuf-framed `CastMessage`s with a heartbeat. We
    advertise protocol versions 0 and 3, as real Google TVs require.
- **Session.**
  - LAUNCH the mirroring app `0F5096E8`, then OFFER/ANSWER.
  - The receiver only answers an OFFER, so the OFFER is built from user
    options **before** the encode is planned. The offer only caps size and
    bitrate, so a smaller final plan is fine.
- **Media.**
  - Each frame is converted to Annex B, encrypted whole with AES-128-CTR,
    and packetized as Cast RTP.
  - Opus uses 10 ms frames. AAC is sent ADTS-framed, as Chromium does.
- **Feedback** mirrors openscreen's sender:
  - ACK/NACK handling,
  - an in-flight limit derived from RTT,
  - kickstarts,
  - PLI handling.

  The feedback logic does no I/O and takes time as an argument, so it can
  be tested deterministically.

## Ctrl-C

Ctrl-C must send TEARDOWN or STOP, or the TV stays in mirroring mode. std
cannot install a signal handler, and ffmpeg dies from the same signal.
Ctrl-C handling is therefore the one FFI exception, and it uses the C
runtime std already links:

- **Unix:** `signal()` with a handler that only sets an atomic flag.
- **Windows:** `SetConsoleCtrlHandler`.

A second Ctrl-C exits immediately. `q` then Enter also stops, using std
only.

## Licensing and Clean Room

- **Licenses.** `tvpipe` is Apache-2.0, except the FairPlay module
  (LGPL-3.0-or-later). While that module is included, the binary is
  distributed under LGPL terms.
- **Tainted sources.** doubletake, UxPlay, playfair and their derivatives
  are read only by a dirty-room spec writer and by the FairPlay porter.
- **AirPlay mirroring** was implemented from a reviewed prose spec plus test
  vectors.
- **Permissive references** (openscreen, pyatv, pychromecast, RFCs, ITU/ISO
  specs) are ported with attribution.
- **Provenance.** Protocol commits carry a `Sources:` line.

## Testing

- **Known-answer tests** cover every hand-written format and algorithm.
  The vectors come from FIPS/NIST, RFCs, Python, `plutil`, `protoc`,
  `ffprobe`, and captures from real devices (with device identifiers
  scrubbed).
- **Fuzz smoke tests** check that decoders never panic.
- **Oracle tests** require ffmpeg to decode the H.264 encoder's output
  bit-exactly to the encoder's own reconstruction. They are skipped without
  ffmpeg.
- **Fake receivers** run in process for Cast (with injected loss), AirPlay
  mirroring and pairing. Full `play` sessions run against them.
- **Opt-in tests** against openscreen's `cast_receiver` and the Apple TV run
  with `--ignored` and `TVPIPE_*` variables.
- **CI** runs fmt, clippy and the tests on Linux, macOS and Windows.

## Known Gaps

- **Untested hardware:** LG and Samsung AirPlay, and the legacy AirPlay
  pairing and media profile.
- **Cast:** testing on real hardware is in progress.
- **Not implemented:** HEVC, AAC-ELD, adaptive bitrate, and IPv6
  discovery.
- **B-frames** on real receivers are still untested.
