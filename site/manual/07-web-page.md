---
title: A web page on the TV
summary: Dashboards and slides in a browser of tvpipe’s own.
group: Use
spec:
  Command: `share URL`
  Needs: Chrome, Edge, Chromium or Brave
---

Give `share` a URL and tvpipe starts a browser of its own, headless, opens the page at exactly the size of the stream and puts it on the TV, with sound.

## Showing a page

:::aside Size
The TV’s screen shape, at most 1920×1080, or `--size`.
:::

```sh
# a dashboard, reloaded every 30 minutes
$ tvpipe share https://status.example.com --to TV --reload 30

# a local file, also in a browser tab here
$ tvpipe share ./slides.html --to TV --to browser --open
```

The start takes about a second. If the page navigates or reloads, tvpipe shares it again after a gap of about half a second. If the page or the browser crashes, it is started again.

## Pages that need a login

:::aside Profiles
tvpipe never uses your own browser profile. Each run gets a temporary one that is deleted afterwards, unless you pass `--profile`.
:::

Log in once in a visible window and keep the profile; later runs reuse it headless.

```sh
$ tvpipe share https://grafana.example.com --to TV --visible --profile ~/tvpipe-grafana
$ tvpipe share https://grafana.example.com --to TV --profile ~/tvpipe-grafana
```

With `--visible`, closing the window ends the session.

## Which browser

:::aside Lookup order
1. `--browser PATH`
2. `$TVPIPE_BROWSER`
3. Chrome, Edge, Chromium, Brave
:::

Any Chromium-based browser works. tvpipe controls it over the DevTools protocol and shares the tab through the same path as [share](06-share.md).

## Limitations

:::caution
tvpipe identifies its browser as automated and does not disguise it.
:::

Sites that refuse automated browsers do not work; tvpipe reports the site’s 401, 403 or 429. Video calls are one such case: open the call in a normal browser and present a [`--to browser` tab](08-browser.md) in it instead.
