---
title: Quick start
summary: Find a TV, pair it once and play a file.
group: Start
spec:
  Needs: A receiver on the same network
  Covers: `discover`, `pair`, `play`
---

The procedure for putting a file on a receiver for the first time.

## First transmission

:::aside Procedure
The computer and the TV have to be on the same local network. Discovery uses mDNS, which does not cross routers.
:::

:::steps
### Find receivers
List everything on the network that speaks AirPlay or Cast:

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
A name is matched case-insensitively and a unique prefix is enough, so `liv` works too.

```sh
$ tvpipe play --to "Living Room" movie.mp4
```

### Stop
Press `Ctrl-C`, or type `q` and Enter. Either way the TV gets a proper teardown and leaves mirroring mode.
:::

## Next

:::aside Next
:::

- [Stream the test pattern](09-latency.md) to see the delay of your setup.
- [Send to two TVs](04-receivers.md) by repeating `--to`.
- [Share a browser tab](06-share.md) instead of a file.
