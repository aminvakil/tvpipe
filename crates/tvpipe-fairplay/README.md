# tvpipe-fairplay

FairPlay SAP for AirPlay mirroring: the part of
[tvpipe](https://github.com/mitsuhiko/tvpipe) that is not Apache-2.0.

It is a port of [doubletake](https://github.com/omarroth/doubletake)'s SAP implementation
(LGPL-3.0-or-later; see the attribution at the top of each file), with a
bytes-in, bytes-out API (`FpSession`: `m1`, `m3`, `check_m4`,
`wrap_key`). The HTTP requests and the keys derived from the wrapped key
live in `tvpipe`.

You normally do not depend on this crate directly: `tvpipe`'s optional
`fairplay` feature pulls it in.

## License

LGPL-3.0-or-later (`LICENSE-LGPL-3.0`, which supplements
`LICENSE-GPL-3.0`), except `src/aes.rs`, a copy of tvpipe's AES
(Apache-2.0, `LICENSE-APACHE-2.0`). Rust links statically: a program that
includes this crate must, besides the license texts and notices, let its
users relink it with a modified version of this crate (LGPLv3 §4(d)),
e.g. by offering its object files or source. Not legal advice.
