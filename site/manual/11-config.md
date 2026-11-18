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

## Environment variables

| Variable | Effect |
|---|---|
| `TVPIPE_CONFIG_DIR` | The config directory. |
| `TVPIPE_FFMPEG` | The ffmpeg to use. |
| `TVPIPE_BROWSER` | The browser for `share URL`. |
