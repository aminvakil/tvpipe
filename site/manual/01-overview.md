---
title: Overview
summary: What tvpipe does and how it works.
group: Start
spec:
  Platforms: macOS; Linux; Windows
  Receivers: AirPlay; Google Cast; Browsers
---

tvpipe is a command line tool that puts video and sound from your computer on a TV. It sends to Apple TV, Chromecast, other AirPlay and Google Cast receivers, and browser tabs.

## What it does

- Plays a video file or a URL on the TV.
- Mirrors a camera, a capture card or the screen.
- Shows a browser tab, a window or the whole screen.
- Puts a web page on the TV, such as a dashboard.
- Takes frames and sound from a game.
- Sends to several TVs at once.
- Runs party games that everyone plays with their phone.

## How it works

:::aside Protocols
**AirPlay.** Screen mirroring, the way an iPhone mirrors its screen.

**Google Cast.** Cast Streaming, the way Chrome casts a tab.
:::

tvpipe uses the screen mirroring protocols of AirPlay and Google Cast. The TV does not download anything by itself. tvpipe encodes the picture and the sound and sends them to the TV while they play.

:::figure Every receiver gets the same stream.
<svg class="diagram" viewBox="0 0 720 268" role="img" aria-label="Sources go into tvpipe, which sends to AirPlay, Cast and browser receivers">
<text class="s" x="0" y="12">SOURCES</text><text class="s" x="268" y="12">TVPIPE</text><text class="s" x="520" y="12">RECEIVERS</text>
<rect class="box" x="0.5" y="28.5" width="190" height="44"/><text class="t" x="14" y="55">File, URL, camera</text>
<rect class="box" x="0.5" y="88.5" width="190" height="44"/><text class="t" x="14" y="115">Browser tab or page</text>
<rect class="box" x="0.5" y="148.5" width="190" height="44"/><text class="t" x="14" y="175">Screen</text>
<rect class="box" x="0.5" y="208.5" width="190" height="44"/><text class="t" x="14" y="235">Game</text>
<path class="ln" d="M191 50.5 H229.5 V230.5 H191 M191 110.5 H229.5 M191 170.5 H229.5 M229.5 140.5 H268"/>
<rect class="core" x="268" y="28" width="190" height="225"/><rect class="ac" x="268" y="28" width="4" height="225"/>
<text class="t core-t" x="286" y="80">Read or capture</text>
<text class="t core-t" x="286" y="145">Encode</text>
<text class="t core-t" x="286" y="210">Send</text>
<path class="ln" d="M458 140.5 H489.5 M489.5 50.5 V230.5 M489.5 50.5 H520 M489.5 140.5 H520 M489.5 230.5 H520"/>
<rect class="box" x="520.5" y="28.5" width="199" height="44"/><text class="t" x="534" y="55">Apple TV, AirPlay TVs</text>
<rect class="box" x="520.5" y="118.5" width="199" height="44"/><text class="t" x="534" y="145">Chromecast, Google TV</text>
<rect class="box" x="520.5" y="208.5" width="199" height="44"/><text class="t" x="534" y="235">Browser tab, OBS</text>
</svg>
:::

## What it does not do

- Receive. tvpipe only sends.
- Tell the TV to play a URL by itself.
- Play sound alone on HomePods or speaker groups.

## Sections of this manual

:::index
:::
