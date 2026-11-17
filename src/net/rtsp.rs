//! RTSP client framing (RFC 2326 §6–§7, §12.17 CSeq, §12.37 Session) on
//! top of [`http::Conn`](super::http::Conn).
//!
//! AirPlay speaks RTSP/1.0 on its control connection, including
//! HTTP-style requests such as `GET /info` and `POST /pair-setup`. The
//! client numbers requests with `CSeq`, adds default headers (e.g.
//! `User-Agent`, `DACP-ID`, `Active-Remote`), remembers the `Session` id
//! from `SETUP`, and checks that each response echoes its `CSeq`.

use std::io::{Read, Write};

use super::http::{Conn, Headers, Request, Response, Version};
use crate::error::{Error, Result};

pub struct RtspClient<S> {
    conn: Conn<S>,
    cseq: u32,
    /// Added to every request unless it sets the same header itself.
    pub default_headers: Headers,
    /// The `Session` id, once a response carried one.
    pub session: Option<String>,
}

impl<S: Read + Write> RtspClient<S> {
    pub fn new(stream: S) -> RtspClient<S> {
        RtspClient::from_conn(Conn::new(stream))
    }

    pub fn from_conn(conn: Conn<S>) -> RtspClient<S> {
        RtspClient {
            conn,
            cseq: 0,
            default_headers: Headers::new(),
            session: None,
        }
    }

    pub fn conn(&mut self) -> &mut Conn<S> {
        &mut self.conn
    }

    pub fn into_conn(self) -> Conn<S> {
        self.conn
    }

    /// Replaces the underlying connection (e.g. the same socket wrapped in
    /// the encrypted channel after pair-verify), keeping CSeq and session.
    pub fn replace_conn<T: Read + Write>(self, conn: Conn<T>) -> RtspClient<T> {
        RtspClient {
            conn,
            cseq: self.cseq,
            default_headers: self.default_headers,
            session: self.session,
        }
    }

    /// The CSeq the next request will use.
    pub fn next_cseq(&self) -> u32 {
        self.cseq + 1
    }

    /// Builds a request with this client's CSeq, defaults and session.
    pub fn prepare(&mut self, method: &str, uri: &str) -> Request {
        self.cseq = self.cseq.wrapping_add(1);
        let mut req = Request::new(method, uri, Version::Rtsp10);
        req.headers.add("CSeq", self.cseq.to_string());
        for (k, v) in self.default_headers.iter() {
            req.headers.add(k, v);
        }
        if let Some(s) = &self.session {
            req.headers.add("Session", s.clone());
        }
        req
    }

    /// Sends a request built with [`prepare`](Self::prepare) and reads
    /// its response. Fails if the response's CSeq does not match; a
    /// missing CSeq is tolerated (some receivers omit it on HTTP-style
    /// endpoints). Non-2xx statuses are returned, not turned into errors.
    pub fn send(&mut self, req: &Request) -> Result<Response> {
        let cseq = req.headers.get("cseq").map(str::to_owned);
        trace!("> {} {} (CSeq {:?})", req.method, req.target, cseq);
        let resp = self.conn.request(req)?;
        trace!("< {} {}", resp.status, resp.reason);
        match (resp.headers.get("cseq").map(str::trim), cseq.as_deref()) {
            (Some(got), Some(want)) if got != want => {
                return Err(Error::protocol(format!(
                    "rtsp: response CSeq {got} does not match request CSeq {want}"
                )))
            }
            (None, Some(_)) => debug!("rtsp: response without CSeq"),
            _ => {}
        }
        if let Some(s) = resp.headers.get("session") {
            // `Session: id;timeout=60` (RFC 2326 §12.37).
            let id = s.split(';').next().unwrap_or("").trim();
            if !id.is_empty() {
                self.session = Some(id.to_owned());
            }
        }
        Ok(resp)
    }

    /// `prepare` + optional body + `send`.
    pub fn request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Response> {
        let mut req = self.prepare(method, uri);
        for (k, v) in headers {
            req.headers.set(*k, *v);
        }
        if let Some((ct, body)) = body {
            req = req.body(ct, body);
        }
        self.send(&req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::http::tests::Duplex;
    use crate::util::vectors;

    #[test]
    fn numbers_requests_and_tracks_session() {
        let replies = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: 1A2B;timeout=60\r\n\r\n\
                        RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 2\r\n\r\nok";
        let mut c = RtspClient::new(Duplex::new(replies));
        c.default_headers.add("User-Agent", "AirPlay/550.10");
        let r = c
            .request(
                "SETUP",
                "rtsp://192.168.1.34/1",
                &[],
                Some(("application/x-apple-binary-plist", vec![0xaa])),
            )
            .unwrap();
        assert!(r.is_success());
        assert_eq!(c.session.as_deref(), Some("1A2B"));
        let r = c
            .request(
                "RECORD",
                "rtsp://192.168.1.34/1",
                &[("User-Agent", "override")],
                None,
            )
            .unwrap();
        assert_eq!(r.body, b"ok");
        let sent = String::from_utf8_lossy(&c.conn().get_ref().output).into_owned();
        assert_eq!(
            sent,
            "SETUP rtsp://192.168.1.34/1 RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: AirPlay/550.10\r\n\
             Content-Type: application/x-apple-binary-plist\r\nContent-Length: 1\r\n\r\n\u{fffd}\
             RECORD rtsp://192.168.1.34/1 RTSP/1.0\r\nCSeq: 2\r\nUser-Agent: override\r\nSession: 1A2B\r\n\r\n"
        );
        assert_eq!(c.next_cseq(), 3);
    }

    #[test]
    fn cseq_mismatch_is_an_error() {
        let mut c = RtspClient::new(Duplex::new(b"RTSP/1.0 200 OK\r\nCSeq: 9\r\n\r\n"));
        let err = c.request("OPTIONS", "*", &[], None).unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Protocol);
        // Missing CSeq is tolerated.
        let mut c = RtspClient::new(Duplex::new(b"RTSP/1.0 403 Forbidden\r\n\r\n"));
        let r = c.request("GET", "/info", &[], None).unwrap();
        assert_eq!(r.status, 403);
        assert_eq!(
            r.error_for_status("info").unwrap_err().kind(),
            crate::error::ErrorKind::Auth
        );
    }

    #[test]
    fn replace_conn_keeps_state() {
        let mut c = RtspClient::new(Duplex::new(
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: s\r\n\r\n",
        ));
        c.request("SETUP", "rtsp://x/1", &[], None).unwrap();
        assert!(c.conn().buffered().is_empty());
        let mut c2 = c.replace_conn(Conn::new(Duplex::new(
            b"RTSP/1.0 200 OK\r\nCSeq: 2\r\n\r\n",
        )));
        c2.request("TEARDOWN", "rtsp://x/1", &[], None).unwrap();
        assert!(String::from_utf8_lossy(&c2.conn().get_ref().output)
            .contains("CSeq: 2\r\nSession: s\r\n"));
    }

    /// Real responses: an Apple TV 4K (tvOS 26.6.1) answering `GET /info`
    /// then `OPTIONS *` on one connection, and a Sonos One's `/info`.
    #[test]
    fn captured_receiver_responses() {
        let mut wire = vectors::fixture("net/appletv-info.rtsp");
        wire.extend(vectors::fixture("net/appletv-options-403.rtsp"));
        let mut c = RtspClient::new(Duplex::new(&wire));
        c.default_headers.add("User-Agent", "AirPlay/550.10");
        let info = c.request("GET", "/info", &[], None).unwrap();
        assert_eq!(info.status, 200);
        assert_eq!(info.headers.get("server"), Some("AirTunes/960.13.1"));
        assert_eq!(
            info.headers.get("content-type"),
            Some("application/x-apple-binary-plist")
        );
        assert_eq!(
            info.headers.get("x-apple-requestreceivedtimestamp"),
            Some("1197084103")
        );
        assert_eq!(info.body.len(), 1397);
        assert!(info.body.starts_with(b"bplist00"));
        let options = c.request("OPTIONS", "*", &[], None).unwrap();
        assert_eq!(
            (options.status, options.reason.as_str()),
            (403, "Forbidden")
        );
        assert!(options.body.is_empty());
        assert!(c.conn().buffered().is_empty());

        let mut c = RtspClient::new(Duplex::new(&vectors::fixture("net/sonos-info.rtsp")));
        let info = c.request("GET", "/info", &[], None).unwrap();
        assert_eq!(info.headers.get("server"), Some("AirTunes/366.0"));
        assert_eq!(info.body.len(), 674);
    }

    /// Live check against a real receiver:
    /// `STREAMLIB_LIVE_AIRPLAY=192.168.1.34:7000 cargo test live_ -- --ignored`
    #[test]
    #[ignore]
    fn live_info_over_tcp() {
        let Some(addr) = std::env::var_os("STREAMLIB_LIVE_AIRPLAY") else {
            return;
        };
        let addr = addr.to_string_lossy().into_owned();
        let tcp = std::net::TcpStream::connect(&addr).unwrap();
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut c = RtspClient::new(tcp);
        let info = c.request("GET", "/info", &[], None).unwrap();
        assert!(info.is_success(), "{info:?}");
        assert!(info.body.starts_with(b"bplist00"));
        let again = c.request("GET", "/info", &[], None).unwrap();
        assert_eq!(again.body.len(), info.body.len());
    }
}
