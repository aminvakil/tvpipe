---
title: A web page on the TV
summary: Dashboards and slides, without a browser window.
group: Use
spec:
  Command: `share URL`
  Needs: Chrome, Edge, Chromium, Brave or Firefox
---

Give `share` a URL and tvpipe opens it in a hidden browser of its own and puts it on the TV, with sound.

## Showing a page

:::aside Size
The page gets the shape of the TV, laid out at up to 1920×1080. `--size` sets another layout. `--resolution` (`auto`, `720p`, `1080p`, `4k`) sets how sharp it is on the TV: the layout stays, tvpipe renders it at a higher scale. `auto` goes up to 1080p as far as the TV allows; 4K only when you ask for it.
:::

```sh
# a dashboard, reloaded every 30 minutes
$ tvpipe share https://status.example.com --to TV --reload 30

# a local file
$ tvpipe share ./slides.html --to TV
```

Starting takes about a second. If the page reloads or navigates, the TV shows a short gap of about half a second. If the page or the browser crashes, tvpipe starts it again.

```sh
# a 1280×720 dashboard, sharp on a 4K TV
$ tvpipe share https://status.example.com --to TV --size 1280x720 --resolution 4k
```

`--visible`, `--profile`, `--reload`, `--browser` and `--resolution` only work with a URL. 4K needs a TV that takes it (an Apple TV is asked for its full decoder size; a Chromecast that allows only 1080p gets 1080p) and a computer with a hardware video encoder. Firefox renders at the page's own size.

## Pages that need a login

:::aside Profiles
tvpipe never uses your own browser profile. Without `--profile` every run gets a new one, deleted afterwards, or by the next run if tvpipe was killed.
:::

Log in once in a visible window and keep the profile. Later runs use it without a window.

```sh
$ tvpipe share https://grafana.example.com --to TV --visible --profile ~/tvpipe-grafana
$ tvpipe share https://grafana.example.com --to TV --profile ~/tvpipe-grafana
```

With `--visible`, closing the window stops tvpipe.

## Which browser

:::aside Order
1. `--browser PATH`
2. `TVPIPE_BROWSER`
3. Chrome, Edge, Chromium, Brave
4. Firefox
:::

Any Chromium based browser works, and Firefox. tvpipe uses the first one it finds, so Firefox only when no Chromium based browser is installed. `--browser firefox` picks Firefox.

```sh
$ tvpipe share https://status.example.com --to TV --browser firefox
```

Firefox shows the page with its sound too. Its own speakers stay quiet while tvpipe shares. Pages that set their content security policy in a `<meta>` tag cannot share in Firefox. On Linux, a Firefox installed as a Snap cannot read tvpipe's profile in `/tmp`: use Firefox from Mozilla or your distribution's package.

## Limitations

Some sites refuse browsers that are controlled by a program. tvpipe does not hide that, so those sites do not work, and tvpipe reports their answer (401, 403 or 429). Video calls are an example. For those, open the call in your normal browser and present a [browser tab](browser.md) in it.

On macOS, the system treats tvpipe's hidden browser as a running copy of that browser, Chrome for example. While it runs and your own Chrome does not, opening Chrome from the Dock or Spotlight, or a link from another app, goes to tvpipe's browser, and nothing seems to happen. Start your browser before tvpipe and keep it open, or stop tvpipe first. Another way is to give tvpipe a browser you do not use yourself with `--browser`. The same goes for [`tvpipe screen`](screen.md) and the window of [`tvpipe game play`](party.md), where `--window=open` uses your usual browser instead.
