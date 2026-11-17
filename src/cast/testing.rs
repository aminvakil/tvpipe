//! Test helpers: a TLS Cast-channel server with a fixture certificate.
//! The fake streaming receiver built on it lives in
//! [`super::fake_receiver`].

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::channel::deframe;
use crate::codec::protobuf::CastMessage;
use crate::error::{Error, Result};
use crate::util::vectors::fixture;

pub fn server_config() -> Arc<ServerConfig> {
    let cert = CertificateDer::from(fixture("cast/test-cert.der"));
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(fixture("cast/test-key.pk8.der")));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    Arc::new(cfg)
}

/// One accepted TLS connection, speaking framed CastMessages.
pub struct ServerConn {
    tls: StreamOwned<ServerConnection, TcpStream>,
    buf: Vec<u8>,
    pending: std::collections::VecDeque<CastMessage>,
}

impl ServerConn {
    pub fn accept(sock: TcpStream) -> ServerConn {
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let conn = ServerConnection::new(server_config()).unwrap();
        ServerConn {
            tls: StreamOwned::new(conn, sock),
            buf: Vec::new(),
            pending: Default::default(),
        }
    }

    pub fn set_read_timeout(&self, d: Duration) {
        self.tls.sock.set_read_timeout(Some(d)).unwrap();
    }

    pub fn send(&mut self, msg: &CastMessage) -> Result<()> {
        self.tls
            .write_all(&msg.encode_framed())
            .and_then(|_| self.tls.flush())
            .map_err(|e| Error::io("test server write", e))
    }

    /// Next message; errors on EOF or timeout.
    pub fn recv(&mut self) -> Result<CastMessage> {
        loop {
            if let Some(m) = self.pending.pop_front() {
                return Ok(m);
            }
            let mut chunk = [0u8; 8192];
            let n = self
                .tls
                .read(&mut chunk)
                .map_err(|e| Error::io("test server read", e))?;
            if n == 0 {
                return Err(Error::closed("test server: eof"));
            }
            self.buf.extend_from_slice(&chunk[..n]);
            self.pending.extend(deframe(&mut self.buf)?);
        }
    }
}

/// A one-connection TLS server running `handler` on its own thread.
pub struct TlsTestServer {
    addr: SocketAddr,
    handle: Option<JoinHandle<()>>,
}

impl TlsTestServer {
    pub fn spawn(handler: impl FnOnce(ServerConn) + Send + 'static) -> TlsTestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            handler(ServerConn::accept(sock));
        });
        TlsTestServer {
            addr,
            handle: Some(handle),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Waits for the handler; re-raises its panic.
    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            if let Err(e) = h.join() {
                std::panic::resume_unwind(e);
            }
        }
    }
}
