---
title: Public links
summary: Let people elsewhere watch, or share to your TV.
group: Use
spec:
  Option: `--tunnel`
  Providers: Cloudflare; OpenTunnel
---

`--tunnel` puts tvpipe's browser page on a public https link, for people who are not on your network.

## Watching from elsewhere

:::aside Time limit
`--tunnel-ttl MIN` lets new viewers join for MIN minutes only.
:::

With the player, anyone with the link watches in their browser. That is `preview`, or `play`, `screen` and `test-pattern` with `--to browser`.

```sh
$ tvpipe preview movie.mp4 --tunnel
$ tvpipe screen --to browser --tunnel=opentunnel
```

tvpipe prints the link. In a terminal it also draws the link as a QR code, so a phone can open it by scanning the screen. `--no-qr`, or `TVPIPE_NO_QR=1`, turns that off.

At most 4 people can watch over a link. Each of them gets a full copy of the stream, so your upload speed limits how many can watch.

## Sharing to your TV from elsewhere

:::aside Time limit
The link lets people in for 60 minutes. `--tunnel-ttl MIN` changes that, and `0` removes the limit.
:::

With the share page, someone elsewhere opens the link and shares a tab, a window or a screen to your TVs.

```sh
$ tvpipe share --to "Living Room" --tunnel
```

Nothing reaches the TV until you allow it. tvpipe asks in the terminal and shows their address, their browser and what they want to share. Answer `y` to allow it. `--yes` skips the question.

Programs that use `--machine` or the [Node SDK](node.md) have no terminal to ask in. They need `--yes`, in the SDK `yes: true`, or remote shares never show.

## Any web app

:::aside QR codes
`tvpipe qr TEXT` prints any link as a QR code, `--svg` as an SVG file.
:::

`tvpipe tunnel PORT` puts a server of your own on a public link, for example a web app on port 8080. It runs until you stop it.

```sh
$ tvpipe tunnel 8080
$ tvpipe tunnel 8080 --tunnel=opentunnel
```

Your server answers everything that comes in, so it decides who may do what. The [party games](party.md) library uses this for the players' phones.

## Providers

:::aside Spelling
Attach the provider with `=`. In `--tunnel opentunnel` the second word is read as the input.
:::

Both are built in. Neither needs an account or another program.

| Option | Provider |
|---|---|
| `--tunnel`, `--tunnel=cloudflare` | A Cloudflare quick tunnel on `trycloudflare.com`. Ready in about 7 seconds, with a new address every run. Cloudflare can see the page and the stream. |
| `--tunnel=opentunnel` | [OpenTunnel](https://opentunnel.xyz). The encryption ends in tvpipe, so the relay cannot read anything. The first use sets up this computer, which takes about 20 seconds once. |

## Safety

- Every link has a secret of its own and stops working when tvpipe stops.
- Your local link and its secret never leave the computer.
- A remote share only reaches your TV after you allow it.

## Limits

- Cloudflare's quick tunnels have no uptime guarantee and are not meant for heavy video.
- After a few dozen quick tunnels from one internet address in an hour, Cloudflare refuses new ones with “429 Too Many Requests” for an hour or more. Wait, or use `--tunnel=opentunnel`.
- OpenTunnel allows 30 new tunnels per hour per address, but a computer only creates one, on its first run.

The details are in [docs/tunnels.md](https://github.com/mitsuhiko/tvpipe/blob/main/docs/tunnels.md).
