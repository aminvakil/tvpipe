---
title: Receivers
summary: Finding TVs, pairing and choosing where to send.
group: Use
spec:
  Commands: `discover`; `pair`, `unpair`
  Option: `--to`
---

A receiver is where tvpipe sends to: an Apple TV or AirPlay TV, a Chromecast or Google TV, or a browser tab on your computer.

## Finding receivers

:::aside Options
`--timeout SECS` How long to look. Default 3.

`--json` Every receiver with its addresses and what each protocol announces, for scripts.

`--interface ADDR` Also look on the network of this local address, for computers on several networks.
:::

```sh
$ tvpipe discover
NAME          ADDRESS       PROTOCOLS             MODEL
Living Room   192.168.1.20  airplay(mirror),raop  AppleTV6,2
Kitchen       192.168.1.31  cast                  Chromecast
```

You do not need to run `discover` before streaming. `--to` with a name finds the receiver by itself.

## Choosing with `--to`

:::aside Both protocols
TVs that speak AirPlay and Cast get Cast, unless you write `airplay:` before the name.
:::

| Form | Meaning |
|---|---|
| `"Living Room"`, `liv` | A name. Not case sensitive, and the start is enough. |
| `192.168.1.20` | A Cast receiver at this address. |
| `192.168.1.20:7000` | An AirPlay receiver at this address. |
| `airplay:NAME`, `cast:NAME` | A name, with the protocol to use. |
| `browser` | A browser tab on this computer. See [Playing to a browser](08-browser.md). |

## Several receivers

Repeat `--to` to send to several receivers at once. They all get the same stream.

```sh
$ tvpipe play --to "Living Room" --to Kitchen movie.mp4
```

## When a TV drops out

:::aside Option
`--reconnect SECS` How long to keep trying. Default 120, `0` turns it off.
:::

If a TV drops out while streaming, for example because it left the Wi-Fi or got a new address, tvpipe connects it again and the other receivers keep playing. A receiver given by name is looked up again, one given as an address is tried at that address. It comes back with the next full picture.

A TV where someone stopped mirroring on purpose stays off.

## Pairing with an Apple TV

:::caution
The pairing is a private key in `credentials.txt` in the [config directory](11-config.md). Do not share that file.
:::

An Apple TV asks for a PIN the first time. `tvpipe pair` makes the TV show it and asks you to type it. `--pin` passes it directly.

```sh
$ tvpipe pair "Living Room"
$ tvpipe unpair "Living Room"
```

`unpair` forgets the pairing on this computer. Cast receivers need no pairing.
