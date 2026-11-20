# Jinx

A cooperative word game with personal scores for 3–8 players on
[`@tvpipe/party`](../../npm/party), in English. One player guesses a
secret word from one-word clues the others write on their phones; clues
that are the same (the game decides: same word, plural, verb form, word
family, typo, …) jinx and are lost. The guesser picks whose card to turn,
the pot shrinks with every card, clue-givers can bet on a bold clue, and
the guess is typed live on the TV. The evening ends with the room's
title, the MVP of the night, awards and a web of every word and clue.
The design, with every rule and number: [`docs/jinx-design.md`](../../docs/jinx-design.md).

It is a web app for [`tvpipe game play`](../../docs/playshare-design.md):
tvpipe serves it, the game runs in its host page (a window on the
computer), the TV page is cast to AirPlay and Chromecast receivers, and
phones join with the QR code.

| | | |
|---|---|---|
| `tvpipe.json` | the manifest | pages, 3–8 players, the TV's size; `root: dist` |
| `game/` | the rules (`rules.ts`), the clue matcher (`match.ts`), the game (`game.ts`), bots (`bots.ts`), the views (`views.ts`: the contract between host, TV and phones) | TypeScript on `@tvpipe/party`, tested in Node |
| `host/` | the host page | the party, the TVs, the players, settings (rounds, writing time, words), a muted preview of the TV |
| `screen/` | the TV page | the table seen from above |
| `phone/` | the phone page | faces, clues, cards, the guess |
| `data/words.txt`, `data/dict.txt` | the secret words (with difficulty, category, accepted guesses, banned clues), the dictionary for typos | fetched by the host page; `design/` has how they were made |
| `CREDITS.md` | the word list's sources | |

## Running it

```sh
cargo build --release -p tvpipe-cli         # tvpipe
(cd npm/party && npm ci && npm run build)
cd games/jinx && npm install && npm run build
../../target/release/tvpipe game play .     # or: tvpipe game play games/jinx
npm run zip                                 # jinx.zip (tvpipe game pack)
```

Pick the TVs in tvpipe's launcher, then Start: the window becomes the host
page. Scan the QR code on the TV; the first player is the VIP and starts from their phone once three
are in (or the host page's Start), and offers "Play again" at the end.

Settings: rounds (auto: every player guesses twice with 3–5 players, once
with 6–8; or once, or twice), writing time (30/45/60/90 s), words (easy,
normal: mostly easy and medium with one hard word in ten, hard: medium
and hard). The room deals no word twice until it dealt them all (kept in
the host page's `localStorage`) and never two related words in one game.

## Tests

`npm test`: the matcher's known-answer table (`test/match.test.ts`), the
rules (`test/rules.test.ts`: pot, payouts, dealing, guesser order, titles,
awards, the web), and whole games with bots on `@tvpipe/party`'s relay
in memory (`test/game.test.ts`): 3 and 4 players, play again, clue
refusals and replacing, all clues jinxed, wrong guesses and passes, timers
for a silent or disconnected guesser, a late joiner, a player who leaves,
a guesser who leaves, the secrecy of the word on the TV and the guesser's
phone.

`node test/live/play.ts [--bots N] [--rounds auto|1|2] [--out DIR]` (after
`npm run build`; not part of `npm test`): a whole game on the real `tvpipe
game play` with headless Chrome as the host window, the TV and a phone, bots
for the others; screenshots of every phase, layout checks, and checks that
no phone shows a jinx or the word before the TV does. Bots write the
word's category or another word from the word list, so they jinx now and
then, and they peek at the word to guess. They are for tests: the host
page's bot buttons show only with `?bots` in its URL.
