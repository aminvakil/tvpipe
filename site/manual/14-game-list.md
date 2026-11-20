---
title: Games to play
summary: Party games to play by name, downloaded when you play them.
group: Use
spec:
  Command: `tvpipe game play NAME`
  The list: `games.json` on this site
---

Party games are not part of tvpipe. Each one comes out on its own, and `tvpipe game play NAME` downloads the newest release that fits your tvpipe, asks you once whether to run it, and keeps it for next time. [Party games](party.md) covers playing.

## The games

:::games
:::

## Playing one

```sh
$ tvpipe game play drawbluff
```

:::aside Offline
`--offline` plays a game downloaded before, without the network.
:::

The first time, tvpipe shows where the game comes from, its size and its checksum, and asks before it runs it: a game runs in tvpipe's window and can cast to your TVs. tvpipe asks again when a new release comes out. `--yes` skips the question.

The list comes from [games.json](https://tvpipe.party/games.json) on this site, which tvpipe reads when you play a game by name. `tvpipe game list` shows the games you have downloaded.

## Adding a game

A game on this list is a game anyone can play by name. Publish it as a release (see [Share it](making-games.md#s7)), then open a pull request that adds it to `site/static/games.json` in the [repository](https://github.com/mitsuhiko/tvpipe). Everyone can also play a game from its release without the list: `tvpipe game play gh:OWNER/REPO`.
