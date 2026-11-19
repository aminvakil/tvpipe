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

## Writing a game

A game is three pages and a manifest:

```json
{
  "name": "quiz",
  "pages": { "host": "host.html", "tv": "tv.html", "phone": "phone.html" },
  "players": { "min": 2, "max": 12 }
}
```

The pages load the `@tvpipe/party` library from tvpipe itself (`/_tvpipe/lib/`), so a game needs no build step and no server. The host page:

```js
import { Tvpipe, PartyError } from "/_tvpipe/lib/index.js";

const tvpipe = await Tvpipe.connect();
const party = await tvpipe.party();     // the room, its link and QR code
await party.cast();                     // the TV page on the TVs picked at the start

party.screen.set({ phase: "lobby" });
party.handle("start", async (data, player) => {
  if (!player.vip) throw new PartyError("not-vip", "Only the VIP can start");
  const deadline = party.now() + 60_000;
  party.room.set({ phase: "answer", deadline });
  const answers = await party.collect("answer", { until: deadline });
  party.screen.set({ phase: "results", answers: [...answers.values()] });
});
```

The phone page:

```js
import { PartyClient } from "/_tvpipe/lib/client.js";

const party = PartyClient.fromLocation();
party.on("view", (view) => render(view));
await party.join("Ann");
await party.act("answer", { text: "a nervous cactus" });
```

The TV page uses the same client. Its view also has the join link and its QR code (`qrSvg(view.join.qr)` draws it).

The game never sends events to phones. It sets what each phone shows: `party.room` for every player, `player.state` for one player, and `party.screen` for the TV. A phone that comes back after its screen was locked gets everything it missed. Actions survive a dropped connection and are applied once.

For a stick or a tilt, `party.input(data)` on the phone sends continuous input: never answered, the latest wins. The host gets it as the `input` event.

Joining mid-game is the game's choice. While `party.inProgress` is set, `party.lateJoin` decides what happens to a new name: `"closed"` refuses it (players who were already in still come back), while `"wait"` (the default) and `"open"` let it in as a waiting player (`player.waiting`). The game deals them in with `party.activate(player)` when it wants. The TV page sees the setting in `join.lateJoin`, so it can say "join anytime" under the QR code.

A game may also bring its own start page instead of tvpipe's (`"launcher"` in `pages`), with the same `Tvpipe` calls: the TVs, pairing, then on to the host page. Games written in TypeScript can use the npm package for its types and for tests: `memoryRelay()` connects a host and fake phones in one process.

```sh
$ npm install --save-dev @tvpipe/party
```

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
