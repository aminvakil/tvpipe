# Sharing a screen or a window (`tvpipe screen`)

Research and tests from 2026-11-18/19. The goal: mirror a whole monitor or
a single window **with the computer's sound** to AirPlay and Cast, as
conveniently as `tvpipe screen --to TV`, on macOS, Windows and Linux,
without OS code in tvpipe. For users: the manual's "Sharing the screen"
page; the browser pages' protocol: [`share-protocol.md`](share-protocol.md).

## The decision

`tvpipe screen` starts a **Chromium-based browser of its own** (Chrome,
Edge, Chromium or Brave; a temporary profile; Firefox is refused: its
picker cannot be answered by switches) whose page captures with
`getDisplayMedia`, Chrome's picker answered by command-line switches, and
encodes with WebCodecs (hardware) into the existing share pipeline. No OS
API in tvpipe. **ffmpeg** is the fallback (`--engine ffmpeg`, or when no
such browser is installed), with per-OS input arguments built as strings
(`crates/tvpipe/src/media/screencap.rs`). Native capture APIs were assessed and not
taken (they break the platform-neutral rule; see the end).

```sh
tvpipe screen --to TV                       # the main screen and the sound
tvpipe screen --to TV --display 2           # the second screen
tvpipe screen --to TV --window "Keynote"    # a window (title contains ...)
tvpipe screen --to TV --pick                # choose in the browser's picker
tvpipe screen --to TV --engine ffmpeg       # without a browser
```

## How it works

1. `share_rejoin` (the `share` session, `ShareKind::Screen`) binds the
   share server; `browser::screen::ScreenBrowser::start` starts the
   browser (`launch::screen_args`): a small **normal window** (desktop
   capture does not work headless), `--remote-debugging-port=0`, the
   picker switches below, `--system-audio-capture-default_checked`,
   `--enable-features=PulseaudioLoopbackForScreenShare`, `--lang=en-US`
   and the background-throttling switches.
2. The tab opens the server's `/screen?t=TOKEN` page (`screen.html`:
   `capture.js` and a status line). Once the session has resolved the
   receivers and configured the server, tvpipe evaluates `headless.js`
   with capture options (`surface`, `windowAudio: 'window'`,
   `selfBrowserSurface: 'exclude'`, `localAudio: true`) **as a user
   gesture** over the DevTools protocol; it calls `getDisplayMedia` and
   the switch picks the source without showing the picker.
3. Once the page shares, the window is **minimized** (it would be in the
   picture). From then on it is `tvpipe share`: the capture's size plans
   the stream, the receivers connect, the page encodes.
4. The browser's "Stop sharing" ends the session; a failed capture
   (cancelled, no permission) fails it with a hint; when nothing matched
   after 10 s the window comes to the front, where Chrome's picker waits
   for the user (or shows its permission pane). On exit the browser quits
   and the profile is deleted (`launch::sweep_stale_profiles` cleans up
   after a killed tvpipe).

### Chrome's switches

From `chrome/common/chrome_switches.h` and
`chrome/browser/ui/views/desktop_capture/desktop_media_list_controller.cc`
(Chromium main, 2026-11):

| Switch | Effect |
|---|---|
| `--auto-select-screen-capture-source` | accepts the first screen added to the picker (`--display 1`, the default) |
| `--auto-select-desktop-capture-source=S` | accepts the first source of **any** list (screen, window, tab) whose name contains `S` (`--display N` → `Screen N`, `--display NAME`) |
| `--auto-select-window-capture-source-by-title=S` | accepts the first window whose title contains `S` (case-sensitive; `--window`) |
| `--auto-select-tab-capture-source-by-title=S` | the same for tabs (not used) |
| `--system-audio-capture-default_checked` | ticks the picker's "Also share system audio" (the underscore is Chrome's spelling); without it an auto-selected screen has no sound |
| `--auto-accept-this-tab-capture` | `preferCurrentTab` (used by `share URL`, not here) |

Screen names (`native_desktop_media_list.cc`): "Entire screen" with one
screen, "Screen 1", "Screen 2", ... with several, **in Chrome's UI
language**. `--lang=en-US` makes them English on Windows and Linux; on
macOS the system language decides (`--lang` is ignored there, and
`-AppleLanguages` is taken as a URL), hence `--display NAME` for other
languages. Window names are the window titles (macOS: the app's name for
an untitled window that is the app's only one, `thumbnail_capturer_mac.mm`).

### Sound, per OS

From `media/base/media_switches.cc` and
`chrome/browser/media/webrtc/desktop_media_picker_controller.cc`:

| | macOS | Windows | Linux |
|---|---|---|---|
| System sound with a screen (`systemAudio: 'include'`) | 14.2+: Core Audio process taps ("CATap"); 13–14.x: ScreenCaptureKit | WASAPI loopback (always) | PulseAudio monitor, only with `PulseaudioLoopbackForScreenShare` (off by default; tvpipe turns it on; PipeWire's pulse server works too) |
| A window's sound (`windowAudio: 'window'`) | the window's **app** only (14.2+, `ApplicationAudioCaptureMac`, on by default) | the window's **app** only (Windows 11 process loopback, `ApplicationAudioCaptureWin`); Windows 10: system sound | system sound (no app loopback) |
| `restrictOwnAudio` (leave the browser's own sound out) | 14.2+ | Windows 11 | no |

The track's label says which: "System Audio" or "Application Audio".

### Things found by testing

- **macOS attributes screen and system-audio recording to the
  "responsible" app.** A browser started directly by tvpipe counts as a
  child of the terminal: the picture worked (the terminal had Screen
  Recording) but the system sound was **digital silence** (peak 0 with a
  tone playing). Started through LaunchServices (`open -n -g -a
  "Google Chrome.app" --args ...`) the browser is its own app, asks for
  and keeps the permission itself, and the sound came through (peak 0.5).
  `platform::own_app_command` does that on macOS (no OS API: a command).
  Such a browser is no child of tvpipe: it is quit over DevTools
  (`Browser.close`), killed by the pid from `SystemInfo.getProcessInfo`
  if it does not quit, and its stderr goes to a file (`open --stderr`).
- **`getDisplayMedia` needs a visible tab**: it fails with
  `InvalidStateError` in a minimized window or a background tab (an extra
  tab from an unknown argument did it). So: capture first, minimize after.
  Capture and encoding go on minimized (with the background-throttling
  switches; macOS 29.5 fps, Windows 27 fps with something moving).
- **Captures deliver frames when the picture changes** (ScreenCaptureKit,
  Windows Graphics Capture): an idle desktop gave 2–4 fps on Windows.
  The share page repeats the last frame, so receivers still get a steady
  stream.
- **Chrome's "… is sharing your screen [Stop sharing] [Hide]" bar** is
  always shown (`ScreenCaptureNotificationUIViews`, no switch turns it
  off), at the bottom of the main screen, and it is **in the picture** of
  a shared screen. "Hide" removes it. A shared window does not include it.
  It also takes the focus when a screen share starts.
- **Linux in Docker**: Chromium needs `--no-sandbox` there (not on a
  desktop), and window capture needs a window manager (X11 windows
  without `WM_STATE` are not listed).
- A `--to browser` tab on the same computer plays the shared sound, which
  is then shared again (an echo); tvpipe warns.
- Silent captured sound is noted once after 10 s (`share::Sound`, from
  the page's `audioPeak` statistic): that is how the missing macOS
  permission shows.

## What works where

| | macOS 26.6 (M5, Chrome 157) | Windows 11 ARM64 VM (Edge 157) | Linux X11 (Docker: Xvfb, openbox, PulseAudio, Chromium 156) | Linux Wayland |
|---|---|---|---|---|
| `screen` (main screen) + sound | yes: 3600x2338 (Retina), 29.8 fps, "System Audio" (a test tone measured in it) | yes: 3600x2182, "System Audio" (a test tone measured in it) | yes: 28 fps, system sound through the PulseAudio monitor | untested: the desktop portal's dialog is the picker (the user picks; switches cannot answer it) |
| `--window TITLE` + sound | yes: "Application Audio" (the window's app, its tone measured), 29.4 fps | yes: "Application Audio" (not silent), 25 fps | yes (system sound) | as above |
| `--display 2` | untested (one screen) | untested | untested | — |
| `--pick` | picker in the window | — | waits for a pick (nobody clicked) | — |
| `--engine ffmpeg` screen | yes: avfoundation, no sound | yes: ddagrab, no sound | yes: x11grab + `@DEFAULT_MONITOR@` | refused (x11grab) |
| `--engine ffmpeg --window` | refused | gdigrab `title=` (untested) | yes: x11grab `-window_id` | refused |
| `--to browser` live check | yes (decoded, 29 fps, audio running) | yes | yes | — |

TVs (AirPlay, Cast) were not tried yet: the stream is the share
pipeline's, which is tested with them.

## The ffmpeg fallback

`media::screencap::ffmpeg_input` (OS as a string, helper programs'
output passed in, so every OS's arguments are tested everywhere):

| OS | screen | window | sound |
|---|---|---|---|
| macOS | `-f avfoundation -capture_cursor 1 -framerate 30 -i "Capture screen N-1:none"` | — | none: avfoundation records inputs only; needs a virtual device (BlackHole) |
| Windows | `-f lavfi -i ddagrab=output_idx=N-1:framerate=30:draw_mouse=1,hwdownload,format=bgra` | `-f gdigrab -i title=TITLE` (the whole title) | none: ffmpeg has no WASAPI loopback (dshow "Stereo Mix" where the driver has one) |
| Linux X11 | `-f x11grab -video_size WxH -i $DISPLAY+X,Y` (from `xrandr --listmonitors`) | `-window_id` from `xwininfo -root -tree` | `-f pulse -i @DEFAULT_MONITOR@` |

Wayland is refused (x11grab sees only X11 clients; kmsgrab needs
`CAP_SYS_ADMIN`). With several `-i`, `Source::Args` now takes the sound
from the last input (it took the first input's only).

## Native capture APIs (assessed, not taken)

Each breaks the platform-neutral rule and needs the owner's approval;
rough sizes for hand-written FFI without crates:

| OS | APIs | Size (estimate) | Notes |
|---|---|---|---|
| macOS | ScreenCaptureKit (`SCShareableContent`, `SCStream` with `capturesAudio`), Core Audio taps for one app's sound | 1500–2500 lines: Objective-C runtime calls (`objc_msgSend`), blocks ABI for completion handlers, CMSampleBuffer/CVPixelBuffer | tvpipe is no app bundle: the permissions go to the terminal (the problem above, without the `open` way out) |
| Windows | Windows.Graphics.Capture (WinRT, D3D11 frame pool, staging-texture readback) or DXGI Desktop Duplication; WASAPI loopback, process loopback (`ActivateAudioInterfaceAsync`) for a window's app | 2000–3000 lines of COM vtables | no permission prompts |
| Linux | xdg-desktop-portal ScreenCast over D-Bus (a D-Bus client), PipeWire's native protocol or `dlopen` of libpipewire, SHM/DMA-BUF frames; PulseAudio/PipeWire for sound | 3000–4000 lines | the portal dialog stays (Wayland requires it) |

All of them deliver raw frames: they would need the built-in encoder (CPU,
Baseline) or ffmpeg fed raw frames, where Chrome gives hardware H.264 for
free. Together ~7–10k lines of unsafe, per-OS code to test on three
systems, for what the browser already does. Worth revisiting only if a
browser-free path with sound matters more than the rule.
