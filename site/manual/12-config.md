---
title: Configuration
summary: Files tvpipe keeps, and environment variables.
group: Reference
---

tvpipe has no configuration file. It keeps two files of state and reads a few environment variables.

## Config directory

:::aside Override
`TVPIPE_CONFIG_DIR` uses another directory.
:::

| Platform | Path |
|---|---|
| Linux, other Unix | `$XDG_CONFIG_HOME/tvpipe`, default `~/.config/tvpipe` |
| macOS | `~/Library/Application Support/tvpipe` |
| Windows | `%APPDATA%\tvpipe` |

| File | Contents |
|---|---|
| `credentials.txt` | AirPlay pairings. Holds a private key; keep it private. |
| `player-token` | The browser player’s URL token. `--new-token` replaces it. |

## Environment variables

| Variable | Effect |
|---|---|
| `TVPIPE_CONFIG_DIR` | The config directory. |
| `TVPIPE_FFMPEG` | The ffmpeg binary, if not on `PATH`. |
| `TVPIPE_BROWSER` | The browser for `share URL`. |
