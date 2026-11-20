# Short join links (`tvpipe.party/CODE`)

Status: being built. The service is `apps/tvpipe-party/` (a Cloudflare
Worker on the free plan, the domain `tvpipe.party`); tvpipe registers
links from `crates/tvpipe/src/playshare/shortlink.rs`; the party library
puts the short link into `JoinInfo.short`.

## Why

A tunnel's join link is long: `https://baqlo7vg7agz.opentunnel.xyz/?r=BCDF#k=…`
or `https://some-random-words.trycloudflare.com/?r=BCDF#k=…`. Nobody types
that from a TV. The QR code carries it fine, but people who cannot scan
need something short. So the TV shows **`tvpipe.party/BCDF`**: the
service redirects that to the registered join link. The QR code keeps the
direct link (scanning never depends on the service). Games do not show
the tunnel's link or the room code on their own any more.

## Decisions (the owner's)

- One small service, ours: a Worker with D1 on the free plan, which also
  serves the website (the static files of `site/_out`).
- No keep-alive traffic: a link is registered once and stays until tvpipe
  deletes it at the end of the game. A code nobody refreshed for **24
  hours** may be given to someone else (a crashed tvpipe never deletes
  its link). tvpipe refreshes its link every 6 hours, so even a game that
  runs for days keeps its code.
- Only tvpipe's own join links are accepted (no open redirect).

## The service

Base: `https://tvpipe.party` (tvpipe: `TVPIPE_PARTY_URL` overrides it,
for tests and self-hosting).

### Codes

Four letters of `BCDFGHJKLMNPQRSTVWXZ` (the room code alphabet:
no vowels, so no words; no 0/O). Lookups are case-insensitive, stored
upper case. A code is **free** when it has no row or its row's `updated`
is more than 24 hours old.

### Join links it accepts

```
https://LABEL.trycloudflare.com/?r=ROOM#k=KEY
https://ROUTE.ID.opentunnel.xyz/?r=ROOM#k=KEY
```

- `LABEL`, `ROUTE`, `ID`: `[a-z0-9-]{1,63}`, not starting or ending with
  `-`; exactly one label before trycloudflare.com, exactly two before
  opentunnel.xyz (a route per run under the machine's tunnel,
  docs/tunnels.md).
- No port, no user info, path exactly `/`, the query exactly `r=` plus
  4–8 letters `A-Z`, the fragment exactly `k=` plus 16–64 characters of
  `A-Za-z0-9_-`.
- At most 300 characters.

Anything else: `400 bad-url`.

### API (JSON; tvpipe calls it, browsers do not: no CORS)

Errors are `{"error": {"code": "…", "message": "…"}}`.

| Request | Answer |
|---|---|
| `POST /api/links` `{"url": JOIN, "code": "BCDF"?}` | `201 {"code", "short", "url", "secret"}`. The proposed `code` (the party's room code) if it is valid and free, else a random free code. `400 bad-url`, `400 bad-request` (not JSON, no url), `429 rate-limited`, `503 no-code` (no free code after 32 tries). |
| `PUT /api/links/CODE` `Authorization: Bearer SECRET`, body `{"url": JOIN}` or `{}` | `200 {"code", "short", "url"}`: `updated` is now, the url replaced when given. `404 not-found` when the code has no row or the secret does not match (it was given to someone else, or deleted): the caller registers again. `400 bad-url`. |
| `DELETE /api/links/CODE` `Authorization: Bearer SECRET` | `204`, also when there was nothing to delete; a wrong secret deletes nothing (still `204`). |

`short` is `https://tvpipe.party/CODE` (the request's origin). `secret`
is 32 random bytes, base64url; the database keeps its SHA-256 (hex).
Request bodies over 4 KiB: `413`.

### Pages

| Request | Answer |
|---|---|
| `GET /CODE` (4 letters, any case) | `302 Location: JOIN` (the fragment included; browsers keep it), `Cache-Control: no-store`, `Referrer-Policy: no-referrer`. Unknown code: `404`, the join page with "No game with the code BCDF. Check the code on the TV.". |
| `GET /join` | the join page: one field for the code, a button; a `GET` form to `/join` (no JavaScript needed). |
| `GET /join?code=bcdf` | as `GET /BCDF`. Empty or malformed code: the join page with a message. |
| anything else | the website's static files (`site/_out`), served by Cloudflare before the Worker runs; what is not a file: `404` with a short page linking `/` and `/join`. |

The join page is small, self-contained HTML (inline CSS, the site's
colours, works on any phone, `<meta name="viewport">`, the field with
`autocapitalize="characters" autocomplete="off" inputmode="text"
maxlength="8"`, autofocus).

### Limits

Per client IP (`CF-Connecting-IP`), with Cloudflare's `ratelimit`
binding (counted per location, which is enough):

- lookups (`/CODE`, `/join?code=` with a well-formed code), hits and
  misses alike: 20 a minute, then `429` with the join page saying to wait
  a minute (counting only misses would let a scanner tell live codes
  from the 302s once the misses are used up);
- `POST /api/links`: 10 a minute;
- `PUT`/`DELETE`: 60 a minute.

### Storage (D1)

```sql
CREATE TABLE links (
  code TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  secret TEXT NOT NULL,      -- SHA-256 of the secret, hex
  created INTEGER NOT NULL,  -- ms since the epoch
  updated INTEGER NOT NULL
);
```

Taking a code is one statement, so two registrations cannot both get it:

```sql
INSERT INTO links (code, url, secret, created, updated) VALUES (?1, ?2, ?3, ?4, ?4)
ON CONFLICT (code) DO UPDATE SET url = ?2, secret = ?3, created = ?4, updated = ?4
WHERE links.updated < ?5      -- now - 24 h
```

(`changes` is 1 when the code was taken.) Lookups ignore age: a stale row
still redirects until its code is given away. Rows of crashed runs stay
until their code is reused (at most 160,000 rows).

### Cost

Static files do not run the Worker. A game costs about 2 Worker requests
(`POST`, `DELETE`) plus one per 6 hours, plus one per person who types
the code: tens of thousands of games a day within the free plan's
100,000 requests. D1: a write per register, refresh and delete, a read
per lookup.

## tvpipe

- The host page asks for the short link over the local API:
  `shortlink {url, code}` → `{url: "https://tvpipe.party/BCDF", code}`,
  or an error (`unavailable`: no tunnel, `--no-short-link`, the service
  failed; `bad-url`: the url is not on the tunnel's host). The call runs
  on its own thread (network).
- Only for the tunnel's join link (the LAN and this computer get none).
- tvpipe keeps one registration per run. The same url again: the same
  answer, no request. Another url (a reloaded host page, a new tunnel):
  `PUT` with the new url (the code stays); `404` there: `POST` again.
- A thread refreshes it every 6 hours (`PUT {}`) while the run lasts; at
  the end tvpipe sends `DELETE` (3 s at most; failures are only logged).
- `--no-short-link` (`tvpipe game play`) turns it off; so does
  `TVPIPE_PARTY_URL=off`.
- Requests: HTTPS through `net::fetch` (Mozilla's roots), `User-Agent`
  tvpipe's, timeouts 5 s connect, 10 s read; plain http only to
  loopback (tests).

## The party library

- `JoinInfo.short?: string`: `https://tvpipe.party/BCDF`, when tvpipe
  registered one. `HostedParty.refreshJoin()` asks for it when the phone
  link is the tunnel's; failures leave it out.
- `joinText(join)` (exported from the client too): what a TV or host page
  shows people to type: the short link without `https://` (`tvpipe.party/BCDF`),
  or `null`: show only the QR code (a link without its `#k=` key would not
  let anyone in, so there is nothing else worth typing).
- The QR code stays on `join.url` (the direct link).
