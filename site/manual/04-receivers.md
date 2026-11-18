---
title: Receivers
summary: Discovery, pairing and choosing receivers with --to.
group: Use
spec:
  Commands: `discover`; `pair`, `unpair`
  Option: `--to`
---

A receiver is anything tvpipe streams to: an Apple TV or AirPlay TV, a Chromecast or Google TV, or a browser tab on the computer it runs on.

## Discovering receivers

:::aside Specification
`--timeout SECS` How long to browse. Default 3.

`--json` Machine-readable output.

`--interface ADDR` Also browse from this local address. Repeatable.
:::

`tvpipe discover` browses the network with mDNS and lists every AirPlay and Cast receiver it finds, with the protocols each one offers.

```sh
$ tvpipe discover
NAME          ADDRESS       PROTOCOLS             MODEL
Living Room   192.168.1.20  airplay(mirror),raop  AppleTV6,2
Kitchen       192.168.1.31  cast                  Chromecast
```

You do not have to run it before streaming: `--to` with a name discovers on its own. It shows which names are available.

## Choosing receivers with `--to`

:::aside Both protocols
Some TVs speak both AirPlay and Cast. tvpipe uses Cast for them unless you write `airplay:`.
:::

| Form | Meaning |
|---|---|
| `"Living Room"`, `liv` | A discovered name, case-insensitive. A unique prefix is enough. |
| `192.168.1.20` | Cast on port 8009, without discovery. |
| `192.168.1.20:7000` | AirPlay on port 7000, without discovery. |
| `airplay:NAME`, `cast:NAME` | Force a protocol. |
| `browser` | A player page in a browser tab on this computer. See [Playing to a browser](08-browser.md). |

## Several receivers at once

:::aside Sync
Receivers share a timeline and a playout delay, so they show the same frame at about the same time. They are not frame-locked to each other.
:::

Repeat `--to`. tvpipe encodes once and sends the same stream to every receiver.

```sh
$ tvpipe play --to "Living Room" --to Kitchen --to browser movie.mp4
```

When AirPlay and Cast receivers are mixed, both use a 300 ms playout delay so they stay together. See [Test pattern and latency](09-latency.md).

## Pairing with an Apple TV

:::caution
Pairings are stored in `credentials.txt` in the [config directory](12-config.md). The file holds a private key. Do not share it.
:::

AirPlay receivers are paired once. `tvpipe pair` asks the TV to show a PIN and then asks you for it; `--pin` gives it up front.

```sh
$ tvpipe pair "Living Room"
$ tvpipe unpair "Living Room"
```

`unpair` forgets the pairing on this computer. Cast receivers need no pairing.
