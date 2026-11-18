//! The mirroring event channel (spec §9): the receiver sends RTSP
//! `POST /command` requests with a bplist `{type, value}` body over a TCP
//! connection we open to its `eventPort`; we answer every one with
//! `200 OK`. HAP profile: encrypted like the control channel, with the
//! Events keys (`airplay::keys::event_keys`).
//!
//! Sources: docs/spec/airplay-mirroring.md §4.2, §5.1, §9; vectors
//! tests/vectors/airplay-mirror/channels.txt ("event channel"),
//! appletv-session.txt ("event channel command").

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::airplay::secure::{Keys, SecureStream};
use crate::airplay::setup::bits64;
use crate::codec::bplist::Plist;
use crate::error::{Error, Result};
use crate::net::http::{Conn, Limits, Response, Version};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// One receiver command.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A fresh `/info` dict (with `displays`).
    UpdateInfo(Plist),
    /// A new PTP timeline (`ClockID`), if the dict carried one.
    UpdateTimingPeerInfo(Option<u64>),
    ForceKeyFrame,
    Other(String),
}

/// Parses a `POST /command` body.
pub fn parse_command(body: &[u8]) -> Result<Event> {
    let p = Plist::from_bytes(body)?;
    let kind = p
        .get("type")
        .and_then(Plist::as_str)
        .ok_or_else(|| Error::malformed("airplay event: command without a type"))?;
    let value = p.get("value");
    Ok(match kind {
        "updateInfo" => match value {
            Some(v) if v.as_dict().is_some() => Event::UpdateInfo(v.clone()),
            _ => Event::Other("updateInfo".into()),
        },
        "updateTimingPeerInfo" => {
            Event::UpdateTimingPeerInfo(bits64(value.and_then(|v| v.get("ClockID"))))
        }
        "forceKeyFrame" => Event::ForceKeyFrame,
        other => Event::Other(other.to_owned()),
    })
}

fn limits() -> Limits {
    Limits {
        max_head: 16 * 1024,
        max_body: 1 << 20,
        ..Limits::default()
    }
}

/// Serves requests on `conn` until EOF or a malformed message, answering
/// each with `200 OK` and handing its command to `on_event`.
pub fn serve<S: Read + Write>(conn: &mut Conn<S>, mut on_event: impl FnMut(Event)) -> Result<()> {
    loop {
        let req = match conn.read_request() {
            Ok(r) => r,
            Err(e) if conn.buffered().is_empty() => {
                debug!("airplay event channel ended: {e}");
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let mut resp = Response::new(Version::Rtsp10, 200, "OK");
        if let Some(cseq) = req.headers.get("cseq") {
            resp.headers.add("CSeq", cseq.trim());
        }
        conn.send_response(&resp)?;
        if req.body.is_empty() {
            debug!("airplay event: {} {} (no body)", req.method, req.target);
            continue;
        }
        match parse_command(&req.body) {
            Ok(ev) => on_event(ev),
            Err(e) => {
                return Err(Error::malformed(format!(
                    "airplay event: bad command body: {e}"
                )))
            }
        }
    }
}

/// The running event channel.
pub struct EventChannel {
    sock: TcpStream,
    thread: Option<JoinHandle<()>>,
}

impl EventChannel {
    /// Connects to `addr` and serves it on a thread. `keys` = HAP event
    /// keys (`None`: plaintext, legacy profile).
    pub fn connect(
        addr: SocketAddr,
        keys: Option<&Keys>,
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Result<EventChannel> {
        let sock = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .map_err(|e| Error::io(format!("airplay: connect event channel {addr}"), e))?;
        let _ = sock.set_nodelay(true);
        let stream = sock
            .try_clone()
            .map_err(|e| Error::io("airplay: clone event socket", e))?;
        let keys = keys.cloned().map(Arc::new);
        let thread = std::thread::Builder::new()
            .name("airplay-events".into())
            .spawn(move || {
                let r = match keys {
                    Some(k) => serve(
                        &mut Conn::with_limits(SecureStream::new(stream, &k), limits()),
                        on_event,
                    ),
                    None => serve(&mut Conn::with_limits(stream, limits()), on_event),
                };
                if let Err(e) = r {
                    warn!("airplay event channel: {e}");
                }
            })
            .map_err(|e| Error::io("airplay: spawn event thread", e))?;
        debug!("airplay: event channel connected to {addr}");
        Ok(EventChannel {
            sock,
            thread: Some(thread),
        })
    }

    /// Closes the connection and joins the thread.
    pub fn close(&mut self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for EventChannel {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::http::tests::Duplex;
    use crate::util::vectors;

    fn command(body: &[u8], cseq: u32) -> Vec<u8> {
        let mut v = format!(
            "POST /command RTSP/1.0\r\nCSeq: {cseq}\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn answers_and_parses_commands() {
        let update = vectors::load("airplay-mirror/appletv-session.txt")
            .into_iter()
            .find(|b| b.name().starts_with("Apple TV event channel command"))
            .unwrap()
            .hex("body_bplist");
        let timing = Plist::dict()
            .with("type", "updateTimingPeerInfo")
            .with(
                "value",
                Plist::dict().with("ClockID", -6144091866766770166i64),
            )
            .to_bytes();
        let key = Plist::dict().with("type", "forceKeyFrame").to_bytes();
        let input = [command(&update, 1), command(&timing, 2), command(&key, 3)].concat();
        let mut conn = Conn::new(Duplex::new(&input));
        let mut events = Vec::new();
        serve(&mut conn, |e| events.push(e)).unwrap();
        assert_eq!(events.len(), 3);
        match &events[0] {
            Event::UpdateInfo(info) => assert!(info.get("displays").is_some()),
            e => panic!("{e:?}"),
        }
        assert_eq!(
            events[1],
            Event::UpdateTimingPeerInfo(Some(0xaabb_ccff_fe00_000a))
        );
        assert_eq!(events[2], Event::ForceKeyFrame);
        let out = String::from_utf8(conn.get_ref().output.clone()).unwrap();
        assert_eq!(
            out,
            "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n\
             RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 0\r\n\r\n\
             RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: 0\r\n\r\n"
        );
    }

    #[test]
    fn malformed_body_ends_channel() {
        let mut conn = Conn::new(Duplex::new(&command(b"junk", 1)));
        assert!(serve(&mut conn, |_| {}).is_err());
        let typeless = Plist::dict().with("value", 1i64).to_bytes();
        let mut conn = Conn::new(Duplex::new(&command(&typeless, 1)));
        assert!(serve(&mut conn, |_| {}).is_err());
        // A truncated request ends the channel without a panic.
        let mut conn = Conn::new(Duplex::new(
            b"POST /x RTSP/1.0\r\nContent-Length: 99\r\n\r\nab",
        ));
        let mut n = 0;
        let _ = serve(&mut conn, |_| n += 1);
        assert_eq!(n, 0);
    }

    #[test]
    fn encrypted_frame_vector() {
        let b = vectors::load("airplay-mirror/channels.txt")
            .into_iter()
            .find(|b| b.name().starts_with("event channel"))
            .unwrap();
        let keys = Keys {
            write: b.hex("key").try_into().unwrap(),
            read: [0; 32],
        };
        let mut s = SecureStream::new(Vec::new(), &keys);
        s.write_all(&b.hex("plaintext")).unwrap();
        assert_eq!(s.into_inner(), b.hex("frame"));
        // And the keys the sender derives for this channel use the same
        // write key for that secret.
        let k = crate::airplay::keys::event_keys(&(0x40u8..0x60).collect::<Vec<u8>>()).unwrap();
        assert_eq!(k.write.to_vec(), b.hex("key"));
    }
}
