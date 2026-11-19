# tvpipe

Stream a program's own frames and sound — or a file, a web page, a
screen — to an Apple TV, a Chromecast or Google TV, or a browser tab,
with low latency, from Rust. In process: no `tvpipe` binary, no async
runtime, no ffmpeg needed for raw frames.

```rust,no_run
use std::time::Duration;
use tvpipe::{Options, PixelFormat};

fn main() -> tvpipe::Result<()> {
    let mut s = tvpipe::stream(
        Options::new()
            .to("Living Room") // a name from tvpipe::discover(), or "browser"
            .video(1280, 720, 60, PixelFormat::Rgba)
            .audio(48_000, 2),
    )?;
    let frame = vec![0u8; 1280 * 720 * 4];
    let pcm = vec![0i16; 48_000 / 60 * 2];
    for _ in 0..600 {
        let pts = s.elapsed();
        let _ = s.write_video(&frame, pts)?; // never blocks; may drop
        s.write_audio(&pcm, pts)?;
        std::thread::sleep(Duration::from_millis(16));
    }
    s.finish()?; // or drop it: the TVs get their TEARDOWN/STOP
    Ok(())
}
```

Raw frames go to a built-in H.264 encoder (keyframes on demand, so a TV
that joins or loses a packet recovers at once); a program with a
hardware encoder sends its own H.264 instead (`Options::h264`,
`Stream::write_h264`). Several receivers play on one timeline.

- [`discover`] finds receivers; [`Discovery`] keeps looking for a
  program's receiver menu.
- [`pair`] / [`Pairing`]: Apple TVs pair once with a PIN; or give
  `Options::on_pin` and a stream pairs when it has to.
- [`play`] (files, URLs, capture through ffmpeg), [`test_pattern`],
  [`preview`] (a browser tab) and [`share`] (a tab, window or screen,
  through a page in the user's browser) return a [`Session`].
- Events ([`Event`]): poll them, get a callback, or read the snapshots
  (`receivers()`, `stats()`, `player_url()`).
- Pairings live in a [`Storage`]: by default the program's own
  directory; `Storage::cli()` shares the `tvpipe` command's.
- Logging is off until the program installs a handler
  ([`log::set_logger`]); the library never writes to stdout or stderr.

The [`recipes`] cover wgpu readbacks, Bevy, macroquad, cpal audio and
hardware encoders. The `tvpipe` command-line tool (the `tvpipe-cli`
crate, and release binaries) is built on this crate:
<https://mitsuhiko.github.io/tvpipe/>.

## Debug builds

The built-in encoder is too slow without optimizations. Add to the
program's `Cargo.toml`:

```toml
[profile.dev.package.tvpipe]
opt-level = 3
[profile.dev.package.ring]
opt-level = 3
```

## Features

| Feature | Default | Brings |
|---|---|---|
| `airplay` | yes | AirPlay mirroring: pairing, video, ALAC audio (`ring`) |
| `cast` | yes | Google Cast: control over TLS, Cast Streaming, AAC (`rustls`) |
| `player` | yes | the `browser` receiver: a player page on 127.0.0.1 |
| `h264` | yes | the built-in H.264 encoder (raw frames, the test pattern) |
| `aac` | with `cast` | the built-in AAC-LC encoder |
| `fairplay` | no | FairPlay for AirPlay receivers that need it (LGPL, see below) |
| `share` | no | `share`: a capture page for a tab, window or screen |
| `headless` | no | `share(Some(url), …)`: a browser of tvpipe's own |
| `tunnel` | no | public HTTPS links for the pages (`rustls`) |
| `files` | no | MP4/MOV/Matroska sent unchanged, without ffmpeg |
| `ffmpeg` | no | files, URLs and capture through an ffmpeg subprocess |
| `signals` | no | `signals`: Ctrl-C/SIGTERM stops a session cleanly (the receivers get their teardown) |

A program that only wants AirPlay with its own encoder takes
`default-features = false, features = ["airplay"]` (no `rustls`). Asking
for something a build leaves out fails with the error code
`feature-disabled`.

## Licensing

tvpipe is Apache-2.0. FairPlay, which some AirPlay receivers need, is a
port of [doubletake](https://github.com/omarroth/doubletake) under
LGPL-3.0-or-later, in its own crate, `tvpipe-fairplay`, which only the
`fairplay` feature pulls in. Rust links statically, so:

| Features | What you ship | Obligations |
|---|---|---|
| default (no `fairplay`) | Apache-2.0 code only | the Apache-2.0 notice, and the third-party notices in `LICENSES/` (openh264 tables BSD-2-Clause; OFL fonts with `player`/`share`; CDLA roots with `tunnel`) |
| `fairplay` | LGPL-3.0-or-later code, linked statically | the above, plus LGPLv3: the license texts, a notice, and relinkability (your object files or source) |

Without `fairplay`, Apple TVs (pairing with a PIN) work; a receiver that
needs FairPlay fails with the code `fairplay-required`. Cargo unifies
features across a dependency graph, so check your own build:
`cargo tree -e normal --prefix none --format "{p} {l}" | grep LGPL` must
print nothing. This is not legal advice.

Programs that want FairPlay without linking it can run the `tvpipe`
command as a child process (see [`recipes`]).
