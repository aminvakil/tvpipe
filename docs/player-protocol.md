# Browser player protocol (`--to browser`)

`--to browser` is a receiver like a TV: tvpipe serves a player page and
streams to it over a WebSocket; the page decodes and presents on tvpipe's
shared timeline (`ARCHITECTURE.md`, "Several Receivers"). `tvpipe preview` is `play --to browser`.
Code: `src/browser/player.rs` (server and sink), `player.html`,
`player.js` (the page). Version 1.

## Transport

The share page's rules ([`share-protocol.md`](share-protocol.md)), with one more check:

- tvpipe listens on `127.0.0.1` only and prints
  `http://127.0.0.1:PORT/?t=TOKEN`. The URL is the same every run, so a
  tab or an OBS source reconnects to the next session:
  - `PORT` is 8765, or `--port` (0: any free port). When 8765 is taken
    (another tvpipe), tvpipe warns and takes a free port for this run.
  - `TOKEN` (16 random bytes in hex) is kept in `player-token` in the
    config directory (0600 on unix), made on first use; `--new-token`
    replaces it. A config directory that cannot be written costs only
    the stable URL (a token for this run, with a warning).
  - The share page keeps a new token per run: it captures the user's
    screen, while the player only shows what tvpipe streams anyway, so
    a lasting token there is the better trade.
- `GET /?t=TOKEN`: the page. `GET /player.js`: the script (static, no
  token). `GET /ws?t=TOKEN`: the WebSocket (RFC 6455, no extensions).
- Every request needs a `Host` of `127.0.0.1:PORT`, `localhost:PORT` or
  `[::1]:PORT` (421 otherwise: DNS rebinding); the page and the socket
  need the token (403).
- The socket also refuses an `Origin` other than those hosts (403): other
  web pages cannot open it even if they learnt the token. Clients without
  an `Origin` (not browsers) are let in.
- Pages send at most 64 KiB per message (all text).

Any number of tabs may connect. Each gets the current `status` and, once
the stream started, the `config` right away, then records from the next
keyframe on.

## Text messages (JSON objects with a `type`)

tvpipe → page:

| type | fields |
|---|---|
| `status` | `state` (`waiting`: no stream yet, `streaming`), `message` |
| `config` | `version` (1), `name`, `video` (`codec` e.g. `avc1.42C01F`, `width`, `height`, `fps`, `description`: the avcC in base64, absent when the access units are Annex B with in-band SPS/PPS), `audio` (`{codec: "pcm-s16le", sampleRate, channels}` or null), `latency` (playout delay, ms) |
| `pong` | `t` (the ping's value, unchanged), `now` (tvpipe's clock, µs) |
| `end` | `reason`: the session is over (the socket closes after it) |

Page → tvpipe:

| type | fields |
|---|---|
| `hello` | `version`, `agent`, `features` (informational, logged at `-v`) |
| `ping` | `t` (the page's clock); answered with `pong` |
| `keyframe` | `reason`: the decoder failed or fell behind; video for this tab skips to the next keyframe |
| `stats` | every 2 s (logged at `-v`): `fps`, `delayMs`, `lateMs`, drops, audio counters, `rttMs`, … |
| `log` | `level`, `message` (errors and warnings go to tvpipe's log) |

Unknown types are ignored by both sides.

## Binary messages (tvpipe → page)

Each is one SLRW record ([`slrw.md`](slrw.md)), `u8 kind | u64le pts_us |
u32le len | payload`, `len` = message length − 13:

| kind | payload |
|---|---|
| 3 `h264` | `u8 flags` (bit 0: keyframe, bit 1: Annex B) + one access unit (AVCC with 4-byte lengths unless Annex B) |
| 2 `audio` | s16le interleaved PCM at the config's rate and channels |

`pts_us` is **the instant the session scheduled the frame at**, on
tvpipe's clock (µs since the sink started), the same instant every other
receiver of the session got for that frame. The page shows the frame (or
plays the samples) at `pts_us + latency`.

## Timing on the page

- **Clock mapping.** The page pings once a second (five quick ones at
  first) and keeps the offset of the sample with the shortest round trip
  among the last 16 (`offset = now − (t0 + t1) / 2`). On loopback the
  round trip is 0.1–1 ms.
- **Video.** `VideoDecoder` (`optimizeForLatency`), decoded frames held
  until due. Each animation frame draws the newest frame due before the
  middle of the next display refresh (frames are on time ± half a
  refresh, not up to a whole one late) and drops older ones. More than 24
  chunks queued in the decoder or a decoder error: skip to a keyframe and
  tell tvpipe. The canvas has the stream's size and is letterboxed by CSS
  (`object-fit: contain`).
- **Audio.** Each record becomes an `AudioBuffer` played by an
  `AudioBufferSourceNode` at the context time when it must be heard
  (`getOutputTimestamp`, else `currentTime` minus the output latency).
  Records within 30 ms of the running schedule are played back to back;
  further off, the schedule restarts at the right time. Records too late
  to play are dropped. Browsers start audio only after a user gesture:
  the page shows "Click for sound" until the context runs.
- `?delay=MS` overrides the playout delay on one page (presenting into a
  call favours a steady picture over latency).

## tvpipe's side

- **One receiver, many tabs.** Each tab has a queue (256 messages, 8 MiB)
  and a writer thread, so `Sink::push` never blocks. A full queue drops
  the frame and the tab waits for the next keyframe; the others go on. A
  tab whose socket stalls for 5 s is dropped.
- **Keyframes.** A tab starts at a keyframe. The sink reports
  `wants_keyframe` only while *no* tab is playing (the first tab, or all
  tabs recovering): the fanout answers it by skipping video for the whole
  sink, which would freeze the tabs already watching. A tab joining
  mid-stream therefore waits for the next regular keyframe (the GOP is
  0.5 s with a browser in the session).
- **No tab:** frames are dropped (`push` returns `Dropped`) and the
  session goes on; `--wait-for-viewer` holds the connect phase until a
  tab is there. Tabs come and go; the browser never ends a session.
- **Audio family.** `Capabilities` has one `AudioFamily`; the browser
  takes the AirPlay family's PCM (lists ALAC at common rates, mono and
  stereo, which the AirPlay family plans as s16le PCM, like the AirPlay
  sink). No extra encode for the test pattern, games, `share` and AirPlay
  sessions; ffmpeg adds one PCM track otherwise. Passthrough files with
  AAC audio are re-encoded when a browser listens (as with AirPlay).
- **Budget** (the table in `ARCHITECTURE.md`): `--latency L` → L;
  with Cast receivers → Cast's delay (300 ms); otherwise 100 ms (on
  loopback the delay only absorbs the input's burstiness and decoding;
  also the AirPlay automatic profile's video lead).
- `display_aspect` is `None` (any shape) and sizes up to 3840x2160@60
  are offered: the page letterboxes any shape, so `share` (with or without
  URL) with only browser players keep the source's shape; with TVs the TVs'
  shape wins.

## Browsers

Measured with `preview` of the built-in test pattern, 1280x720@30, on
macOS 26 (Apple Silicon), `cargo test --release` opt-in tests in
`src/session/player_tests.rs`:

| | Chrome 157 (headless) | Safari 26 | Firefox 160 (headless) |
|---|---|---|---|
| Decoding | VideoDecoder, no errors | VideoDecoder, no errors | VideoDecoder, no errors |
| Delay, capture instant → drawn (target 100 ms) | 94–107 ms | 100–103 ms | 103 ms |
| Audio lead at scheduling | ~75–100 ms | ~95 ms | ~118 ms |
| `getOutputTimestamp` vs `currentTime`/latency | ±3 ms | +14 ms | +23 ms |

Glass to glass (the test pattern's burned-in clock against the page's
clock in the frame that shows it, Chrome screenshot): 114 ms at a 100 ms
target, before the half-refresh rule; the delay statistic matched the
photo within 1 ms. The test pattern's clock is taken when the frame is
made, so this includes the built-in encoder.
