//! The TLS client side the tunnels and the game fetcher share: TCP to a
//! host (every address in turn, with timeouts), a rustls client (ring)
//! trusting Mozilla's roots plus extra anchors (`$SSL_CERT_FILE`, a test
//! CA), and the handshake.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConnection, StreamOwned};

use super::{base64, roots};
use crate::error::{Error, Result};

/// A TLS connection over TCP.
pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// The ring provider.
pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The DER blocks labelled `label` in a PEM text.
pub fn pem_blocks(text: &str, label: &str) -> Result<Vec<Vec<u8>>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&begin) {
        let body = &rest[i + begin.len()..];
        let j = body
            .find(&end)
            .ok_or_else(|| Error::malformed(format!("pem: unterminated {label}")))?;
        let b64: String = body[..j].split_whitespace().collect();
        out.push(base64::decode(&b64)?);
        rest = &body[j + end.len()..];
    }
    Ok(out)
}

/// The certificates in `$SSL_CERT_FILE` (a company proxy's CA), as extra
/// trust anchors; none when it is unset or unreadable (a warning).
pub fn env_roots(what: &str) -> Vec<Vec<u8>> {
    let Some(f) = std::env::var_os("SSL_CERT_FILE").filter(|f| !f.is_empty()) else {
        return Vec::new();
    };
    match std::fs::read_to_string(&f).map_err(|e| e.to_string()) {
        Ok(text) => match pem_blocks(&text, "CERTIFICATE") {
            Ok(certs) => certs,
            Err(e) => {
                warn!("{what}: SSL_CERT_FILE: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            warn!("{what}: SSL_CERT_FILE: {e}");
            Vec::new()
        }
    }
}

/// A client config trusting Mozilla's roots and `extra` (DER), speaking
/// HTTP/1.1 (ALPN).
pub fn client_config(extra: &[Vec<u8>], what: &str) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = roots::store();
    roots.add_parsable_certificates(extra.iter().cloned().map(CertificateDer::from));
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::crypto(format!("{what}: TLS: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

/// TCP to `host:port`: each address in turn, `timeout` to connect and
/// for every read and write after.
pub fn tcp(host: &str, port: u16, timeout: Duration, what: &str) -> Result<TcpStream> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::io(format!("{what}: resolve {host}"), e))?;
    let mut last = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(tcp) => {
                let _ = tcp.set_nodelay(true);
                tcp.set_read_timeout(Some(timeout))
                    .and_then(|()| tcp.set_write_timeout(Some(timeout)))
                    .map_err(|e| Error::io(format!("{what}: socket"), e))?;
                return Ok(tcp);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => Error::io(format!("{what}: connect {host}"), e),
        None => Error::not_found(format!("{what}: no address for {host}")),
    })
}

/// The TLS handshake for `sni` on `tcp`.
pub fn handshake(
    mut tcp: TcpStream,
    config: Arc<rustls::ClientConfig>,
    sni: &str,
    what: &str,
) -> Result<TlsStream> {
    let name = ServerName::try_from(sni.to_owned())
        .map_err(|_| Error::usage(format!("{what}: bad host {sni}")))?;
    let mut conn = ClientConnection::new(config, name)
        .map_err(|e| Error::crypto(format!("{what}: TLS: {e}")))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| Error::io(format!("{what}: TLS handshake with {sni}"), e))?;
    }
    Ok(StreamOwned::new(conn, tcp))
}
