---
title: Sharing the screen
summary: A whole screen or one window, with the computer's sound.
group: Use
spec:
  Command: `screen`
  Needs: Chrome, Edge, Chromium or Brave; or ffmpeg, without sound
---

`tvpipe screen` puts your screen or a single window on the TV, with the sound the computer plays.

## Sharing

```sh
$ tvpipe screen --to TV                      # the main screen
$ tvpipe screen --to TV --display 2          # the second screen
$ tvpipe screen --to TV --window "Slides"    # a window whose title contains "Slides"
$ tvpipe screen --to TV --pick               # choose in the browser's picker
```

tvpipe starts a browser of its own for this, with a separate profile. Its small window appears for a moment and is then minimized. It records and encodes the picture, so the computer stays quick.

:::aside Stopping
`Ctrl-C`, `q` and Enter, or **Stop sharing** in the browser's bar.
:::

The browser shows a bar at the bottom of the screen while it shares. When you share the screen, the bar is in the picture: press **Hide**.

## Permissions

:::aside macOS
System Settings, Privacy & Security, Screen & System Audio Recording, then your browser.
:::

On macOS the browser needs permission to record the screen and the sound. The first time, macOS asks for it. If the TV stays black or silent, allow your browser in System Settings and run tvpipe again.

## Sound

| | Screen | Window |
|---|---|---|
| macOS 14.2 or newer | everything the computer plays | that app's sound |
| Windows 11 | everything | that app's sound |
| Windows 10 | everything | everything |
| Linux | everything | everything |

`--no-audio` sends the picture only. If you watch with `--to browser` on the same computer, mute that tab, or the sound comes back as an echo.

## Choosing a screen or window

`--display 2` picks the browser's "Screen 2". On a Mac set to another language, give the name the browser's picker shows, for example `--display "Bildschirm 2"`.

`--window` takes any part of the window's title, as shown in its title bar. Capital letters matter. If nothing matches, the browser's window comes to the front with its picker: choose there.

On Linux with Wayland the desktop asks which screen or window to share. Choose it there.

## Without a browser

`--engine ffmpeg` records with ffmpeg instead, and tvpipe does this by itself when no browser is installed. It sends no sound on macOS and Windows. It shares windows only on Windows and on Linux with X11.
