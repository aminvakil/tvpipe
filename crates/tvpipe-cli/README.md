# tvpipe-cli

The `tvpipe` command: low-latency mirroring of video and audio to AirPlay
and Google Cast receivers (Apple TV, smart TVs, Chromecast / Google TV)
and browser tabs, from files, URLs, capture devices, web pages, the screen
or a game's frames on stdin.

```sh
cargo install tvpipe-cli      # installs `tvpipe`
tvpipe discover
tvpipe play movie.mp4 --to "Living Room"
tvpipe game play ./my-game    # a party game written as a web app
```

Release binaries and installers: <https://mitsuhiko.github.io/tvpipe/>.
The manual is there too. The library the command is built on is the
[`tvpipe`](https://crates.io/crates/tvpipe) crate.

## License

The source is Apache-2.0 (`LICENSE`). The command includes FairPlay
(`tvpipe-fairplay`, LGPL-3.0-or-later), so the built binary is
distributed under the terms of LGPL-3.0-or-later (`LICENSES/`), together
with the third-party notices there.
