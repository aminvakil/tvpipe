# W3 Net + discovery

Status: done; HTTP/RTSP framing waits for merge from `w3-net`. LG/Samsung/Cast captures still
open (no such responder was visible on 2026-11-17). Owns `src/net/**`, `src/airplay/discovery.rs`
(TXT parsing only), `tests/fixtures/net/**`.

- [x] `net/dns.rs` (or inside `mdns.rs`): DNS message build/parse: header,
      questions, PTR/SRV/TXT/A/AAAA/NSEC records (others kept as raw),
      name compression on read (loop-safe pointer following), optional on
      write; QU bit and cache-flush bit handling.
- [x] `net/mdns.rs`: legacy-unicast browse (RFC 6762 §6.7): send PTR
      queries for `_airplay._tcp.local`, `_raop._tcp.local`,
      `_googlecast._tcp.local` from an ephemeral UDP port to
      224.0.0.251:5353, re-query with backoff 1,2,4… s until timeout,
      follow up with SRV/TXT/A queries for incomplete instances, collect
      answers + additionals. Blocking sockets + read timeouts, no async.
- [x] `net/iface.rs`: default local IPv4 via UDP connect trick; local
      address to reach a given peer.
- [x] Device model: merge AirPlay and Cast records of the same device
      (by address and/or name), TXT parsing for AirPlay (`features` as two
      32-bit hex halves `0xLO,0xHI` → u64, `pk`, `model`, `srcvers`,
      `deviceid`, `flags`, `pw`) and Cast (`fn`, `md`, `id`, `ca`).
      Name matching: case-insensitive, unique prefix.
- [x] `net/http.rs` + `net/rtsp.rs`: request writer and response reader
      (status line, headers, Content-Length bodies, chunked for HTTP) over
      any `Read + Write`, so it works on TCP and on W5's encrypted stream.
      Size limits on headers/bodies.
- [x] `streamlib discover [--json] [--timeout] [--interface]`: put the
      command body in `net::discover::run(timeout, json, interfaces)`
      (or similar) in your module; the lead wires it into `session.rs`.

Done when: `streamlib discover` lists real devices on this network (run
it!), plus parsing fixtures. Capture real responses for fixtures (scrub
nothing that's needed but replace serials/MACs with fakes).

Sources allowed: RFC 1035, RFC 6762, RFC 6763, RFC 9110/9112, RFC 2326,
pyatv (MIT) for TXT/feature bit meaning. Never doubletake/UxPlay/etc.

## Notes for integration

- `discover` is wired in `session.rs` (main). For `--to`:
  `net::discover::resolve(query, timeout, &ifaces)` (`Ok(None)` when the
  query is an IP address), or `net::discover::discover` + `net::device::select`.
- W5: `net::rtsp::RtspClient` over `TcpStream`; after pair-verify take the
  socket with `client.into_conn().into_parts()` (leftover must be empty),
  wrap it, and continue with `RtspClient::replace_conn(Conn::new(wrapped))`.
  Real `/info` responses: `tests/fixtures/net/*.rtsp`.
- `--interface ADDR` binds the query socket to ADDR. Without an
  `IP_MULTICAST_IF` socket option (not in std) the OS picks the outgoing
  interface: Linux uses the bound address's interface, other systems may
  use the default multicast route. The default interface always works.

