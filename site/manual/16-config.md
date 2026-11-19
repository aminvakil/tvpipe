---
title: Configuration
summary: Where tvpipe keeps its files.
group: Reference
---

tvpipe has no configuration file. It keeps two files and reads a few environment variables.

## Config directory

:::aside Elsewhere
`TVPIPE_CONFIG_DIR` moves it.
:::

| Platform | Directory |
|---|---|
| Linux | `$XDG_CONFIG_HOME/tvpipe`, usually `~/.config/tvpipe` |
| macOS | `~/Library/Application Support/tvpipe` |
| Windows | `%APPDATA%\tvpipe` |

| File | Contents |
|---|---|
| `credentials.txt` | Apple TV pairings. Keep it private. |
| `player-token` | The secret part of the player page address. |
| `opentunnel/` | This computer's OpenTunnel tunnel and certificate, made on the first `--tunnel=opentunnel`. Keep it private. |

## Environment variables

| Variable | Effect |
|---|---|
| `TVPIPE_CONFIG_DIR` | The config directory. |
| `TVPIPE_FFMPEG` | The ffmpeg to use. |
| `TVPIPE_BROWSER` | The browser for `share URL`. |
| `TVPIPE_OPENTUNNEL_API` | A self-hosted OpenTunnel service. |
| `TVPIPE_OPENTUNNEL_DIR` | Another directory for the `opentunnel/` files. |
| `TVPIPE_NO_QR` | `1`: no QR codes of public links. |
| `TVPIPE_LOG` | The log at this level: `error`, `warn`, `info`, `debug` or `trace`. See [Diagnostics](diagnostics.md). |
| `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `FORCE_COLOR` | Colours in the terminal. See [Diagnostics](diagnostics.md). |
| `SSL_CERT_FILE` | More certificates to trust when connecting to OpenTunnel, for example behind a company proxy. |
