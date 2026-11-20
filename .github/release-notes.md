## Install

macOS and Linux:

```sh
curl -fsSL https://tvpipe.party/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://tvpipe.party/install.ps1 | iex
```

The installers download the archive for your platform from this release,
check it against `SHA256SUMS` and put `tvpipe` in `~/.local/bin` (Windows:
`%LOCALAPPDATA%\tvpipe\bin`, added to your user `PATH`). Set
`TVPIPE_VERSION={{version}}` to install this version specifically.

Or download an archive yourself:

| Platform | Archive |
|---|---|
| macOS, Apple silicon | `tvpipe-{{version}}-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `tvpipe-{{version}}-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 (static, any distribution) | `tvpipe-{{version}}-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 (static, any distribution) | `tvpipe-{{version}}-aarch64-unknown-linux-musl.tar.gz` |
| Windows x64 | `tvpipe-{{version}}-x86_64-pc-windows-msvc.zip` |
| Windows ARM64 | `tvpipe-{{version}}-aarch64-pc-windows-msvc.zip` |

ffmpeg 6 or newer (`ffmpeg` and `ffprobe` in `PATH`) is optional: tvpipe
uses it to transcode inputs it cannot send as they are (see "When ffmpeg is
used" in the README). The macOS binaries are not notarized:
if you downloaded an archive with a browser, run
`xattr -d com.apple.quarantine tvpipe` once.

## License

tvpipe's source is Apache-2.0, except the FairPlay crate
`crates/tvpipe-fairplay/`, a port of
[doubletake](https://github.com/omarroth/doubletake) under
LGPL-3.0-or-later. These binaries include that crate, so **the binaries
are distributed under the terms of LGPL-3.0-or-later**. The corresponding
source is this release's tag, `v{{version}}` (also attached below as
source archives).

The binaries also contain fonts under the SIL Open Font License 1.1
(Sixtyfour, Michroma, IBM Plex; served by the share page) and H.264
encoder tables transcribed from openh264 (BSD-2-Clause). Every archive
contains `LICENSE`, `LICENSES/` (LGPL-3.0 with the GPL-3.0 text it builds
on, OFL-1.1, BSD-2-Clause) and `README.md`.
