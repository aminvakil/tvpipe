---
title: Installation
summary: Build tvpipe and, if you need it, ffmpeg.
group: Start
spec:
  Needs: Rust 1.85+; A C toolchain
  Optional: ffmpeg, ffprobe
---

tvpipe is a single binary. Build it from source; at runtime it needs nothing besides ffmpeg, and only for sources it cannot read itself.

## Building from source

:::aside Toolchain
The C toolchain is for `ring`, which provides the cryptography for TLS and AirPlay pairing.
:::

```sh
$ git clone https://github.com/mitsuhiko/tvpipe
$ cd tvpipe
$ cargo build --release
```

The binary is `target/release/tvpipe`. Copy it anywhere on your `PATH`; it needs no other files next to it.

## ffmpeg

:::aside Lookup order
1. `--ffmpeg PATH`
2. `$TVPIPE_FFMPEG`
3. `ffmpeg` on `PATH`
:::

tvpipe uses ffmpeg to read and re-encode files, URLs and capture devices. It is not needed for the test pattern, for sharing a browser tab or a web page, or for games that send raw frames or their own H.264.

| Platform | Install |
|---|---|
| macOS | `brew install ffmpeg` |
| Debian, Ubuntu | `apt install ffmpeg` |
| Windows | `winget install ffmpeg`, or any build with `ffmpeg.exe` and `ffprobe.exe` |

[Options and encoders](11-options.md) lists exactly when ffmpeg is used.

## Licence

:::aside Licence
:::

tvpipe is Apache-2.0, except the FairPlay module (`src/airplay/fairplay/`), a port of doubletake under LGPL-3.0-or-later. While that module is included, the distributed binary is under LGPL-3.0-or-later terms.
