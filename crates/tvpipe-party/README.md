# tvpipe-party

Party games on [tvpipe](https://crates.io/crates/tvpipe), hosted in Rust:
phones join with a QR code on the TV, the host keeps rooms, players and
their views in sync over WebSockets. Speaks the protocol of the
`@tvpipe/party` npm package (the same browser client, `web/party-client.js`,
and the same test vectors, `tests/vectors/`).

Status: in development; see `docs/multiscreen-design.md` in the repository.
