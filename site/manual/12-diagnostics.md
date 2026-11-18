---
title: Diagnostics
summary: What to do when something does not work.
group: Reference
spec:
  Flags: `-v`, `--debug`
---

What to try when a TV does not show up or the picture does not appear.

## More output

`-v` prints what tvpipe does. `--debug` also prints every protocol message, which is a lot.

```sh
$ tvpipe -v play --to TV movie.mp4
```

## Common problems

| Problem | What to do |
|---|---|
| No receivers found | Make sure the computer and the TV are on the same network. Try `--interface`, or give `--to` the address. |
| No sound in a browser tab | Click the page once. |
| A site does not show with `share URL` | It refuses controlled browsers. See [limitations](07-web-page.md#s4). |
