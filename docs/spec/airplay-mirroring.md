# AirPlay screen mirroring: sender functional spec

Status: draft by the S2 spec writer (dirty room), 2026-11-18, awaiting
review. Audience: the clean implementer of `airplay/{mirror,audio,timing,
sink}.rs` (W7). Everything here is a wire fact or a behavioural rule; there
is no reference code. Constants are collected in
`docs/spec/data/airplay-mirroring.txt`, byte-exact examples in
`tests/vectors/airplay-mirror/*.txt` (index in §12).

Confidence tags used below:

- **[HW]** observed on the dev Apple TV 4K (AppleTV6,2, tvOS 26.6.1,
  `AirTunes/960.13.1`, "Livingroom (2)") on 2026-11-18 with a HAP pairing
  made by streamlib (`X-Apple-HKP: 3`), FairPlay, PTP and ALAC audio. The
  session was accepted end to end (all requests 200, data channel open,
  receiver audio statistics reported); **nobody looked at the screen**, so
  "picture/sound actually rendered" is not yet confirmed (§13).
- **[REF]** what the reference sender does and what its authors report as
  working on real receivers; not re-verified here.
- **[MODEL]** behaviour of a receiver family as modelled by the reference's
  test receiver; treat as plausible, not proven.

Notation: `u32le` = 32-bit little-endian unsigned, `u64be` = 64-bit
big-endian, and so on. Offsets are 0-based byte offsets. "bplist" means a
binary property list (`bplist00`, `codec::bplist`). bplist value types are
written `int`, `real`, `bool`, `string`, `data`, `dict`, `array`. A bplist
`int` is a signed 64-bit value on the wire; where a field is really an
unsigned 64-bit quantity (clock ids, seeds, bit masks) it may arrive or must
be sent as a negative `int` and is reinterpreted bit-for-bit.

### Minimal path for the dev Apple TV

The smallest implementation that the dev Apple TV accepted [HW]: HAP
pair-verify (existing), FairPlay, control-first SETUP with PTP (§4.1,
§7.2), event channel answering `200` to everything (§9), `GET /info` for
the canvas, early RECORD, audio SETUP in layout S with a random ChaCha key
and no MediaDataControl seed, video SETUP, then: codec packet + ChaCha
video frames + 1 s heartbeats (§6), `/feedback` every 2 s, ALAC (verbatim
is enough) in PT 96 ChaCha packets sent once each (§8.3, §8.4), a sync
packet at start and every second (§8.5), retransmit answers (§8.6), and
TEARDOWN. Everything else (NTP, legacy profile, media-first ordering,
RFC 2198, volume policy, MediaDataControl, HEVC) is for other receivers or
robustness.

---

## 1. Overview

A mirroring session uses these transports, all to the address of the
receiver the control connection is connected to (use the peer address of
that TCP socket, not a re-resolved name):

| Transport | Direction | Purpose |
|---|---|---|
| Control TCP (port 7000, `airplay::pair::connect`) | sender → receiver | RTSP requests; encrypted after HAP pair-verify (`airplay::secure`) |
| Event TCP (receiver's `eventPort`) | sender connects | receiver → sender RTSP commands (§9); encrypted with its own keys under HAP |
| Video data TCP (receiver's video `dataPort`) | sender connects | 128-byte-header video packets (§6) |
| Audio data UDP | sender's data socket → receiver's RTP port | RTP audio (§8) |
| Audio control UDP | sender's control socket ↔ receiver's RTCP port | sync packets out, retransmit requests in (§8.5, §8.6) |
| Timing UDP (NTP mode only) | receiver → sender's timing port | NTP-style timing requests answered by the sender (§7.3) |
| MediaDataControl TCP (optional) | sender connects | receiver audio statistics, dynamic latency (§8.10) |

Session sequence (control-first ordering; §4 has every message):

1. Connect, `GET /info`, pair-verify (or transient pair-setup), switch to
   the encrypted channel — already implemented by W5.
2. FairPlay SAP (`POST /fp-setup` ×2) when feature 14 is set (§2.2).
3. `SETUP` #1: the session, no streams. Learn `eventPort`, the PTP clock,
   and the timing anchor.
4. Connect the event channel.
5. `GET /info` again: now it contains `displays` (§2.6).
6. `RECORD` (early; deferred if refused with 455).
7. `SETUP` #2: the audio stream (type 96). Learn the audio ports.
8. `SETUP` #3: the video stream (type 110). Learn the video `dataPort`.
9. Connect the video data channel.
10. Optional initial-volume handling (`GET_PARAMETER`/`SET_PARAMETER`).
11. `RECORD` if still pending.
12. Streaming: codec packet + IDR, then frames; audio starts after the
    first video frame; heartbeats every 1 s; `POST /feedback` every 2 s;
    audio sync every 1 s.
13. `TEARDOWN`, close the media sockets.

---

## 2. Prerequisites and capability selection

### 2.1 Pairing and the two crypto profiles

The pairing outcome selects one of two profiles for everything after it:

- **HAP profile**: pair-verify was the HomeKit-style TLV8 exchange and the
  control connection is now ChaCha20-Poly1305 framed (what streamlib does
  today). Video uses ChaCha20-Poly1305 keyed from the pair-verify shared
  secret, audio uses ChaCha20-Poly1305 with a random key sent in SETUP, the
  event channel is encrypted, PTP timing is available. **This is the Apple
  TV profile [HW].**
- **Legacy profile**: the original raw (non-TLV) pair-verify, after which
  the control connection stays plaintext. Video uses AES-128-CTR, audio
  AES-128-CBC, both keyed from FairPlay; timing is NTP. Third-party
  receivers and old Apple TVs. The raw pair-setup/verify formats are in
  Appendix A.

Let `S` be the 32-byte X25519 shared secret of the pair-verify that keyed
the current control connection (`Control::shared`).

[HW] Pair-verify must use the same `X-Apple-HKP` value family that the
pairing was created with. A streamlib pairing (created with `X-Apple-HKP: 3`)
verifies with only `X-Apple-HKP: 3`; the same keys presented with
`X-Apple-HKP: 5` (the screen-capture pairing type) plus
`X-Apple-Client-Name`, `X-Apple-Client-ID` and `X-Apple-PD: 1` fail with
TLV error value 7 in M4. A pairing made with type 3 and no extra
access-control entry is sufficient for a mirroring session on the Apple TV
[HW].

### 2.2 FairPlay SAP and what mirroring takes from it

FairPlay is done by `airplay::fairplay` (LGPL port, decision 0005); this
spec only uses its outputs. Run it after pair-verify, on the (encrypted)
control connection, when `/info` feature bit 14 is set. FairPlay counts as
unsupported, and the session continues without it, only when feature 14 is
absent or either `/fp-setup` request is answered 404. Every other failure
(any other non-2xx status, an m2/m4 of the wrong length or with a bad
header, `check_m4` rejecting m4, a transport error) aborts the session
setup [REF].

Wire: two `POST /fp-setup` requests, each with headers `X-Apple-ET: 32` and
`Content-Type: application/octet-stream`; bodies m1 (16 bytes) → response
m2 (142 bytes), then m3 (164 bytes) → response m4 (32 bytes) [HW].

After a successful exchange the sender creates:

- `K_fp`: 16 random bytes, the "FairPlay key".
- `ekey`: `wrap_key(K_fp)`, 72 bytes.
- `eiv`: 16 random bytes (an IV; not wrapped).
- The **stream master key** `K = SHA-512(K_fp ‖ S)[0..16]` when the
  pair-verify was HAP or was a raw pair-verify that carried
  `X-Apple-PD: 1` (the reference always sends that header); otherwise
  `K = K_fp`. Vector: `keys.txt` "stream master key".

How they are used:

| Value | HAP profile | Legacy profile |
|---|---|---|
| `ekey`, `eiv`, `et = 32` in the session SETUP | not sent | sent (§4.1) |
| video `shk` / `shiv` | `K` / `eiv` (sent, not used for the ChaCha video key, §5.1) | `K` / `eiv` (CTR key and IV are derived from `shk`, §5.2) |
| audio key | random 32-byte ChaCha key (not FairPlay) | AES-CBC key `K`, IV `eiv` |

[HW] With the HAP profile the Apple TV also accepted the whole session with
FairPlay skipped (shk/shiv then filled as in §5.3), but it closed the
control connection instead of answering `TEARDOWN`; whether it renders
video without FairPlay is unknown. Do FairPlay whenever feature 14 is set.

### 2.3 Feature bits

`features` from `/info` (64 bits; bits ≥ 64 come from `featuresEx` and are
not needed). Bit numbers (also in the data file):

| Bit | Meaning for mirroring |
|---|---|
| 7 | screen mirroring supported |
| 14 | FairPlay SAP supported |
| 18, 19, 20, 21 | audio PCM / ALAC / AAC-LC / AAC-ELD 44.1 kHz stereo (legacy capability, used only when `supportedFormats` is absent, §2.4) |
| 26, 51 | third-party (non-Apple) receiver |
| 28 | default screen 1920×1080 when no `displays` (else 1280×720) |
| 32 | volume control (allows querying the initial volume, §8.9) |
| 38, 43, 46, 48 | CoreUtils/HAP pairing (any of them) |
| 41 | PTP timing |
| 42 | multi-codec screen (HEVC allowed, §6.7) |
| 49 | third-party TV family (volume policy, §8.9) |
| 59 | audio descriptor uses `streamConnections` (§4.6) |
| 60 | audio MediaDataControl channel (§8.10) |
| 61 | RFC 2198 audio redundancy (§8.4) |

Dev Apple TV: `features = 0x3c177fde4a7fdfd5` (bits 0 2 4 6–12 14–22 25 27
30 33–36 38–46 48–50 52 58–61) [HW].

### 2.4 Audio format

`/info` → `supportedFormats` (dict) → `screenStream` (int bit mask; may be
negative, reinterpret as u64). Bits: `0x800` PCM 44.1k/16/2, `0x40000` ALAC
44.1k/16/2, `0x400000` AAC-LC, `0x1000000` AAC-ELD. If the whole
`supportedFormats` dict is absent, build the mask from feature bits 18–21
(PCM→0x800, ALAC→0x40000, AAC-LC→0x400000, AAC-ELD→0x1000000). A present
but zero mask means "unknown": use ALAC.

Selection for streamlib: ALAC if bit `0x40000` is set; else AAC-ELD if
`0x1000000` is set and an AAC-ELD encoder is available; else fail with a
clear error ("receiver only offers AAC-ELD"). The dev Apple TV sends
`screenStream = 0x1440800` (PCM, ALAC, AAC-LC, AAC-ELD) [HW]. The reference
prefers AAC-ELD when it has an encoder; ALAC is equally accepted.

A video-only session still sends the audio stream SETUP (with the ALAC
descriptor as a placeholder) and simply never sends audio packets [REF].

### 2.5 Timing protocol

Use **PTP** iff all of: HAP profile; feature 41; and `sourceVersion`
(`/info` key; if absent, the version after `AirTunes/` in the `Server`
response header) ≥ 354.54.6. Otherwise use **NTP**. Version compare: split
on `.`, 2 or 3 numeric parts (missing patch = 0), compare numerically;
unparsable = "too old".

If PTP fails during setup (§7.2: no usable clock id/anchor, or a media-first
receiver rejects the PTP session SETUP with 400/406/415/455/501), tear the
partial session down, close the control connection, reconnect, redo
pair-verify and FairPlay, and run the whole setup once more with NTP [REF].

[HW] The Apple TV accepts both: with PTP it returns a clock id and anchor
headers; with NTP it probes the sender's timing port (§7.3). PTP needs no
inbound UDP on the sender, NTP does (a host firewall that drops the probes
makes SETUP #1 hang). Prefer PTP.

### 2.6 Display size (the receiver's canvas)

The initial `/info` of current Apple TVs has no `displays` [HW] (see
`tests/fixtures/airplay/`). The display list appears only once a media
session exists. Sources, in order of preference:

1. The `info` dict inside the SETUP #1 response, if the request carried
   `combinedGetInfoWithControlSetup = true` and the receiver honours it. The
   dev Apple TV does **not** return it [HW]; other receivers may.
2. A new `GET /info` sent on the control connection right after SETUP #1
   (media-first ordering: after the audio SETUP). [HW] returns
   `displays`.
3. The event-channel command `updateInfo` (§9) carries the same dict [HW].

`displays` is an array of dicts; use the first. Keys [HW]: `width`,
`height` (ints, logical size), `widthPixels`, `heightPixels` (ints),
`widthPixelsMax`, `heightPixelsMax` (decoder maximum), `maxFPS` (int),
`uuid` (string), `overscanned`, `rotation`, `receiverSupports444` (bools),
`widthPhysical`, `heightPhysical`, `edid` (data), `HDRInfo` (dict with
`HDRMode`, e.g. `"DoVi"`). Dev Apple TV after SETUP: 1280×720 nominal,
3840×2160 max, maxFPS 60 [HW] (its HDMI mode was not checked).

Size rules (reference policy):

- Nominal size = `widthPixels`×`heightPixels`, else `width`×`height`;
  without `displays`: 1920×1080 if features 7 and 28, 1280×720 if only 7.
- Maximum = `widthPixelsMax`×`heightPixelsMax`, else nominal.
- H.264 canvas = nominal, except: if the maximum is ≤ 1920×1080 in both
  dimensions, use the component-wise larger of nominal and maximum.
  (Dev Apple TV: max 3840×2160 > 1080p, so the H.264 canvas stays
  1280×720.) A user-requested size (both width and height given) that fits within the
canvas in both dimensions is used as is (no upscaling, smaller is fine);
otherwise it is scaled by `min(canvas_w / w, canvas_h / h)` with each
result rounded to the nearest integer (halves away from zero). Then, in
every case, round both dimensions down to even. Without a request the
canvas itself is used (rounded down to even).
- The encoded size is announced only in the codec packet (§6.2); [HW] a
  1920×1080 stream was accepted while the canvas said 1280×720 (rendering
  not confirmed).

### 2.7 Latency profile

The sender picks one pair of playout leads for the session:

| Profile | Video `latencyMs` | Audio lead | Audio `latencyMax` (samples) |
|---|---|---|---|
| normal (wired/unknown route) | 75 ms | 85 ms | 3748 |
| wireless route | 100 ms | 170 ms | 7497 |
| explicit (user) | L | L | floor(L × 44.1) |

The reference picks "wireless" when the control connection's local
interface is Wi-Fi (an OS query); streamlib cannot ask the OS portably, so
use "normal" by default and let the user override (or choose "wireless"
when unsure; it only costs latency). Explicit values are clamped to
5–2000 ms. ms → samples always truncates (`floor(ms × 44100 / 1000)`).

---

## 3. Control-connection conventions

Request line `METHOD URI RTSP/1.0`. Headers sent by the reference on every
request, in this order:

```
CSeq: <n>                     (1, 2, 3, … per connection)
User-Agent: AirPlay/935.7.1
<request-specific headers>
Content-Type: <type>          (only when the body is non-empty)
Content-Length: <n>           (always, also 0)
```

No `X-Apple-Session-ID`, `DACP-ID` or `Active-Remote` on mirroring requests
[REF; HW accepted without them]. `Session:` only on `RECORD`.

URIs:

- Session/audio URI: `rtsp://<receiver ip>:<port>/<audio streamConnectionID>`
  (decimal). Used by SETUP #1, SETUP #2, `RECORD`, `GET_PARAMETER`,
  `SET_PARAMETER`, `TEARDOWN`.
- Video URI: `rtsp://<receiver ip>:<port>/<video streamConnectionID>`, used
  only by SETUP #3.
- `/info`, `/feedback`, `/fp-setup`: plain paths.

`streamConnectionID`: a random positive 63-bit integer, different for the
audio and the video stream (the reference uses the current time in ns with
the top bit cleared). Sent as a bplist `int`; formatted in URIs and key
salts as unsigned decimal.

Every response carries `X-Apple-RequestReceivedTimestamp` (decimal ms on
the receiver's clock) and `X-Apple-ProcessingTime` (decimal ms) [HW]; they
anchor PTP (§7.2). SETUP response bodies are bplists
(`Content-Type: application/x-apple-binary-plist`).

Requests are strictly sequential on the control connection (one
outstanding request). Observed latencies [HW]: SETUP #1 130–480 ms,
TEARDOWN ~750 ms, others < 50 ms. Response timeouts used by the reference: 3 s for the post-SETUP `GET /info`
refresh (§4.3) and the volume `GET_PARAMETER` (§4.9); 2 s for the cleanup
TEARDOWN after a failed setup (§4.14); 30 s for everything else (in NTP
mode SETUP #1 is answered only after the receiver's timing probes, so it
is slow when they are lost). Connect timeouts: control 10 s, video data
5 s, event and MediaDataControl 3 s. A timeout or transport error while
waiting for a response makes the control connection unusable (a late
response would be read as the answer to the next request): treat it as
fatal for the session, unlike a complete non-2xx response, which leaves the
connection in sync.

**HTTP Digest** (receivers with a playback password, `statusFlags` bit 7):
a request may be answered `401` with `WWW-Authenticate: Digest realm="…",
nonce="…"`. Retry the same request once with `Authorization: Digest
username="AirPlay", realm="…", nonce="…", uri="<request URI>",
response="<r>"` where `r = MD5hex(MD5hex("AirPlay:" realm ":" password) ":"
nonce ":" MD5hex(method ":" uri))` (RFC 2069, lowercase hex). Cache the
challenge and send the header up front on later requests [REF].

---

## 4. Session setup, message by message

All SETUP bodies are bplists sent with `Content-Type:
application/x-apple-binary-plist`. Full examples (bplist hex plus XML) in
`setup.txt`; real Apple TV request/response pairs in `appletv-session.txt`.

### 4.1 SETUP #1: the session

`SETUP <audio URI>`. Body dict:

| Key | Type | Value |
|---|---|---|
| `deviceID` | string | a sender id formatted as a MAC, six uppercase hex pairs with colons, first octet `02` (locally administered), e.g. `02:34:73:DA:E2:DF`. The reference takes the first 12 hex digits of a UUID generated per control connection and forces the first octet to `02`, so the value changes every run; it is not related to the pairing identity and need not match anything sent in pair-setup/verify [HW: a fresh random value per run was accepted with streamlib's stored pairing]. Deriving it from the controller id is also fine. |
| `macAddress` | string | same as `deviceID` |
| `sessionUUID` | string | random UUID v4, lowercase; reused in `RECORD`'s `Session:` header |
| `sourceVersion` | string | `980.71.1` |
| `isScreenMirroringSession` | bool | true |
| `timingProtocol` | string | `PTP` or `NTP` |
| `osBuildVersion` | string | `13F69` |
| `model` | string | `Linux` (reference value; any short string was not tested) |
| `name` | string | sender name (host name) |
| `updateSessionRequest` | bool | false |
| `combinedGetInfoWithControlSetup` | bool | true |
| NTP only: `timingPort` | int | the sender's UDP timing port (§7.3), already listening |
| PTP only: `timingPeerInfo` | dict | `{ID: string (random UUID), Addresses: [string: the control socket's local IP], DeviceType: int 0, SupportsClockPortMatchingOverride: bool true}` |
| PTP only: `timingPeerList` | array | one element, the same dict |
| Legacy profile with FairPlay only: `ekey` | data | 72 bytes |
| ... `eiv` | data | 16 bytes |
| ... `et` | int | 32 |

No `streams` key. Response dict (keys that matter):

| Key | Type | Meaning |
|---|---|---|
| `eventPort` | int | TCP port of the event channel; connect now (§9). May be absent. |
| `timingPeerInfo` | dict | PTP: `ClockID` (int, reinterpret as u64; **negative on the Apple TV** [HW]), `ID`, `Addresses`, `DeviceType` (1), `ClockPorts` (dict: our peer `ID` → int), `SupportsClockPortMatchingOverride` |
| `asyncPTPClockConfig` | bool | true: the clock id will come later as an event (§9) |
| `timingPort` | int | NTP receivers that want the sender to initiate timing (§7.3) |
| `skipRecord` | bool | true: never send `RECORD` |
| `info` | dict | combined `/info` (see §2.6) |

[HW] Apple TV response: `{eventPort, timingPeerInfo}` only.

PTP: configure the clock from this response (§7.2). If `ClockID` is missing
or 0: if `asyncPTPClockConfig` is true and the event channel is up, wait up
to 2 s for `updateTimingPeerInfo`; otherwise PTP failed (§2.5).

NTP: start sender-initiated probes if the response has `timingPort`
(§7.3).

If SETUP #1 is answered 400, 405, 406, 415, 455 or 501, the receiver wants
the **media-first** ordering (§4.12); other errors are fatal.

### 4.2 Event channel

Connect TCP to `eventPort` immediately (before waiting for anything else),
set up its encryption (§9) and keep serving it until teardown. If the
response had no `eventPort`, try again after each later SETUP response
that has one; a session without an event channel is allowed.

### 4.3 Refresh `/info`

If the SETUP #1 response had no `info` dict with a usable `displays`
entry, send `GET /info` (no body) on the control connection and parse the
bplist response (§2.6), with a 3 s timeout. A complete non-2xx answer is
not fatal (keep the old info), except 401, which is fatal for this attempt
(configure the password first). A timeout, a transport error or an
incomplete response is fatal (§3).

### 4.4 Early RECORD

Unless `skipRecord`, and unless the session is video-only, send:

```
RECORD <audio URI> RTSP/1.0
CSeq: n
User-Agent: AirPlay/935.7.1
Session: <sessionUUID>
Range: npt=0-
RTP-Info: seq=0;rtptime=0
Content-Length: 0
```

(Header order is free.) Response: optional `Audio-Latency: <samples>`
header (informational; Apple TV sends `0` [HW]). If the answer is `455`,
the receiver wants RECORD after the streams: remember that and send it in
§4.10 instead. Any other error is fatal.

### 4.5 Choose the canvas, start the encoder

With the refreshed info, pick codec and canvas (§2.6, §6.7) and start the
encoder now, so the first IDR is ready soon after SETUP #3; the
`latencyMs`/`latencyMax` values sent next must already account for any
extra encoder delay. Later display updates (§9 `updateInfo`) do not change
a running session's size.

### 4.6 SETUP #2: the audio stream

`SETUP <audio URI>`. Body: `{streams: [<audio descriptor>]}`, nothing else
(in control-first ordering the session keys are not repeated).

Audio descriptor, common keys:

| Key | Type | Value |
|---|---|---|
| `type` | int | 96 |
| `streamConnectionID` | int | the audio id (same as in the URI) |
| `ct` | int | 2 (ALAC) or 8 (AAC-ELD) |
| `spf` | int | 352 (ALAC) or 480 (AAC-ELD) |
| `sr` | int | 44100 |
| `audioFormat` | int | 0x40000 (ALAC) or 0x1000000 (AAC-ELD) |
| `audioMode` | string | `default` |
| `usingScreen` | bool | true |
| `latencyMin` | int | 0 |
| `latencyMax` | int | audio lead in samples (§2.7) |
| `redundantAudio` | int | 2 |

Then one of two layouts. **Layout S** (`streamConnections`) when feature
59 is set, else **layout P** (`controlPort`):

Layout S adds:

| Key | Type | Value |
|---|---|---|
| `isMedia` | bool | false [REF: required value for screen audio] |
| `supportsDynamicStreamID` | bool | true |
| `shk` | data | HAP profile: the 32-byte random audio key (§5.1). Omitted otherwise. |
| `streamConnections` | dict | `{streamConnectionTypeRTP: {streamConnectionKeyUseStreamEncryptionKey: bool (true iff shk present)}, streamConnectionTypeRTCP: {streamConnectionKeyPort: int (our audio control port)}}` |
| optionally inside `streamConnections` | | `streamConnectionTypeMediaDataControl: {streamConnectionKeyEncryptionSeed: int (random 64-bit seed, bit pattern)}` — only if §8.10 is implemented |

Layout P adds: `controlPort` (int, our audio control port), and `shk`
(32 bytes) in the HAP profile. Layout P does **not** send `isMedia`,
`supportsDynamicStreamID` or `streamConnections`; layout S does **not**
send the flat `controlPort`. Both send all the common keys.

If the audio SETUP is answered 400, 406, 415, 455 or 501, retry it once
with the other layout; keep whichever was accepted.

Response: `{streams: [{type: 96, …}]}`. Ports, either form:

- layout P style: `dataPort` (int, receiver RTP port), `controlPort` (int,
  receiver RTCP port);
- layout S style: `streamConnections.streamConnectionTypeRTP.streamConnectionKeyPort`
  (RTP), `…TypeRTCP.streamConnectionKeyPort` (RTCP),
  `…TypeMediaDataControl.streamConnectionKeyPort` (MediaDataControl TCP).
  These win over the flat keys when both appear.

Other keys [HW]: `streamID` (int, can be negative; not used),
`arrivalToRenderLatencyMs` (int, receiver output delay, informational; 27 on
the Apple TV). A response with no audio ports means "no audio": continue
video-only.

### 4.7 SETUP #3: the video stream

`SETUP <video URI>`. Body: `{streams: [<video descriptor>]}`:

| Key | Type | Value |
|---|---|---|
| `type` | int | 110 |
| `streamConnectionID` | int | the video id (same as in the video URI) |
| `latencyMs` | int | video lead in ms (§2.7) |
| `timestampInfo` | array | five dicts, in order: `{name: "SubSu"}`, `{name: "BePxT"}`, `{name: "AfPxT"}`, `{name: "BefEn"}`, `{name: "EmEnc"}` |
| `shk` | data | 16 bytes, §5.3 |
| `shiv` | data | 16 bytes, §5.3 |

Response: `{streams: [{type: 110, dataPort: int}]}` [HW]. No `dataPort` is
fatal. If the event channel is still not connected and this response has
`eventPort`, connect it now.

### 4.8 Video data connection

Connect TCP to `dataPort`; set `TCP_NODELAY`; a small send buffer (the
reference uses 64 KiB) keeps queuing latency down. Nothing is sent until
the first encoded frame (§6). Keep a reader on this socket: the receiver
normally sends nothing; EOF or an error means the receiver ended the
session.

### 4.9 Initial volume

See §8.9 for the policy. If `/info` has no `initialVolume` and the receiver
supports volume queries (feature 32, or `sourceVersion` ≥ 120.2.0), and the
session has audio, send

```
GET_PARAMETER <audio URI>
Content-Type: text/parameters
body: "volume\r\n"
```

and parse a line `volume: <float>` (case-insensitive name) from the body,
with a 3 s timeout. A complete non-2xx answer is not fatal (volume stays
unknown); a 2xx answer without a parsable value counts as "invalid"; a
timeout or transport error is fatal (§3).

### 4.10 Late RECORD

If RECORD is still pending (media-first ordering, video-only session, or
an early RECORD refused with 455) and not `skipRecord`, send it now (same
form as §4.4).

### 4.11 Initial SET_PARAMETER volume, then streaming

If the §8.9 policy says so, send

```
SET_PARAMETER <audio URI>
Content-Type: text/parameters
body: "volume: <v>\r\n"        (v printed with 6 decimals, e.g. "-20.000000")
```

Failure is not fatal. Then start: the feedback loop (first `POST /feedback`
immediately, then every 2 s, §4.13), the video stream (§6), the heartbeat
(§6.4), and audio once the first video frame has been sent (§8.8).

Do not send `GET_PARAMETER` keep-alives during streaming [REF]; `/feedback`
(§4.13) is the only periodic control request.

### 4.12 Media-first ordering (legacy receivers)

When SETUP #1 was refused (§4.1):

1. SETUP #2 becomes the first request and carries the session too: its
   body is the §4.1 session dict **without** `updateSessionRequest` and
   `combinedGetInfoWithControlSetup`, plus `streams: [<audio
   descriptor>]`, plus (legacy profile with FairPlay) `ekey`, `eiv`,
   `et = 32`.
2. From its response take, in addition to the audio ports, `eventPort`,
   `timingPeerInfo`/`asyncPTPClockConfig` (PTP), `timingPort` (NTP) and
   `skipRecord`, exactly as §4.1 describes for SETUP #1.
3. `GET /info` (§4.3), then SETUP #3 (§4.7, body unchanged: stream only),
   then the data connection, §4.9, RECORD (§4.10), §4.11.

Later stream SETUPs never repeat the session keys or `ekey`/`eiv`/`et`;
receivers reject that [MODEL].

### 4.13 Steady state on the control connection

`POST /feedback` with no body and no `Content-Type` (`Content-Length: 0`),
every 2 s, the first immediately after setup (do not wait for the first
video frame). Response: a bplist, `{streams: []}` on the Apple TV
[HW]; other receivers list stream dicts like `{type: 96, sr: 44100}`
[MODEL]. Each response also re-anchors the PTP clock (§7.2). A failed
feedback request is logged, not fatal.

`SET_PARAMETER` volume (§8.9) may be sent at any time, e.g. to mute.

### 4.14 Teardown

1. Stop media.
2. `TEARDOWN <audio URI>`, no body (one request ends the whole session;
   the reference sends no per-stream TEARDOWN). Apple TV: 200 after
   ~750 ms [HW]; it then closes the video data connection.
3. Close the video data, event, MediaDataControl and UDP sockets.

If setup fails after the first SETUP was accepted, release the receiver
state with the same TEARDOWN but bounded by a 2 s deadline; if that fails
or times out, close the control connection.

---

## 5. Keys and ciphers

### 5.0 Which path applies

The choice depends only on how the current control connection was
verified, not on the receiver model:

| | **HAP profile**: control channel encrypted after TLV8 pair-verify (every `airplay::pair::connect` connection; the Apple TV) | **Legacy profile**: raw pair-verify, control channel plaintext (Appendix A) |
|---|---|---|
| Video payload | **ChaCha20-Poly1305**, key per §5.1 (HKDF from `S`), AAD = 128-byte header, nonce = 4 zero bytes ‖ u64le frame counter from 0, tag appended (§6.3) | **AES-128-CTR**, key/IV = `fairplay::stream_key_iv(K, video streamConnectionID)` (§5.2), one continuous keystream, no tag |
| Audio payload | **ChaCha20-Poly1305**, random 32-byte key sent as audio `shk`, AAD = RTP bytes 4–11, nonce counter appended (§8.3) | **AES-128-CBC**, key `K`, IV `eiv`, whole blocks only (§8.3); clear if no FairPlay |
| `ekey`/`eiv`/`et` in session SETUP | **not sent** | sent |
| Video `shk`/`shiv` | `K` / `eiv` (placeholders for the receiver; nothing in the sender is keyed by them) | `K` / `eiv` (the receiver derives the CTR key from `shk`) |
| Event channel | ChaCha20-Poly1305, Events keys (§5.1, §9) | plaintext |

So in the HAP profile FairPlay contributes only `K` (via `shk`) and `eiv`
(via `shiv`); `ekey` is produced but not sent, and neither
`fairplay::stream_key_iv` nor any AES path is used for video or audio.
This was checked on the Apple TV [HW]: the accepted session's SETUP #1 had
no `ekey`/`eiv`/`et`, the audio descriptor carried a 32-byte ChaCha `shk`,
and the video ran ChaCha20-Poly1305.

Mapping to `airplay::fairplay` (main): `fp_setup(&mut client)` → session;
`K_fp` = 16 random bytes chosen by the sender; `ekey =
session.wrap_key(&K_fp)`; `K = stream_master_key(&K_fp, S)` where `S` is
`Control::shared.secret()` in the HAP profile and the raw pair-verify
secret in the legacy profile when `X-Apple-PD: 1` was sent (pass an empty
secret only when it was not); `stream_key_iv(&K, id)` = §5.2 video key/IV,
legacy profile only. `eiv` is 16 random bytes chosen by the sender (not a
FairPlay output).

ChaCha20-Poly1305 below is always the RFC 8439 AEAD (as in `ring`) with a
96-bit nonce formed as **4 zero bytes followed by a 64-bit little-endian
counter**. (Apple's code uses the original 64-bit-nonce ChaCha20 variant;
with the 32-bit block counter starting at 1 the two are identical for any
message under 256 GiB, checked over 300 random cases.) HKDF is HKDF-SHA-512
(RFC 5869), output length 32. Decimal numbers inside salts are unsigned,
no leading zeros, no sign.

### 5.1 HAP profile

- **Video key**: `HKDF(ikm = S, salt = "DataStream-Salt" ‖ dec(video
  streamConnectionID), info = "DataStream-Output-Encryption-Key")`, 32 bytes.
  If there is no shared secret but FairPlay ran, the reference uses `K_fp`
  as the IKM instead (never seen in practice).
- **Audio key**: 32 fresh random bytes per session, sent as the audio
  descriptor's `shk` (§4.6). Not derived from anything.
- **Event channel keys**: sender→receiver key `HKDF(S, "Events-Salt",
  "Events-Read-Encryption-Key")`, receiver→sender key `HKDF(S,
  "Events-Salt", "Events-Write-Encryption-Key")` (note the reversal
  relative to the control channel).
- **MediaDataControl keys** (optional): with the 64-bit seed sent in the
  audio descriptor, `salt = "DataStream-Salt" ‖ dec(seed as u64)`;
  receiver→sender key `HKDF(S, salt, "DataStream-Input-Encryption-Key")`,
  sender→receiver key `HKDF(S, salt, "DataStream-Output-Encryption-Key")`.
  The seed goes into the bplist as an `int` with the same 64 bits (it is
  negative when the top bit is set).

Vectors: `keys.txt`.

### 5.2 Legacy profile

- **Video AES-128-CTR**: with `shk` = the 16-byte value sent in the video
  descriptor (= `K`):
  - key = `SHA-512("AirPlayStreamKey" ‖ dec(video streamConnectionID) ‖ shk)[0..16]`
  - initial counter block = `SHA-512("AirPlayStreamIV" ‖ dec(video
    streamConnectionID) ‖ shk)[0..16]`
  (ASCII prefix immediately followed by the decimal digits, then the raw
  16 key bytes.) The counter block increments as a 128-bit big-endian
  integer (standard CTR, as `crypto::aes` CTR). `shiv` is not an input.
- **Audio AES-128-CBC**: key `K`, IV `eiv` (§8.3). No audio encryption if
  FairPlay did not run.

### 5.3 What to put in the video `shk`/`shiv`

`shk = K`, `shiv = eiv` whenever FairPlay ran (both profiles). Without
FairPlay the reference sends, in the HAP profile, the first 16 bytes of the
control write key as `shk` and the first 16 bytes of the control read key
as `shiv`, and two random 16-byte values with no pairing keys at all; these
are placeholders that do not key anything in the HAP profile.

---

## 6. Video data channel

### 6.1 Packet format

Every packet is a 128-byte header followed by `payload_size` payload bytes.
No other framing. Unused header bytes are zero.

| Offset | Size | Field |
|---|---|---|
| 0 | u32le | `payload_size`: number of payload bytes that follow (for ChaCha frames: ciphertext + 16-byte tag) |
| 4 | u8 | packet type: 0x00 video frame, 0x01 codec configuration, 0x02 heartbeat, 0x05 sender report (optional, not sent) |
| 5 | u8 | type 0: 0x10 if the frame contains an IDR picture, else 0x00; other types: 0x00 |
| 6 | u8 | option: type 1: 0x16 H.264 / 0x1e HEVC (0x56 / 0x5e = "stream suspended", not used); type 2: 0x1e; type 0: 0x00 |
| 7 | u8 | type 1: 0x01; otherwise 0x00 |
| 8 | u64le | timestamp, unsigned 32.32 fixed-point seconds (high 32 bits seconds, low 32 bits fraction) on the session timeline (§7); 0 for heartbeats |
| 16–127 | | type-specific, below |

Type 0 (video frame) extra field: offset 40, u64le = PTP timeline id
(`ClockID` as u64) in PTP mode, 0 in NTP mode.

Type 1 (codec) extra fields: IEEE-754 single-precision floats, little
endian: offset 16 width, 20 height (encoded size), 40 width, 44 height
(source rectangle size), 48 x, 52 y (source rectangle origin, 0.0), 56
width, 60 height (destination size). The reference writes the encoded
width/height into all three size pairs and zero origins. (These float
pairs overlap the type-0 timeline field; a codec packet carries no
timeline id.)

Example headers in `video.txt` and, as accepted by the Apple TV,
`appletv-session.txt`.

### 6.2 Codec packet (type 1)

Sent, unencrypted, immediately before the first IDR frame and again before
the next IDR whenever the SPS or PPS bytes change. Its timestamp equals
that of the IDR frame that follows. Payload for H.264 is an
`AVCDecoderConfigurationRecord` plus a 4-byte trailer:

| Offset | Size | Value |
|---|---|---|
| 0 | 1 | 0x01 (configurationVersion) |
| 1 | 1 | SPS byte 1 (profile_idc) |
| 2 | 1 | SPS byte 2 (constraint flags) |
| 3 | 1 | SPS byte 3 (level_idc) |
| 4 | 1 | 0xFF (reserved bits + lengthSizeMinusOne = 3: 4-byte NAL lengths) |
| 5 | 1 | 0xE1 (reserved bits + one SPS) |
| 6 | u16be | SPS length |
| 8 | n | SPS NAL unit (with its NAL header byte, no start code, emulation-prevention bytes kept) |
| 8+n | 1 | 0x01 (one PPS) |
| 9+n | u16be | PPS length |
| 11+n | m | PPS NAL unit |
| 11+n+m | 4 | 0x02 0x00 0x00 0x00 (trailer; Apple senders append it, the reference always sends it) |

Only one SPS and one PPS. No high-profile chroma/bit-depth extension
fields. Vector: `video.txt` "avcC record"; the Apple TV accepted exactly
`01640028ffe1001b67640028…c002000000` [HW].

### 6.3 Video frame (type 0)

Payload plaintext: one access unit in AVCC form — each NAL unit prefixed by
its length as u32be — containing the picture's NAL units in encoder order,
**without** SPS (7), PPS (8) and access-unit delimiters (9); SEI (6) and all
slices are kept. Byte 5 = 0x10 iff a NAL of type 5 (IDR) is present. One
packet per access unit (no fragmentation; payloads of 60+ KiB are normal
[HW]).

Encryption of the payload (the header is never encrypted):

- **HAP profile**: ChaCha20-Poly1305 with the §5.1 video key; nonce counter
  starts at 0 for the session's first video frame and increases by 1 per
  video frame packet (codec packets and heartbeats do not use the cipher);
  AAD = the complete 128-byte header **as sent**, i.e. with
  `payload_size` already including the 16-byte tag. Payload on the wire =
  ciphertext ‖ tag. Vectors: `video.txt` "ChaCha20-Poly1305 video frame".
- **Legacy profile**: AES-128-CTR (§5.2) as **one continuous keystream over
  all video frame payloads of the connection**: the first payload uses
  keystream bytes 0…n−1, the next one starts at byte n, and so on, across
  packet boundaries and partial blocks; codec packets and heartbeats do not
  consume keystream. No tag; `payload_size` = plaintext length. Vectors:
  `video.txt` "AES-128-CTR video payload 0…4" (lengths 37, 5, 20, 16, 3).

Frame timestamps (§7.4) must be strictly increasing; if a computed value is
not greater than the previous frame's, use previous + 1 (one 2⁻³² s unit).
A late frame is still sent with its original deadline-based timestamp (do
not drop reference frames); the receiver decides.

### 6.4 Heartbeat (type 2)

128-byte header only (`payload_size` 0): byte 4 = 0x02, byte 6 = 0x1e, all
else zero. Every 1 s, starting after the first video frame has been sent,
until teardown. A failed heartbeat write ends the session. [HW] sent and
accepted. Writes to the data socket from the heartbeat and the frame sender
must not interleave (one writer at a time, whole packets).

### 6.5 Write behaviour

Write header and payload as one unit (vectored write is fine). Use a write
timeout (reference: 2 s for frames, 1 s for codec packets, 5 s for
heartbeats); a timeout is a session failure. Write durations and
socket backlog are the congestion signal (the reference feeds them into
its encoder bitrate adaptation; out of scope for this spec).

### 6.6 Encoder requirements (informative)

What the reference feeds receivers: H.264 (High profile works [HW]),
progressive 4:2:0 8-bit, **no B-frames**, an IDR at least every ~0.5 s
(`(fps + 1) / 2` frames), constant-bitrate-ish rate control with a VBV of
about two frames, bitrate ≈ width × height × fps / 15000 kbit/s clamped to
1800–12000 (4500 default). Annex-B output with AUDs makes access-unit
splitting trivial. The receiver may send `forceKeyFrame` on the event
channel (§9); honour it if the encoder can.

### 6.7 HEVC (not implementable from this spec yet)

Allowed only when feature 42 is set. Known facts: the codec packet uses
option bytes 6–7 = `0x1e 0x01`; its payload is an ISO-BMFF `hvc1` visual
sample entry (width/height, 72 dpi, depth 0x0018) whose only child is an
`hvcC` box with lengthSizeMinusOne = 3 and one VPS, SPS and PPS; receivers
recognise HEVC by the `hvc1` fourcc at payload offset 4; frame payloads are
as §6.3 with VPS/SPS/PPS/AUD (NAL types 32–35) removed and the IDR flag set
for NAL types 16–21. The exact `hvcC` bit packing and a vector are not
part of this spec; it is out of scope for M6/M7.

---

## 7. Timing

### 7.1 Timestamp formats

- **32.32 seconds** (video headers, NTP packets, internal mapping):
  u64 = `seconds << 32 | fraction`, fraction = `floor(subsecond_ns × 2³² /
  10⁹)`. Video headers store it little endian; NTP and audio sync packets
  big endian.
- **Nanoseconds** (PTP audio sync): from 32.32, `seconds × 10⁹ +
  floor(fraction × 10⁹ / 2³²)`.
- **Milliseconds** (response headers): `X-Apple-RequestReceivedTimestamp`.

### 7.2 PTP mode (no PTP stack needed)

The receiver's `X-Apple-RequestReceivedTimestamp` is in the same time base
as its PTP timeline (milliseconds since its boot), so the sender maps its
own monotonic clock onto the receiver's timeline directly and never sends
or answers PTP messages (the receiver's clock is the master; nothing has to
listen on UDP 319/320).

- **Anchor**: when the SETUP response that carries `timingPeerInfo` (or
  `asyncPTPClockConfig = true`)
  (SETUP #1, or the first SETUP in media-first ordering) has been read
  completely, at local monotonic time `t0`, set `R0 = (received + processing)
  ms` from that response's two headers, converted to 32.32 seconds. The
  mapping is `R(t) = R0 + (t − t0)`. Missing or unparsable headers = PTP
  failure (§2.5).
- **Timeline id** = `ClockID` as u64.
- **Re-anchor** on every `/feedback` response read at local time `t1`:
  compute `R1` from its headers the same way and the projection `P = R0 +
  (t1 − t0)`; the new anchor is `(t1, max(R1, P))`. The mapping never moves
backwards; audio and video timestamps both come from it.
- **Timeline change**: an event-channel `updateTimingPeerInfo` (§9)
  replaces the timeline id and keeps the current mapping unchanged.
- **Clock id arriving late** (`asyncPTPClockConfig`, §4.1): anchor the
  mapping from that SETUP response's two headers anyway (they are present
  on every response), and take only the timeline id from the later
  `updateTimingPeerInfo`. If the headers are missing, PTP failed (§2.5).
  (The reference instead starts the mapping from its own monotonic clock
  in this case, which the never-backwards rule can then keep ahead of the
  receiver indefinitely; do not copy that. No receiver seen here uses the
  asynchronous form, so this rule is untested.)

Worked example [HW]: SETUP #1 response headers `…ReceivedTimestamp:
1201707321`, `…ProcessingTime: 127` → `R0` = 1201707.448 s = 32.32
`0x0012562b_72b020c4`; 1.5 s of local time later `R = 1201708.948 s`; a video
frame captured then gets `R + 0.075 s` (normal lead). The first frame sent
in that session (0.266 s after the anchor) had header timestamp bytes
`382addc92b561200` (= `0x0012562bc9dd2a38`, 1201707.788 s). Its timeline
field is the ClockID, which the receiver sent as a negative bplist int; in
the scrubbed fixtures it is `0xaabbccfffe00000a` (bplist int
−6144091866766770166, header bytes `0a0000feffccbbaa`).

### 7.3 NTP mode

The sender keeps one monotonic clock `C` (any origin; the reference uses
time since boot, or since process start where that is unavailable).

- Video header timestamps are `C(capture) + video lead` in 32.32 seconds
  **without** any epoch offset.
- NTP timing replies and NTP audio sync packets carry `C + 2208988800 s`
  (the 1900→1970 offset added to the seconds part), so that the receiver,
  which removes that offset, lands on the same values as the video headers.

Responder: bind the timing UDP socket before SETUP #1 and advertise its
port. The receiver sends 32-byte requests:

| Offset | Size | Request (receiver → sender) |
|---|---|---|
| 0 | 1 | 0x80 |
| 1 | 1 | 0xd2 |
| 2 | u16be | sequence (Apple TV: 7 [HW]) |
| 4 | 20 | zero |
| 24 | u64be | receiver transmit time (32.32, its clock) |

Reply to the request's source address with a copy of the request where
byte 1 = 0xd3, bytes 8–15 = request bytes 24–31 (origin), bytes 16–23 =
our receive time, bytes 24–31 = our transmit time (both `C + epoch`, may
be the same value); bytes 0, 2–7 as received. Ignore packets shorter than
32 bytes, not starting `0x80 0xd2`; never answer a `0xd3` (two responders
would ping-pong).

[HW] The Apple TV sends three requests while processing SETUP #1 (the
SETUP response comes only after them) and then one every ~3 s; its own
clock reads ~2208988800 s, i.e. it starts near the NTP epoch.

Sender-initiated timing [MODEL: AirServer-style receivers]: if a SETUP
response contains `timingPort`, also send three requests from the timing
socket to that port, 100 ms apart, with sequence 1, 2, 3 and our transmit
time at 24–31, and ignore the `0xd3` replies.

### 7.4 Video timestamp rule

Frame timestamp = `map(capture instant) + video lead`, where `map` is
`R(t)` (PTP) or `C(t)` (NTP) and the capture instant is the local monotonic
time the frame represents (for file playback: the pacing clock's time for
that frame's PTS). Capture instants older than 30 s or more than 1 s in the
future are a bug: stop the stream rather than inventing timestamps. Then
apply the strictly-increasing rule (§6.3).

---

## 8. Realtime audio

### 8.1 Codecs

| Codec | `ct` | `spf` | `audioFormat` | Payload per packet |
|---|---|---|---|---|
| ALAC | 2 | 352 | 0x40000 | one ALAC frame of exactly 352 stereo 16-bit samples at 44.1 kHz |
| AAC-ELD | 8 | 480 | 0x1000000 | one raw AAC-ELD access unit (no ADTS/LATM) of 480 samples, 44.1 kHz stereo |

No codec configuration (magic cookie) is sent; the receiver derives it from
`ct`/`spf`/`sr`. ffmpeg's ALAC encoder always produces 4096-sample frames
(not configurable), so it cannot be used directly; produce 352-sample ALAC
frames from PCM in streamlib, at least as verbatim frames (§8.7). AAC-ELD
needs an external encoder (ffmpeg `libfdk_aac`, profile `aac_eld`,
480-sample frames).

### 8.2 RTP packet

Sent from the sender's audio **data** socket to the receiver's RTP port.
The data socket is a separate UDP socket from the audio control socket; the
reference binds three consecutive ports N (NTP timing), N+1 (audio control,
the port advertised in SETUP), N+2 (audio data), as Apple senders do. Only
N+1 is advertised; the reference states that receivers accept RTP from the
data socket's own port [REF]; the consecutive layout is what was used on
the Apple TV [HW].

| Offset | Size | Value |
|---|---|---|
| 0 | 1 | 0x80 (V=2, no padding, no extension, CC=0) |
| 1 | 1 | payload type 96 (0x60), or 97 (0x61) for RFC 2198 packets; marker bit always 0 |
| 2 | u16be | sequence number: 1 for the first frame, +1 per frame, wrapping |
| 4 | u32be | RTP timestamp (44.1 kHz sample clock) |
| 8 | u32be | SSRC = 0 |
| 12 | … | payload, encrypted as §8.3 |

RTP timestamps: `RECORD` announced `seq=0;rtptime=0`, so the first frame
(sequence 1) has timestamp 352 for ALAC (one frame after 0; for AAC-ELD the
same rule gives 480) and each later frame adds the number of samples since
the first (352 per frame when the source is continuous; a real gap in the
source stays a gap in the timestamps). Datagrams must not exceed 1472
bytes.

### 8.3 Payload encryption

- **HAP profile (ChaCha20-Poly1305)**, key = the 32-byte audio key (§5.1):
  - nonce counter: 0 for the first sealed packet of the session, +1 for
    every packet sealed afterwards (including the extra PT 96 copy made for
    RFC 2198, §8.4); retransmissions reuse the stored bytes and seal
    nothing;
  - AAD = packet bytes 4–11 (RTP timestamp and SSRC, 8 bytes);
  - wire payload = ciphertext ‖ 16-byte tag ‖ the 8-byte little-endian
    nonce counter. Overhead 24 bytes.
  [HW] accepted. Vectors: `audio.txt` "ChaCha20-Poly1305 audio packet".
- **Legacy profile (AES-128-CBC)**, key `K`, IV `eiv`: CBC-encrypt the
  first `floor(len/16) × 16` payload bytes with the IV restarted for every
  packet; the remaining `len mod 16` bytes are sent in the clear. No
  padding, no tag. Vector: `audio.txt` "AES-128-CBC audio packet".
- **No FairPlay and no HAP**: payload in the clear.

### 8.4 Redundancy

The SETUP always says `redundantAudio = 2`. Then:

- **Feature 61 set (RFC 2198)** [HW, Apple TV]: every frame goes out once as
  a PT 97 packet whose payload is RFC 2198: for each redundant block, oldest
  first, a 4-byte header `0xE0` (F=1, block PT 96), then 14-bit timestamp
  offset (primary RTP timestamp − block timestamp) and 10-bit block length
  packed big-endian into the next 3 bytes (`offset >> 6`, `(offset & 0x3F)
  << 2 | length >> 8`, `length & 0xFF`); then the final 1-byte header 0x60
  (F=0, PT 96); then the redundant blocks' payloads in the same order; then
  the primary payload. Redundant blocks are chosen from the previous frames walking back from the
most recent one, at most 2, stopping at the first frame whose offset
exceeds 0x3FFF, whose length exceeds 0x3FF, or that would push the
datagram over 1472 bytes (counting 12 bytes of RTP header and, for ChaCha,
24 bytes of tag and nonce); the chosen ones are then written oldest first. The headers (the first `4k + 1`
  bytes) stay in the clear; everything after them is encrypted as one
  message (§8.3; for ChaCha the AAD and appended nonce are as usual). In
  addition, the primary frame is also sealed on its own as a normal PT 96
  packet with the same sequence and timestamp, **not sent**, and kept in
  the retransmit history (§8.6). The first packet of a session has no
  redundant blocks (payload = `0x60` ‖ primary). Vector: `audio.txt`
  "RFC 2198 audio packet".
- **Feature 61 not set**: send the new frame's PT 96 packet, then resend the
  previous two frames' PT 96 packets byte-for-byte (same sequence numbers),
  i.e. packets N, N−1, N−2 back to back.
- [HW] Sending every frame exactly once with no redundancy was also
  accepted by the Apple TV (statistics showed no lost packets on a quiet
  network). That is the simplest correct start; add redundancy for Wi-Fi.

Pace bursts: at most 12 packets per 5 ms window (only matters when catching
up).

### 8.5 Sync packets (TimeAnnounce)

Sent from the audio **control** socket to the receiver's RTCP port: one
with the first audio frame, then one per second, and one immediately when
the audio lead changes (§8.10).

Define, at the sampled local instant `t`: `rtp_now` = the RTP timestamp of
the sample captured at `t` (for the first packet: the first frame's
timestamp; later: the last sent frame's timestamp plus the samples elapsed
since that frame's capture instant, rounded to the nearest sample), `L` =
audio lead in samples (= `latencyMax`), `rtp_play = rtp_now − L` (mod 2³²).
Meaning: sample `rtp_play` is to be heard at network time `T` (= the
receiver will play each sample L samples after its capture).

PTP form, 28 bytes:

| Offset | Size | Value |
|---|---|---|
| 0 | 1 | 0x90 for the first packet (X bit set), else 0x80 |
| 1 | 1 | 0xd7 |
| 2 | u16be | 4 |
| 4 | u32be | `rtp_play` |
| 8 | u64be | `T` in nanoseconds on the receiver timeline: `R(t)` from §7.2 (no lead added), converted per §7.1 |
| 16 | u32be | `rtp_play` (again) |
| 20 | u64be | timeline id (`ClockID`) |

NTP form, 20 bytes:

| Offset | Size | Value |
|---|---|---|
| 0 | 1 | 0x90 first, else 0x80 |
| 1 | 1 | 0xd4 |
| 2 | u16be | 4 |
| 4 | u32be | `rtp_play` |
| 8 | u64be | `T` = `C(t) + 2208988800 s`, 32.32 |
| 16 | u32be | `rtp_now` (not `rtp_play`: in this form the two RTP fields differ by `L` [REF]) |

Example [HW, ClockID scrubbed]: first PTP packet `90d70004 fffff2bc
000444f26299deaa fffff2bc aabbccfffe00000a` (rtp_now 352, L 3748, so
rtp_play = 352 − 3748 mod 2³²; `T` = 1201707.530 s = R(t) at the first
audio frame's capture, no lead). Vectors in `audio.txt` list their inputs
(`rtp_now`, `t_32_32`, `t_ns`, `timeline_id`) next to each packet; note
the nanosecond conversion floors (`…447999999`).

If the source clock jumps backwards (new segment), continue the RTP
timestamps after the last frame and send the next sync packet with 0x90
again.

### 8.6 Retransmission

Keep the last 512 sent PT 96 packets, indexed by sequence number (for RFC
2198 the stored copy of §8.4). Requests arrive on the audio control socket
(accept only from the receiver's IP):

| Offset | Size | Request (receiver → sender), exactly 8 bytes |
|---|---|---|
| 0 | 1 | 0x80 |
| 1 | 1 | 0xd5 |
| 2 | u16be | request id |
| 4 | u16be | first missing sequence |
| 6 | u16be | count (0 = ignore) |

For each sequence `first … first+count−1` (at most 513 lookups), answer to
the request's source address from the control socket: if stored, `0x80
0xd6 <request id u16be> ‖ <stored packet bytes>`; if not stored, send the
8-byte **expired reply** `0x80 0xd6 <request id u16be> <that sequence
u16be> 0x00 0x00` and stop processing the request. Vector: `audio.txt`
"retransmit request".

### 8.7 ALAC frame layout (verbatim / uncompressed)

A frame is a bit string written MSB-first, zero-padded to a whole byte. For
352 interleaved stereo 16-bit samples:

| Bits | Field | Value |
|---|---|---|
| 3 | element tag | 1 (channel pair; 0 for a mono single channel element) |
| 4 | element instance | 0 |
| 12 | unused | 0 |
| 1 | has-size | 1 (the 32-bit sample count follows) |
| 2 | extra (shifted) bytes | 0 |
| 1 | not-compressed | 1 (verbatim) |
| 32 | sample count | 352 |
| 352 × 2 × 16 | samples | for each sample instant: left then right, each a 16-bit two's-complement value, MSB first (i.e. big-endian; byte-swap from s16le) |
| 3 | end tag | 7 |
| 0–7 | padding | zero to the byte boundary |

Total: 11322 bits → 1416 bytes. With the 12-byte RTP header and 24 bytes of
ChaCha overhead the datagram is 1452 bytes (≤ 1472). Vector: `audio.txt`
"ALAC verbatim frame" (decodes with ffmpeg's ALAC decoder to the given PCM).
Compressed ALAC frames (the reference's normal output, ~270 bytes for a
tone) are equally valid but need an encoder; verbatim is enough for
correctness, at ~1.4 Mbit/s of audio. At 1416 bytes no RFC 2198
redundant block fits in a datagram, so verbatim frames go out without
redundancy under RFC 2198 (and triple the audio rate under the N, N−1, N−2
scheme).

### 8.8 Start-up and pacing

- Start audio only after the first video frame (not just the codec packet)
  has been sent; discard captured audio until then.
- Then discard frames that are already too late: a frame captured at `p` is
  stale if `p + audio lead ≤ now + 5 ms`. Give up (error) after 128
  consecutive stale frames at start-up.
- Establish the RTP/clock mapping (first sync packet) at the first fresh
  frame, then send each frame as soon as it is encoded; do not buffer.
- In steady state never drop a frame for being late; send it anyway, so the
  RTP sample sequence has no holes.

### 8.9 Volume

Values are dB as decimal text: 0 = full, −30 = quietest, −144 = mute.
Outgoing values other than −144 that are NaN, < −30 or > 0 are replaced by
−20.

Initial value: `/info` `initialVolume` (real) and `isMuted` (bool), else the
`GET_PARAMETER` answer (§4.9), else unknown. Apple TV: `initialVolume = 0.0`
[HW].

Classify the initial value by its integer part (truncation toward zero):
unknown/NaN/±∞ → *invalid* (target −20); ≥ 0 → *loud* (target −20);
≤ −30 → *quiet* (target −27); otherwise no correction. Which corrections a
receiver family takes:

| Family (first match) | Corrections applied | Resend on start |
|---|---|---|
| `model` starts with `AudioAccessory` | invalid | no |
| `model` starts with `AppleTV` | invalid, quiet | no |
| `model` starts with `AirPort` | invalid, quiet | no |
| `manufacturer` starts with `Apple` | invalid | no |
| third-party (feature 26 or 51) and feature 49 | invalid | no |
| anything else | invalid, quiet, loud | yes |

(case-insensitive). If the family takes the correction, the initial volume
becomes its target and counts as known. Every family takes *invalid*, so an
unknown or unparsable initial volume always ends up as −20 and is sent. A
value of exactly −144 that was not corrected means muted (keep −20 as the
unmute level). Send the initial `SET_PARAMETER` (§4.11) iff the value is
known and (it was corrected, or it is muted (send −144), or the family
resends on start). Dev Apple TV: 0.0 is *loud*, not corrected for Apple TV,
so no volume is sent [HW]. Third-party TVs reportedly need an explicit
volume; the "anything else" row ensures one.

### 8.10 MediaDataControl (optional; dynamic audio latency)

Only when features 59 and 60, `sourceVersion` ≥ 450.1.0, the HAP profile,
layout S, and the automatic latency profile are all in effect. The sender
adds the seed entry to the audio descriptor (§4.6), and after setup
connects TCP to the returned MediaDataControl port. Framing is the same as
the control channel (u16le length ≤ 1024, ChaCha20-Poly1305, AAD = the two
length bytes, counter nonces from 0 per direction) with the §5.1 keys.
Inside, messages are a 32-byte header plus payload:

| Offset | Size | Field |
|---|---|---|
| 0 | u32be | total size, header included (≥ 32) |
| 4 | 4 | message type, ASCII: `sync` (reply wanted), `asyn` (no reply), `rply` (a reply) |
| 8 | 8 | opaque; copied unchanged into the reply |
| 16 | 4 | command, ASCII (`raup`, `rrmt`, …) |
| 20 | u64be | reply token; copied into the reply |
| 28 | u32be | status (0 in requests) |
| 32 | … | payload, a bplist | Commands seen [HW]: `raup` (audio performance
report: `ProtocolVersion` 1, `SessionDurationSec`, `RenderDeadlineMinMs`,
glitch histograms) and `rrmt` (RTP statistics: `lostPackets`,
`latePackets`, `totalGlitches`, `latencyMs`, …). For `sync` messages reply with a 32-byte header and no payload: size 32,
type `rply`, bytes 8–15 of the request, the same command, the same token,
status 0 (accepted) or 1 (payload not a valid report); no other status
values are known. Vector: `channels.txt` "MediaDataControl". The
dynamic-latency algorithm (when and by how much to step the audio lead
along the ladders in the data file, driven by `raup` reports carrying a
`LiveAdaptiveGlitchHistogram`) is **out of scope** for this spec; a sender
that connects this channel should keep its initial lead. Skipping this channel entirely is fine: the session keeps the
initial lead [HW, a session without it was accepted]. The `rrmt` report is
a cheap way to check that audio arrives.

---

## 9. Event channel

TCP to `eventPort`. HAP profile: the same framing as the encrypted control
channel (u16le plaintext length ≤ 1024 ‖ ciphertext ‖ 16-byte tag, AAD =
the 2 length bytes, ChaCha nonce = 4 zero bytes ‖ u64le counter, counters
from 0 per direction) but with the §5.1 event keys (vector: `channels.txt`
"event channel"). Legacy profile: plaintext. Message boundaries do not align with frames.

The receiver sends RTSP requests; the sender only answers. Request:
`POST /command RTSP/1.0` with `CSeq`, `Content-Type:
application/x-apple-binary-plist`, `Content-Length`, bplist body
`{type: string, value: …}`. Answer every request, whatever it is, with

```
RTSP/1.0 200 OK\r\nCSeq: <same>\r\nContent-Length: 0\r\n\r\n
```

Types:

| `type` | `value` | Action |
|---|---|---|
| `updateInfo` | the `/info` dict, including `displays` and `txtAirPlay` [HW] | may refresh the canvas (only before encoding starts) |
| `updateTimingPeerInfo` | a `timingPeerInfo` dict with `ClockID` | §7.2 timeline change |
| `forceKeyFrame` | — | ask the encoder for an IDR if possible |

[HW] The Apple TV sent `updateInfo` right after the RECORD/audio SETUP,
sometimes followed by one or two more commands. Limit header size and body
size (e.g. 16 KiB / 1 MiB); a malformed message ends the event channel,
not the session.

---

## 10. Receiver families (facts and models)

| Family | Pairing / profile | SETUP order | Timing | Audio descriptor | Notes |
|---|---|---|---|---|---|
| Current Apple TV (tvOS; dev unit AppleTV6,2, sourceVersion 960.13.1) [HW] | HAP (`X-Apple-HKP` 3 or CoreUtils types) | control-first | PTP (NTP also accepted) | layout S, ChaCha `shk`, MediaDataControl, RFC 2198 | `displays` only after SETUP #1 via `GET /info`/`updateInfo`; ignores `combinedGetInfoWithControlSetup`; `screenStream` 0x1440800; initialVolume 0.0; statusFlags change from 1604 to 132676 once a session exists |
| Apple TV 4K gen 2/3, Macs [REF] | same | control-first | PTP | layout S | macOS may ask the user to accept |
| Apple TV 3rd gen (AppleTV3,2, sourceVersion 220.68) [MODEL] | legacy raw pairing, plaintext control | media-first | NTP, receiver probes | layout P, no shk, AES-CBC | needs `ekey`/`eiv`/`et` in the session SETUP; no `supportedFormats` (use feature bits); video AES-CTR |
| Roku (Streaming Stick 4K 3820R2, sourceVersion 377.40.00) [MODEL; Roku/TCL Roku reported working] | legacy (HKP 3) | media-first | advertises PTP (feature 41, `PTPInfo`), uses NTP with receiver probes | layout P | omits `supportedFormats`; AAC-ELD |
| LG (75UP75009LC, sourceVersion 377.25.06) [MODEL; LG not in the reference's tested list] | HKP 3 but TLV8/HAP, encrypted control | media-first (rejects control-only SETUP) | accepts PTP but sends no anchor headers → PTP fails → fall back to NTP on a new connection | — | ALAC only (`screenStream` 0x40000); third-party TV family |
| Samsung TU8300, Hisense, TCL Google TV [REF: reported working] | — | — | — | — | no per-family facts recorded; third-party TVs need an explicit initial volume |
| UxPlay (open-source receiver, advertises AppleTV3,2) [MODEL] | legacy raw pairing | control-first | NTP, receiver probes | ALAC, layout P | no `eventPort`; AES-CTR video keyed from `K = SHA-512(K_fp ‖ S)[0..16]`, because the legacy pair-verify sent `X-Apple-PD: 1` (§2.2, §5.2) |
| AirServer / Airtame [MODEL] | legacy raw pairing | control-first | NTP, **sender-initiated** (SETUP response has `timingPort`) | AAC-ELD only (`screenStream` 0x1000000) | |
| Xiaomi 4K HDR TV (AFTBR92D74) [REF] | | | | | reported not working |

Generic rules that cover these without model checks: §4.1 (fall back to
media-first on 400/405/406/415/455/501), §4.6 (retry the other audio
layout once), §2.5 (PTP → NTP on a fresh connection), §4.4 (RECORD later on
455), §4.13 (no GET_PARAMETER keep-alive).

---

## 11. Observed Apple TV session

Transcript of one accepted session (dev Apple TV, PTP, ALAC, 12 s; bodies in
`appletv-session.txt`, scrubbed):

1. `GET /info` → 200, no `displays`, statusFlags 1604.
2. `POST /pair-verify` ×2 (`X-Apple-HKP: 3`) → M2, M4 (`06 01 04`); channel
   encrypted.
3. `POST /fp-setup` m1 → m2 (142 bytes), m3 → m4 (32 bytes).
4. `SETUP rtsp://…/<A>` session (PTP) → `{eventPort, timingPeerInfo
   {ClockID <0, ClockPorts, …}}`, ProcessingTime 127 ms. Event channel
   connected.
5. `GET /info` → 2032 bytes with `displays` (1280×720, max 3840×2160),
   statusFlags 132676.
6. `RECORD rtsp://…/<A>` → 200, `Audio-Latency: 0`; event `updateInfo`
   arrives.
7. `SETUP rtsp://…/<A>` audio (layout S, ChaCha shk, MediaDataControl seed)
   → RTP/RTCP/MediaDataControl ports, `streamID`, `arrivalToRenderLatencyMs
   27`.
8. `SETUP rtsp://…/<V>` video → `{streams: [{type 110, dataPort}]}`.
9. Data connection; `POST /feedback` → `{streams: []}`.
10. Codec packet (48-byte avcC, 1920×1080) + IDR (66802 bytes) at the same
    timestamp; frames at 30 fps; first TimeAnnounce `0x90 0xd7`; RFC 2198
    audio; heartbeats each second; feedback every 2 s; TimeAnnounce each
    second.
11. MediaDataControl `asyn raup` after ~8 s, `asyn rrmt` after ~10 s (0 lost
    packets).
12. `TEARDOWN rtsp://…/<A>` → 200 after 711 ms; receiver closes the data
    connection.

Variants also accepted [HW]: NTP timing (receiver probed 3× during SETUP #1,
then every ~3 s); no MediaDataControl + no redundancy + fixed 100 ms lead;
FairPlay skipped (see §2.2 caveat).

---

## 12. Test vectors

`tests/vectors/airplay-mirror/`:

| File | Contents |
|---|---|
| `keys.txt` | master key `K`; legacy video CTR key/IV for two ids; ChaCha video keys; event keys; MediaDataControl keys with a high-bit seed |
| `video.txt` | avcC record; codec packet header+payload; two ChaCha video frames (header = AAD, ciphertext‖tag); five AES-CTR payloads on one keystream; an NTP-mode frame header; heartbeat |
| `audio.txt` | ChaCha audio packets (nonce 0, 1); RFC 2198 packet with two redundant blocks plus its stored PT 96 copy; AES-CBC packet with a partial block; PTP and NTP sync packets; retransmit request and both response forms; ALAC verbatim frame with its PCM |
| `channels.txt` | legacy raw pair-verify CTR key/IV; an encrypted event-channel frame; a MediaDataControl request header and its reply |
| `setup.txt` | SETUP #1 (PTP/HAP; NTP/legacy with root fields), SETUP #2 (both layouts), SETUP #3 as bplist hex with an XML rendering |
| `appletv-session.txt` | scrubbed real request/response bodies and headers from the Apple TV session in §11, MediaDataControl messages, and the codec/frame/sync/heartbeat bytes it accepted |

All synthetic inputs are byte ramps (`000102…`); keys in the real-session
file are zeroed.

---

## 13. Open points

1. **Rendering not confirmed.** All [HW] facts are protocol acceptance.
   Someone has to watch and listen during a session (video picture, A/V
   sync, ALAC verbatim audio audible) — ideally with streamlib's own W7
   build, which settles it for good.
2. Whether the Apple TV needs FairPlay at all in the HAP profile (§2.2).
3. Whether sending the encoded size larger than the advertised 1280×720
   canvas is rendered at full resolution or scaled (§2.6); the dev Apple
   TV's HDMI output mode is unknown.
4. Receiver families other than the Apple TV are models (§10); LG, Samsung,
   Roku have not been observed here.
5. Legacy pairing (Appendix A) is described from the reference only.

---

## Appendix A. Legacy (raw) pairing, for third-party receivers

Receivers whose features lack all of bits 38/43/46/48, or that have bit 26
or 51, use `X-Apple-HKP: 3` and may implement the original non-TLV
protocol. Raw pair-verify requests carry `Content-Type: application/octet-stream`,
`User-Agent`, `X-Apple-ProtocolVersion: 1`, `X-Apple-PD: 1`, `CSeq` and
`Content-Length`; no `X-Apple-Session-ID`.

- Raw pair-setup: `POST /pair-setup` with only the usual `CSeq`,
  `User-Agent`, `Content-Type: application/octet-stream` and
  `Content-Length` headers; body = the sender's 32-byte Ed25519 public key
  (the controller's long-term key, the same one sent in V1); response = the
  receiver's 32-byte Ed25519 public key. It is unauthenticated (no PIN).
  Do it once and store the receiver key with the controller identity, like
  a HAP pairing; later connections only run raw pair-verify against the
  stored key. If a later connection does run raw pair-setup again, the
  returned key must equal the stored one, else refuse the receiver. If raw
  pair-setup fails, try TLV8 transient pair-setup with HKP 3; if that also
  fails with 400/404/405/415/455/501, the receiver is not legacy.
- Raw pair-verify, two `POST /pair-verify` exchanges:
  - V1 (68 bytes): `01 00 00 00` ‖ sender X25519 ephemeral public key (32)
    ‖ sender Ed25519 public key (32).
  - V2 (96 bytes): receiver X25519 public key (32) ‖ encrypted receiver
    signature (64).
  - `S` = X25519(sender secret, receiver public). AES-128-CTR key =
    `SHA-512("Pair-Verify-AES-Key" ‖ S)[0..16]`, initial counter block =
    `SHA-512("Pair-Verify-AES-IV" ‖ S)[0..16]` (plain SHA-512, not HKDF).
    Vector: `channels.txt` "legacy raw pair-verify".
  - Decrypt the signature with keystream bytes 0–63; verify it with the
    receiver's long-term Ed25519 key (the one from pair-setup, stored) over
    `receiver X25519 pub ‖ sender X25519 pub`.
  - V3 (68 bytes): `00 00 00 00` ‖ the sender's Ed25519 signature over
    `sender X25519 pub ‖ receiver X25519 pub`, encrypted with keystream
    bytes 64–127 of the same CTR stream.
  - V4: empty 200.
  - The control connection stays plaintext; the legacy profile applies.
