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
The page gets the shape of the TV, up to 1920×1080. `--size` sets another size.
:::

```sh
# a dashboard, reloaded every 30 minutes
$ tvpipe share https://status.example.com --to TV --reload 30

# a local file
$ tvpipe share ./slides.html --to TV
```

Starting takes about a second. If the page reloads or navigates, the TV shows a short gap of about half a second. If the page or the browser crashes, tvpipe starts it again.

`--visible`, `--profile`, `--reload` and `--browser` only work with a URL.

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

Some sites refuse browsers that are controlled by a program. tvpipe does not hide that, so those sites do not work, and tvpipe reports their answer (401, 403 or 429). Video calls are an example. For those, open the call in your normal browser and present a [browser tab](08-browser.md) in it.
