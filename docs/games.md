# Web games for `tvpipe playshare`

How to build a party game that `tvpipe playshare` runs (the TV shows the
game, everyone plays on their phone's browser), check it, pack it into a
zip, and how to run one. The design behind it:
[`playshare-design.md`](playshare-design.md); the library:
[`npm/party/README.md`](../npm/party/README.md); the protocol between
phones and the game: [`party-protocol.md`](party-protocol.md); a whole
game: [`games/drawbluff`](../games/drawbluff/).

Contents: [Building](#building-a-game) (start, layout, `tvpipe.json`,
pages, the library, the dev loop, assets, checking and packing,
versions) · [Running](#running-a-game) (start, the launcher, TVs and
AirPlay PINs, phones and links, ending, troubleshooting).

```
 this computer                                          TVs (AirPlay, Cast)
 ┌──────────────────────────────────────────────────┐        ▲
 │ window: launcher → host page (the rules)          │        │ cast
 │ headless browser: TV page ───────────────────────────────────┘
 │ tvpipe playshare: serves the game, relays phones  │◀── phones (tunnel or Wi-Fi)
 └──────────────────────────────────────────────────┘
```

A game is a **web app**: static files, no server code. tvpipe serves
them, opens the **host page** in a window of its own browser (the game's
rules run there), renders the **TV page** in a headless browser and
casts it, and relays the phones' WebSockets (from the **phone page**) to
the host page. Rooms, players, reconnects and views live in the browser
library `@tvpipe/party`, which tvpipe serves itself.

---

# Building a game

## Start

```sh
tvpipe bundle --init my-game     # a starter game: a buzzer, about 250 lines
tvpipe playshare my-game         # play it
```

The starter is plain JavaScript, no build step: a host page that counts
rounds and points, a TV page with the QR code and who buzzed first, and a
phone page with a join form and a big button. Change it from there.

## Layout

The smallest game is a folder with `tvpipe.json` and three pages:

```
my-game/
  tvpipe.json
  style.css             shared files: anywhere in the folder
  host/index.html       the rules (the window on this computer)
  host/host.js
  tv/index.html         what the TV shows
  tv/tv.js
  phone/index.html      what phones show
  phone/phone.js
```

Every file below the served folder is public (phones need to load the
pages; the host page is code, not a secret). Names starting with `.` are
never served, and `/_tvpipe/` belongs to tvpipe.

**With a build step** (TypeScript, a bundler), keep the sources next to
the output and point `root` at the output; only `root` is served and
packed:

```
drawbluff/
  tvpipe.json           "root": "dist"
  package.json, src/, host/host.ts, …   (not served)
  dist/                 the build: host/, screen/, phone/, …
```

## `tvpipe.json`

```json
{
  "name": "my-game",
  "title": "My game",
  "tvpipe": "0.2",
  "root": "dist",
  "pages": {
    "host": "host/index.html",
    "tv": "tv/index.html",
    "phone": "phone/index.html",
    "launcher": null
  },
  "players": { "min": 2, "max": 12 },
  "tv": { "width": 1280, "height": 720 },
  "limits": { "message": 262144, "rate": 20, "burst": 50 }
}
```

| Field | | Meaning |
|---|---|---|
| `name` | required | 1–40 of `a-z`, `0-9`, `-`: logs, the zip's folder and file name |
| `title` | optional | the launcher's heading (default: `name`), at most 80 characters |
| `tvpipe` | recommended | the tvpipe (and so `@tvpipe/party`) the game is made for, `"0.2"`; see [Versions](#versions) |
| `root` | optional | the served folder, relative to `tvpipe.json` (default: its own folder) |
| `pages.host` | required | the host page, relative to `root` |
| `pages.tv` | required | the TV page |
| `pages.phone` | required | the phone page; `/` on the phones' link redirects to it |
| `pages.launcher` | optional | the game's own launcher instead of tvpipe's ([Pages](#the-pages)) |
| `players.min`, `players.max` | optional | shown in the launcher; `max` (default 8, at most 1000) sizes the relay's connection limit and is the library's default `maxPlayers` |
| `tv.width`, `tv.height` | optional | the TV page's viewport (160–3840 × 120–2160; default: the receivers' shape) |
| `limits.message` | optional | the largest message from a phone, 1 KiB–1 MiB (default 256 KiB; larger: closed with 1009) |
| `limits.rate`, `limits.burst` | optional | messages per second per phone (1–200, default 20) and at once (default 50); more: closed with 4008 |

Paths use `/`, stay inside `root` (no `..`, no leading `/`, no hidden
names). Fields tvpipe does not know are ignored (`tvpipe bundle` warns
about them: usually typos), so `description` or your own data can go
there. A wrong or missing required field stops `tvpipe playshare` with a
message naming it.

## The pages

| Page | Runs | Opened with | Uses |
|---|---|---|---|
| host | the window on this computer (tvpipe's own browser, an app window) | after the launcher, at `pages.host` | `Tvpipe` and `Party` from `/_tvpipe/lib/index.js` |
| TV | a headless browser on this computer, cast to the TVs | `party.links.tv`: `pages.tv#r=CODE&k=SCREENKEY&role=screen` | `PartyClient` from `/_tvpipe/lib/client.js` |
| phone | each player's phone | `party.links.join`: `https://HOST/?r=CODE#k=JOINKEY` (the QR code) | `PartyClient` |
| watch | anywhere (a preview in the host page, a second screen) | `party.links.watch`: `pages.tv#r=…&k=WATCHKEY&role=watch` | the TV page, read-only |
| launcher | the window, before the host page | `pages.launcher`, if set | `Tvpipe` |

- The **host page** keeps the game: it connects to tvpipe's API
  (`Tvpipe.connect()`, the window's session cookie does the
  authentication), makes the party (`tvpipe.party()`: the room code,
  keys, join link and QR code, the relay), casts the TV page
  (`party.cast()`) and then runs rounds. Reloading it starts a new
  party (a new room code and QR code): the phones have to join again.
- The **TV page** only draws `view.screen` (what the host sets) and
  `view.join` (the code, link and QR code). It renders at `tv` size,
  with sound; a browser of tvpipe's own allows autoplay. Keep it to one
  screen without scrolling.
- The **phone page** joins with a name (`client.join(name)`), or
  reconnects (`client.connect()`) when the phone played before, then
  shows `view.room` and `view.me` and sends actions. Make it work on a
  small touch screen, in portrait.
- A **launcher of your own** (rarely needed) may call any `Tvpipe`
  method; it ends with `tvpipe.select(names)` and a navigation to the
  host page. tvpipe's standard one lists the TVs, pairs AirPlay TVs that
  show a PIN, and offers "a browser tab on this computer".

## Using `@tvpipe/party`

tvpipe serves the library at `/_tvpipe/lib/` (`index.js` for host pages,
`client.js` for the TV and phone pages), always the version that matches
the running tvpipe. Import it from there; do not copy it into the game.

The host page:

```js
import { Tvpipe, PartyError } from "/_tvpipe/lib/index.js";

const tvpipe = await Tvpipe.connect();       // fails outside tvpipe playshare
const party = await tvpipe.party();          // { maxPlayers, lateJoin, moderate, … }
if ((await tvpipe.selection()).length > 0) await party.cast();
tvpipe.on("cast", (s) => showTvState(s));    // starting, streaming, failed, stopped

party.screen.set({ phase: "lobby" });        // the TV's view.screen
party.handle("start", (data, from) => {
  if (from.role !== "player" || !from.vip) throw new PartyError("not-vip", "Only the VIP starts");
  void round();
});

async function round() {
  party.inProgress = true;
  const deadline = party.now() + 60_000;
  party.room.set({ phase: "answer", deadline });          // every phone's view.room
  const answers = await party.collect("answer", { until: deadline });
  party.screen.set({ phase: "results", answers: [...answers.values()] });
}
endButton.onclick = () => tvpipe.quit();     // ends tvpipe: TVs stop, phones are told
```

The TV page:

```js
import { PartyClient, qrSvg } from "/_tvpipe/lib/client.js";

const client = PartyClient.fromLocation();   // room, key and role from the link
client.on("view", ({ roster, screen, join }) => {
  qrBox.innerHTML = qrSvg(join?.qr);         // the join link's QR code
  draw(screen, roster);
});
await client.connect();
```

The phone page:

```js
import { PartyClient } from "/_tvpipe/lib/client.js";

const client = PartyClient.fromLocation();
client.on("view", ({ roster, room, me }) => render(room, me));
client.on("bye", (reason, message) => showEnd(message ?? reason));
if (client.canResume) await client.connect();
else form.onsubmit = () => client.join(nameInput.value).catch((e) => show(e.message));
button.onclick = () => client.act("answer", { text: input.value }).catch((e) => show(e.message));
```

The rules of thumb: the host **sets views** (`party.room`, `party.screen`,
`player.state`), it never sends events to keep phones in sync; phones
**send actions** (acknowledged, resent after a reconnect, applied once);
render everything players type as text (`textContent`), never as HTML.
The full API is in [`npm/party/README.md`](../npm/party/README.md).

**TypeScript and bundlers.** Install the package for its types
(`npm install --save-dev @tvpipe/party`), then make the built files
import tvpipe's copy: rewrite `"@tvpipe/party"` to
`"/_tvpipe/lib/index.js"` and `"@tvpipe/party/client"` to
`"/_tvpipe/lib/client.js"` (drawbluff's `scripts/build.ts` does it in
ten lines), mark them external in a bundler, or put an import map in
each page:

```html
<script type="importmap">
{ "imports": { "@tvpipe/party": "/_tvpipe/lib/index.js", "@tvpipe/party/client": "/_tvpipe/lib/client.js" } }
</script>
```

Browsers do not resolve bare names (`"@tvpipe/party"`) or add file
extensions (`"./rules"`), and they do not run TypeScript: ship `.js`
with `type="module"` scripts.

**Tests** run in Node without tvpipe: `memoryRelay()` from
`@tvpipe/party/testing` connects a `Party` and `PartyClient`s in memory
(drawbluff's `test/` has examples, bots included).

## The dev loop

```sh
tvpipe playshare ./my-game                    # the window opens with the launcher
tvpipe playshare ./my-game --no-tunnel        # no public link: this computer only
tvpipe playshare ./my-game --window open      # the host page in your own browser (DevTools)
```

- A folder is read on every request: edit a file and reload the page.
  Reloading the host page starts a new party (it casts again, phones
  join again); a phone or TV page can be reloaded on its own.
- Pick **"A browser tab on this computer"** in the launcher to see the TV
  page without a TV; the host page's preview (`party.links.watch` in an
  `<iframe>`) shows it too.
- Open the join link on this computer (`http://127.0.0.1:PORT/?r=…#k=…`,
  shown by the host page) in a few tabs to be several players; a phone
  on the same Wi-Fi works with `--lan`.
- `--window open` uses your browser and its DevTools; tvpipe then ends
  30 s after the last page closed. `-v` prints more of what happens.
- `TVPIPE_PLAYSHARE_WEB=DIR` serves `/_tvpipe/` from a folder (working on
  the library itself: `npm run build` in `npm/party`).

## Assets and offline play

- Load everything with relative paths (`style.css`, `../img/a.png`) or
  paths from the root (`/img/a.png`). Mind the case: a folder on macOS or
  Windows forgives `Logo.png` for `logo.png`, a zip and Linux do not.
- Put fonts, sounds and libraries in the game. Phones on `--lan` and TVs
  may have no internet, and a party at a cabin should still work:
  `tvpipe bundle` warns about every file loaded from `http(s)://`.
- `fetch()` with URLs built at run time is not checked by `tvpipe
  bundle`; `--smoke` notices if they fail while the pages start.
- A bundle holds at most 10 000 files and 256 MiB unpacked; phones load
  what their page needs, so keep the phone page light (warnings above
  32 MiB per file).

## Checking and packing

```sh
tvpipe bundle --check ./my-game          # check a folder, write nothing
tvpipe bundle ./my-game                  # check, then write my-game.zip here
tvpipe bundle ./my-game -o out/my-game.zip --smoke
tvpipe bundle --check my-game.zip        # check a zip (someone else's, or before sharing)
tvpipe bundle --check --strict ./my-game # warnings fail too: for CI
```

What it checks (errors stop it with exit code 1, warnings do not):

| | Errors | Warnings |
|---|---|---|
| `tvpipe.json` | missing, not JSON, a wrong or missing field (as `tvpipe playshare` would refuse it), a `tvpipe` version this tvpipe cannot run | unknown fields and pages (typos), no `tvpipe` version |
| the folder | `root` missing ("build the game first"), outside the folder, or empty; symlinks that leave `root` (never served) or are broken or loop; names with backslashes or control characters, or not UTF-8; a `_tvpipe` folder; more than 10 000 files or 256 MiB | names Windows cannot unpack (`<>:"\|?*`, a trailing dot or space); `node_modules` in `root`; files over 32 MiB |
| a zip | not a zip, zip64, encryption, other methods, CRC errors; unsafe names (`..`, absolute, drive letters); a name twice; no `tvpipe.json` at the top or in its one top-level folder (with a hint for a folder too deep) | files outside `root` (never served) |
| names | two files or folders that differ only in case (one is lost on macOS and Windows) | |
| the pages | an entry page missing (and why: the case, an unbuilt `root`) | an entry page that is not `.html` |
| what pages load | a missing file, with the page and line: `<script src>`, `<link href>`, `<img src/srcset>`, media, `<iframe>`, CSS `url()` and `@import`, JS `import`/`export from`/`import("…")`, inline scripts and styles; a path above the root; a hidden file; a name that differs in case; an import without its extension; TypeScript or JSX loaded as a script; a module with `import` loaded as a classic script; a bare import (`"@tvpipe/party"`) no import map resolves; a `/_tvpipe/` path tvpipe does not serve | a file loaded from the internet; a missing `<a href>` target; `<base href>` |
| `@tvpipe/party` | | a host page that never loads `/_tvpipe/lib/index.js` (or `tvpipe.js`); a TV or phone page without the client (not the TV page when the game streams a canvas: `tvpipe.surface`, `SurfaceEncoder`); a copy of the library inside the game |

Finally the result is opened the way `tvpipe playshare` opens it.

The zip is **deterministic**: the same files give the same bytes
(entries sorted, every time 1980-01-01, modes `rw-r--r--`, deflate or
stored, whichever is smaller). It holds one folder named after the game
with `tvpipe.json` (without `root`) and the files of `root`, so it
unpacks into a tidy folder and `tvpipe playshare` reads it as it is.
Hidden files (`.DS_Store`, `.git`) are left out, and so is the zip itself
when it is written inside the game. A zip made by hand works too
(`tvpipe playshare` ignores Finder's `__MACOSX/` and hidden files beside
the game's folder), but `tvpipe bundle DIR` checks it and leaves them out.

**`--smoke`** runs the game once, without TVs or a tunnel, in a headless
Chromium-based browser (Chrome, Edge, Chromium, Brave; `--browser PATH`
or `$TVPIPE_BROWSER`): the launcher, the host page (with a stand-in TV
chosen; it must attach the relay), the TV page it casts, the phone page
at the join link, and a fake phone that joins. Uncaught exceptions and
failed requests are errors, `console.error` and failed requests to the
internet warnings. It takes about ten seconds:

```
$ tvpipe bundle my-game --smoke
My game (my-game): 12 files, 9.3 KiB; 0 errors, 0 warnings
wrote my-game.zip (6.1 KiB, 9.3 KiB unpacked)
smoke: tvpipe's launcher loaded
smoke: the host page attached the relay
smoke: the TV page loaded (/tv/index.html)
smoke: the phone page loaded
smoke: a phone joined room SHRM as Smoke (1 in the roster)
smoke: ok
```

A host page that only casts after a button press gets a warning (the TV
page is then opened without its link); a game whose join link is not
`party.links.join` skips the fake phone.

## Versions

`"tvpipe": "0.2"` says which tvpipe the game is made for, and so which
`@tvpipe/party` it uses: tvpipe serves its own copy at `/_tvpipe/lib/`,
and the host page talks to tvpipe's API with it. `tvpipe playshare`
refuses a game made for a newer tvpipe ("update tvpipe"), and, while
tvpipe is 0.x, one made for another 0.x series ("update the game");
from 1.0 on, the same major version. Without the field nothing is
checked (`tvpipe bundle` warns).

When tvpipe moves to a new series: read the library's changelog, update
the game, run its tests and `tvpipe bundle --smoke`, then set the new
version. The phone protocol has its own version (`hello.v`, 1); the
library on both sides is always tvpipe's, so it matches by itself.

Version your game however you like (a `version` field in `tvpipe.json`
is ignored by tvpipe); the zip's bytes change only when its files do, so
a hash of the zip identifies a release.

---

# Running a game

```sh
tvpipe playshare drawbluff.zip            # a zip
tvpipe playshare ./drawbluff              # or a folder with tvpipe.json
```

tvpipe checks the game (errors before anything starts), opens a window
with the launcher, starts the phones' link in the background, and waits.

## The launcher: picking TVs

The window lists the TVs as they answer on the network: Apple TVs and
AirPlay TVs, Chromecasts and Google TVs. Pick one or more, then
**Start**; or **Start without a TV**, or **A browser tab on this
computer** to try the game (the TV picture plays in a tab). The game's
host page follows and casts its TV page to the chosen TVs.

- **AirPlay PIN.** An AirPlay TV that shows a code needs pairing once:
  click **Pair**, type the digits the TV shows. tvpipe keeps the pairing
  (`tvpipe pair NAME` does the same in a terminal; `tvpipe unpair`
  forgets it).
- **A TV that is not listed**: it must be on the same network (not a
  guest Wi-Fi), awake, with AirPlay or casting allowed; see `tvpipe
  discover` and the manual's Receivers page.
- **Several TVs** show the same picture; they are not frame-locked.
- A cast that fails (the TV went to sleep) shows in the host page; the
  game decides whether to offer "try again".

## Phones

The TV shows a QR code with the **join link**. Phones open it in their
browser; nothing is installed.

| | Phones reach the game through | |
|---|---|---|
| default | a public `https://` link: OpenTunnel, Cloudflare if that fails | works on any network, mobile data too; tvpipe keeps OpenTunnel's identity so it starts in a fraction of a second |
| `--tunnel=cloudflare` / `--tunnel=opentunnel` | that provider only | |
| `--lan` (`--lan=IP`) | also `http://IP:7420` on the Wi-Fi | no internet needed; the phones must be on the same Wi-Fi; plain HTTP, the join key crosses the Wi-Fi in clear |
| `--no-tunnel` | nothing public: this computer only (with `--lan`: the Wi-Fi) | |

The link carries the room code and a key in its fragment; only people
who see the QR code can join. A phone that locks or loses the network
reconnects by itself and gets everything it missed. Whether latecomers
can join a running game is the game's choice.

## The window and the browser

| `--window` | |
|---|---|
| `app` (default) | an app window of tvpipe's own Chromium-based browser (Chrome, Edge, Chromium, Brave) with a temporary profile; closing it ends the game |
| `open` | the system's browser; the game ends 30 s after its last page closed |
| `none` | prints the window's link (programs, tests) |

`--browser PATH` (or `$TVPIPE_BROWSER`) picks the browser for the window
and for the TV page; with only Firefox, the window opens in the system's
browser. `--port PORT` fixes the window's port on 127.0.0.1.

## Ending

Any of these ends everything: closing the window, the game's own end
button (`tvpipe.quit()`), Ctrl-C, or `q` and Enter in the terminal. The
host page gets a second to say goodbye, phones show that the room
closed, the TVs leave mirroring (TEARDOWN/STOP), the browsers quit and
their profiles are removed, the tunnel closes. Exit code 0 (130 after
Ctrl-C).

## Troubleshooting

| Symptom | Cause, fix |
|---|---|
| `tvpipe.json: …` at start | the manifest is wrong; `tvpipe bundle --check PATH` lists everything at once |
| `no tvpipe.json at the top of the archive` | the zip has more than one top-level entry (a stray file or folder; `__MACOSX/` and hidden names do not count) or the game is a folder too deep: pack it with `tvpipe bundle DIR` |
| `pages.host: dist/… is not in the bundle (is the game built?)` | `root` points at a build output that is missing: run the game's build |
| `the game needs tvpipe X or newer` | update tvpipe; `made for tvpipe X`: the game needs updating |
| a blank or broken page | open it with `--window open` and look at the browser's console; `tvpipe bundle --check --smoke PATH` names missing files, exceptions and failed requests |
| the launcher finds no TVs | same network, TV awake, AirPlay or casting allowed; a firewall that blocks mDNS (UDP 5353); `tvpipe discover` |
| AirPlay asks for a PIN every time | the pairing was refused or forgotten: pair again from the launcher, or `tvpipe pair NAME` |
| phones cannot open the link | the tunnel failed (the host page says "on this computer only"): try `--tunnel=cloudflare`, or `--lan` with phones on the same Wi-Fi; on Windows allow tvpipe through the firewall for `--lan` |
| phones lost the game | the host page was reloaded or closed: a reload starts a new party, so phones scan the new QR code |
| "the room closed" on phones | the game ended (the window was closed, Ctrl-C) |
| the TV shows the picture late | a cast is a video stream (about a second behind); keep timing-critical feedback on the phones |
| more detail | `tvpipe -v playshare …` prints connections and casts; `--debug` the protocols |
