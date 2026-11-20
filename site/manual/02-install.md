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

tvpipe also uses ffprobe, which comes with ffmpeg. If ffmpeg is not on your `PATH`, point tvpipe to it with `--ffmpeg PATH` or the `TVPIPE_FFMPEG` environment variable.

## Updating and other versions

:::aside Installer settings
`TVPIPE_VERSION` A version to install instead of the latest.

`TVPIPE_INSTALL_DIR` Another directory for the binary.
:::

Run the installer again to update. To install a specific version:

```sh
$ curl -fsSL https://tvpipe.party/install.sh | TVPIPE_VERSION=0.1.0 sh
```

```powershell
PS> $env:TVPIPE_VERSION = "0.1.0"; irm https://tvpipe.party/install.ps1 | iex
```

Every release has these archives, with a `SHA256SUMS` file that the installers check them against:

| Platform | Archive |
|---|---|
| macOS, Apple silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Linux, x86_64 | `x86_64-unknown-linux-musl` |
| Linux, ARM64 | `aarch64-unknown-linux-musl` |
| Windows, x64 | `x86_64-pc-windows-msvc` |
| Windows, ARM64 | `aarch64-pc-windows-msvc` |

The Linux binaries are static and run on any distribution.

## Uninstalling

Delete the `tvpipe` binary and the [config directory](config.md).

## With Cargo

:::aside Needs
Rust 1.85 or newer and a C compiler.
:::

```sh
$ cargo install tvpipe-cli
```

This builds tvpipe from crates.io and puts it into `~/.cargo/bin`. To build from the source code instead:

```sh
$ git clone https://github.com/mitsuhiko/tvpipe
$ cd tvpipe
$ cargo build --release
```

The binary is `target/release/tvpipe`.
