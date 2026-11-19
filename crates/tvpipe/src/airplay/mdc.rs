//! The MediaDataControl channel (spec §8.10): a TCP connection to the
//! receiver's audio `streamConnectionTypeMediaDataControl` port, framed
//! like the control channel with keys from a 64-bit seed sent in the
//! audio SETUP. The receiver sends audio statistics (`raup`, `rrmt`); we
//! answer `sync` messages and log the reports (a cheap way to see whether
//! audio arrives in time). The dynamic-latency algorithm is out of scope:
//! the session keeps its initial lead.
//!
//! Message: 32-byte header (u32be total size, 4-byte type `sync`/`asyn`/
//! `rply`, 8 opaque bytes, 4-byte command, u64be token, u32be status)
//! plus a bplist payload.
//!
//! Sources: docs/airplay-mirroring.md §4.6, §5.1, §8.10; vectors
//! tests/vectors/airplay-mirror/channels.txt ("MediaDataControl"),
//! appletv-session.txt (raup/rrmt bodies).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::airplay::secure::{Keys, SecureStream};
use crate::codec::bplist::Plist;
use crate::error::{Error, Result};

pub const HEADER_LEN: usize = 32;
/// Largest message we accept (the reports are a few hundred bytes).
const MAX_MESSAGE: usize = 1 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long `close` waits for the reader thread after the shutdown.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

/// A parsed message header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub size: usize,
    pub kind: [u8; 4],
    pub opaque: [u8; 8],
    pub command: [u8; 4],
    pub token: u64,
    pub status: u32,
}

impl Header {
    pub fn parse(b: &[u8]) -> Result<Header> {
        if b.len() < HEADER_LEN {
            return Err(Error::malformed("mediadatacontrol: short header"));
        }
        let arr4 = |i: usize| [b[i], b[i + 1], b[i + 2], b[i + 3]];
        let size = u32::from_be_bytes(arr4(0)) as usize;
        if !(HEADER_LEN..=MAX_MESSAGE).contains(&size) {
            return Err(Error::malformed(format!(
                "mediadatacontrol: message size {size}"
            )));
        }
        let mut opaque = [0u8; 8];
        opaque.copy_from_slice(&b[8..16]);
        let mut token = [0u8; 8];
        token.copy_from_slice(&b[20..28]);
        Ok(Header {
            size,
            kind: arr4(4),
            opaque,
            command: arr4(16),
            token: u64::from_be_bytes(token),
            status: u32::from_be_bytes(arr4(28)),
        })
    }

    /// The reply to a `sync` message.
    pub fn reply(&self, status: u32) -> [u8; HEADER_LEN] {
        let mut r = [0u8; HEADER_LEN];
        r[0..4].copy_from_slice(&(HEADER_LEN as u32).to_be_bytes());
        r[4..8].copy_from_slice(b"rply");
        r[8..16].copy_from_slice(&self.opaque);
        r[16..20].copy_from_slice(&self.command);
        r[20..28].copy_from_slice(&self.token.to_be_bytes());
        r[28..32].copy_from_slice(&status.to_be_bytes());
        r
    }
}

/// A one-line summary of a report for the log.
pub fn summarize(command: &[u8; 4], payload: &[u8]) -> String {
    let Ok(p) = Plist::from_bytes(payload) else {
        return format!(
            "{} ({} bytes, not a bplist)",
            String::from_utf8_lossy(command),
            payload.len()
        );
    };
    let keys: &[&str] = match command {
        b"rrmt" => &[
            "lostPackets",
            "latePackets",
            "unrecoveredPackets",
            "totalGlitches",
            "latencyMs",
            "retransmitSends",
            "senderIODiscontinuityCount",
            "receiverIODiscontinuityCount",
        ],
        b"raup" => &["SessionDurationSec", "RenderDeadlineMinMs"],
        _ => &[],
    };
    let fields: Vec<String> = keys
        .iter()
        .filter_map(|k| {
            let v = p.get(k)?;
            Some(match v {
                Plist::Integer(i) => format!("{k}={i}"),
                Plist::Real(r) => format!("{k}={r}"),
                other => format!("{k}={other:?}"),
            })
        })
        .collect();
    format!("{} {}", String::from_utf8_lossy(command), fields.join(" "))
}

/// Serves messages until EOF; replies to `sync` with status 0 (or 1 for a
/// payload that is not a bplist).
pub fn serve<S: Read + Write>(stream: &mut S) -> Result<()> {
    let mut header = [0u8; HEADER_LEN];
    loop {
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(Error::io("mediadatacontrol: read", e)),
        }
        let h = Header::parse(&header)?;
        let mut payload = vec![0u8; h.size - HEADER_LEN];
        stream
            .read_exact(&mut payload)
            .map_err(|e| Error::io("mediadatacontrol: read", e))?;
        let valid = payload.is_empty() || Plist::from_bytes(&payload).is_ok();
        debug!(
            "airplay: receiver report ({}): {}",
            String::from_utf8_lossy(&h.kind),
            summarize(&h.command, &payload)
        );
        if &h.kind == b"sync" {
            stream
                .write_all(&h.reply(if valid { 0 } else { 1 }))
                .and_then(|()| stream.flush())
                .map_err(|e| Error::io("mediadatacontrol: write", e))?;
        }
    }
}

/// The running channel.
pub struct MediaDataControl {
    sock: TcpStream,
    thread: Option<JoinHandle<()>>,
}

impl MediaDataControl {
    pub fn connect(addr: SocketAddr, keys: &Keys) -> Result<MediaDataControl> {
        let sock = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .map_err(|e| Error::io(format!("airplay: connect MediaDataControl {addr}"), e))?;
        let stream = sock
            .try_clone()
            .map_err(|e| Error::io("airplay: clone MediaDataControl socket", e))?;
        let keys = keys.clone();
        let thread = std::thread::Builder::new()
            .name("airplay-mdc".into())
            .spawn(move || {
                let mut s = SecureStream::new(stream, &keys);
                if let Err(e) = serve(&mut s) {
                    debug!("airplay: MediaDataControl ended: {e}");
                }
            })
            .map_err(|e| Error::io("airplay: spawn MediaDataControl", e))?;
        debug!("airplay: MediaDataControl connected to {addr}");
        Ok(MediaDataControl {
            sock,
            thread: Some(thread),
        })
    }

    pub fn close(&mut self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
        // A receiver answers our FIN by closing; one that hangs keeps the
        // reader blocked on Windows (see `util::thread`), so do not wait
        // for it past CLOSE_WAIT.
        if let Some(t) = self.thread.take() {
            crate::util::thread::join_until(t, std::time::Instant::now() + CLOSE_WAIT);
        }
    }
}

impl Drop for MediaDataControl {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::http::tests::Duplex;
    use crate::util::vectors;

    #[test]
    fn reply_vector() {
        let b = vectors::load("airplay-mirror/channels.txt")
            .into_iter()
            .find(|b| b.name().starts_with("MediaDataControl"))
            .unwrap();
        let h = Header::parse(&b.hex("request_header")).unwrap();
        assert_eq!(h.size, 37);
        assert_eq!(&h.kind, b"sync");
        assert_eq!(&h.command, b"raup");
        assert_eq!(h.reply(0).to_vec(), b.hex("reply"));
        assert!(Header::parse(&[0; 31]).is_err());
        let mut bad = b.hex("request_header");
        bad[3] = 4;
        assert!(Header::parse(&bad).is_err());
    }

    #[test]
    fn serves_reports() {
        let blocks = vectors::load("airplay-mirror/appletv-session.txt");
        let mut input = Vec::new();
        for b in blocks
            .iter()
            .filter(|b| b.name().starts_with("Apple TV MediaDataControl"))
        {
            input.extend(b.hex("header"));
            input.extend(b.hex("payload_bplist"));
            let h = Header::parse(&b.hex("header")).unwrap();
            assert_eq!(h.size, HEADER_LEN + b.hex("payload_bplist").len());
            let s = summarize(&h.command, &b.hex("payload_bplist"));
            if &h.command == b"rrmt" {
                assert!(
                    s.contains("lostPackets=0") && s.contains("latePackets=58"),
                    "{s}"
                );
            }
        }
        // A sync request gets a reply.
        let mut sync = vec![0, 0, 0, 32];
        sync.extend(b"sync");
        sync.extend([7; 8]);
        sync.extend(b"raup");
        sync.extend(9u64.to_be_bytes());
        sync.extend([0; 4]);
        input.extend(&sync);
        let mut d = Duplex::new(&input);
        serve(&mut d).unwrap();
        assert_eq!(d.output.len(), 32);
        assert_eq!(&d.output[4..8], b"rply");
        assert_eq!(&d.output[8..16], &[7; 8]);
    }
}
