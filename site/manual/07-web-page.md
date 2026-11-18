---
title: A web page on the TV
summary: Dashboards and slides, without a browser window.
group: Use
spec:
  Command: `share URL`
  Needs: Chrome, Edge, Chromium or Brave
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

If the page reloads, navigates or crashes, tvpipe keeps it on the TV.

## Pages that need a login

:::aside Profiles
tvpipe never uses your own browser profile. Without `--profile` every run starts fresh.
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
:::

Any Chromium based browser works. tvpipe uses the first one it finds.

## Limitations

Some sites refuse browsers that are controlled by a program. tvpipe does not hide that, so those sites do not work. Video calls are an example. For those, open the call in your normal browser and present a [browser tab](08-browser.md) in it.
