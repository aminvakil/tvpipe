---
title: Installation
summary: Install tvpipe with one command.
group: Start
spec:
  Platforms: macOS; Linux; Windows
---

Install tvpipe with one command.

## Installing

:::aside Where it goes
macOS and Linux: `~/.local/bin`

Windows: `%LOCALAPPDATA%\tvpipe\bin`, added to your `PATH`
:::

:::install
:::

The installer downloads the release for your platform from GitHub and checks it before installing. The archives are also on the [releases page](https://github.com/mitsuhiko/tvpipe/releases).

## ffmpeg

Playing files, URLs, cameras and the screen needs ffmpeg. Sharing a browser tab or a web page does not.

| Platform | Install |
|---|---|
| macOS | `brew install ffmpeg` |
| Debian, Ubuntu | `apt install ffmpeg` |
| Windows | `winget install ffmpeg` |

If ffmpeg is not on your `PATH`, point tvpipe to it with `--ffmpeg PATH` or the `TVPIPE_FFMPEG` environment variable.

## Building from source

:::aside Needs
Rust 1.85 or newer and a C compiler.
:::

```sh
$ git clone https://github.com/mitsuhiko/tvpipe
$ cd tvpipe
$ cargo build --release
```

The binary is `target/release/tvpipe`.
