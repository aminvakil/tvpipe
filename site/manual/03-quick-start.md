---
title: Quick start
summary: Find a TV, pair it once and play a file.
group: Start
---

Find a TV, pair it once and play a file.

## First steps

:::aside Network
The computer and the TV need to be on the same network.
:::

:::steps
### Find receivers
List the TVs on your network:

```sh
$ tvpipe discover
NAME          ADDRESS       PROTOCOLS             MODEL
Living Room   192.168.1.20  airplay(mirror),raop  AppleTV6,2
Kitchen       192.168.1.31  cast                  Chromecast
```

### Pair once (Apple TV only)
The first time, an Apple TV shows a PIN. Type it when tvpipe asks for it. Cast devices need no pairing.

```sh
$ tvpipe pair "Living Room"
```

### Play
Names are not case sensitive, and the start of a name is enough, so `liv` works too.

```sh
$ tvpipe play --to "Living Room" movie.mp4
```

### Stop
Press `Ctrl-C`, or type `q` and press Enter. The TV leaves mirroring mode. A second `Ctrl-C` quits at once.
:::

## Next

- [Send to two TVs](receivers.md) by repeating `--to`.
- [Share the screen or a browser tab](screen.md) instead of a file.
- [Put a web page on the TV](web-page.md).
