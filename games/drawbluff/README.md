# drawbluff

A Drawful-style party game (our own name, prompts and look) on
[`@tvpipe/party`](../../npm/party): 3–50 players in one mode, everyone
draws a secret prompt on their phone, others invent fake titles for each
drawing, everyone votes for the real one. Points for finding the truth
(1000), for the artist per player who found it (1000), and for every
player fooled by your lie (500); round 2 counts double.

Up to 8 players it is the classic game: every drawing on the TV, every
other player lies. From 9 the round has six spotlight drawings (the rest
in a gallery on the TV, a page of eight at a time), at most seven liars
per drawing, rotating so everybody lies about as often, and from 13 the
next drawing's liars write while the TV shows the current vote. Every
phone gets at most six options, sampled so each lie is shown about
equally often, and the artist's and the liars' points are per exposure
(× min(1, 7 / voters who saw it)), so a big room does not inflate them
and the spotlight draw does not decide the game. Players can join
mid-game, as the VIP (or the host page) sets it in the lobby: from the
next round (`wait`, the default), from the next drawing (`open`, "join
anytime" on the TV) or not at all (`closed`); every phone shows its role and
its rank ("#12 of 50"). The design and the numbers:
[`docs/drawbluff-scale.md`](../../docs/drawbluff-scale.md).

It is a web app for [`tvpipe playshare`](../../docs/playshare-design.md):
tvpipe serves it, the game runs in its host page (a window on the
computer), the TV page is cast to AirPlay and Chromecast receivers, and
phones join with the QR code.

| | | |
|---|---|---|
| `tvpipe.json` | the manifest | pages, 3–50 players, the TV's size; `root: dist` |
| `game/` | the rules (`rules.ts`), the game (`game.ts`), bots (`bots.ts`) | TypeScript on `@tvpipe/party`, tested in Node |
| `host/` | the host page | the party, the TV, the players, bots, settings, a preview of the TV |
| `screen/` | the TV page | the screen view (`{ roster, screen, join }`) |
| `phone/` | the phone page | drawing, lies, votes |
| `data/prompts.txt` | the prompts | fetched by the host page |

## Running it

```sh
cargo build --release -p tvpipe-cli         # tvpipe (or a release build of it)
(cd npm/party && npm ci && npm run build)
cd games/drawbluff && npm install && npm run build
../../target/release/tvpipe playshare .     # or: tvpipe playshare games/drawbluff
npm run zip                                 # drawbluff.zip: tvpipe playshare drawbluff.zip
```

tvpipe opens a window with its launcher: pick the TVs (or "A browser tab
on this computer"), then Start. The window becomes the host page: the
room code and QR code, the TVs, the players, rounds and timers, and who
may join during a game (from the next round, anytime, or not at all).
"Add a bot" adds fake players to try it alone. Scan the QR code on the
TV; the first player is the VIP and starts the game from their phone
once three are in (or the host page's Start). Closing the window ends
it all. `tvpipe playshare --help`: tunnels (`--tunnel=opentunnel`,
`--no-tunnel`), the LAN (`--lan`), `--window`.

## Tests

`npm test`: the rules and scoring (the rule vectors in
`test/vectors/rules.txt`, written by `node test/vectors.ts --bless`),
whole games with 3, 8 and 50 bots on `@tvpipe/party`'s relay in memory
(a player who drops mid-game and comes back, one who leaves, too few
players, late joiners in every mode, the VIP removing a player).
`npm run build && node test/live/playshare.ts [--zip] [--bots N]
[--rounds N] [--tunnel=PROVIDER]` runs the real thing: `tvpipe
playshare` on the bundle (or the zip), a headless Chrome as the window
(the launcher, the host page), as a phone (it joins, draws with touch
events, lies and votes) and as the TV (the `browser` receiver's player
page: the TV page went through tvpipe's own headless Chrome and the
stream), bots through the phones' listener, and screenshots of the
launcher, the host window, the TV and the phone per phase.
