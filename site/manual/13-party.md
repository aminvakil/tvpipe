---
title: Party games
summary: Games on the TV that everyone plays with their phone.
group: Use
spec:
  Command: `tvpipe playshare GAME`
  Package: `@tvpipe/party` (npm, for writing games)
  A game: a folder or a zip with `tvpipe.json`
  Example: `games/drawbluff`
---

`tvpipe playshare` runs games like the ones from Jackbox: the TV shows the game and a QR code, and everyone joins by scanning it with their phone. Nobody installs anything: a game is a small web app, and tvpipe serves it, puts its main screen on the TV and connects the phones.

```sh
$ tvpipe playshare games/drawbluff     # a folder with tvpipe.json
$ tvpipe playshare drawbluff.zip       # or a zip of one
```

:::aside Not released yet
`playshare` and `@tvpipe/party` come out with the next release. Until then, use them from a clone of the repository.
:::

## Playing

1. tvpipe opens a window. It lists the TVs on your network as they answer: Apple TVs and AirPlay TVs, Chromecasts and Google TVs. An Apple TV you have not paired with shows a **Pair** button: the TV shows a code, you type it in the window. "A browser tab on this computer" lets you try a game without a TV.
2. Pick one or more TVs and press **Start**. The window becomes the game's own page, and the TV shows the game with its QR code.
3. Everyone scans the QR code and plays on their phone. Phones that lose the connection (a locked screen, a tunnel hiccup) come back on their own.
4. Close the window (or press Ctrl-C in the terminal) to end it: the TVs leave the game, the phones say the room closed.

:::aside How phones get in
By default through a public link: OpenTunnel, which tvpipe sets up once and reuses, with Cloudflare when OpenTunnel fails. `--lan` serves phones on the same Wi-Fi instead.
:::

| Option | |
|---|---|
| `--tunnel=opentunnel`, `--tunnel=cloudflare` | Who carries the phones' link (default: OpenTunnel, then Cloudflare) |
| `--no-tunnel` | No public link: phones on the Wi-Fi (`--lan`) or this computer only |
| `--lan`, `--lan=IP` | Also serve phones on the Wi-Fi, at `http://IP:7420` |
| `--window=open` | The game's window in your usual browser instead of tvpipe's own |
| `--window=none` | No window: tvpipe prints its link |
| `--browser PATH` | The browser for the window and for rendering the TV |

The QR code holds the link with the room's code and a key, so only people who can see the TV can join. On the Wi-Fi (`--lan`) the page is plain HTTP and the key crosses the network unencrypted: use the public link on networks with strangers. With Cloudflare, Cloudflare can read the game's traffic. With OpenTunnel, nobody else can.

## How it works

:::aside The split
tvpipe serves the game, casts its TV page and passes the phones' messages on. The game, in its own page, keeps the players and the rules.
:::

- The game's **host page** runs in the window: it holds the rules, the players and what everyone sees.
- Its **TV page** runs in a browser tvpipe starts in the background. tvpipe streams that tab, picture and sound, to the TVs, as with [A web page on the TV](web-page.md).
- Its **phone page** loads over the public link. Every phone's connection ends in tvpipe, which hands it to the host page.

## Making a game

A game is a small web app: a host page with the rules, a TV page, a phone page and a `tvpipe.json` that names them. [Making party games](making-games.md) covers writing one, checking it and packing it into a zip.

## Safety

- tvpipe only answers for the game's public link and for this computer, and limits how much and how fast every phone may send.
- The game's own controls (tvpipe's window and its API) are not on the public link at all.
- Names are cleaned up and must be unique. A `moderate` function can refuse words you do not want on the TV.
- The VIP and the game can remove players.

## The demo game

`games/drawbluff` in the repository is a drawing and bluffing game for 3 to 50 players. Build it once, then play:

```sh
$ cd games/drawbluff && npm install && npm run build
$ tvpipe playshare .
$ npm run zip && tvpipe playshare drawbluff.zip
```

Its window shows the room code and the QR code, the TVs, the players, the number of rounds, the timers and whether latecomers join at the next round, at the next drawing, or not at all, and a preview of the TV. **Add a bot** adds fake players, to try it alone. Each round deals prompts of different kinds: odd pairs, idioms taken literally, feelings, "the moment…", the secret lives of things and more.

The design is in [docs/playshare-design.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/playshare-design.md), the protocol between phones and the game in [docs/party-protocol.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/party-protocol.md).
