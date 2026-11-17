//! Local address selection without OS interface APIs.
//!
//! The trick: `connect` on a UDP socket sends nothing, but makes the OS pick
//! the route and the source address it would use for that peer, which
//! `local_addr` then reports. This works the same on every platform.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use crate::error::{Error, Result};

/// The local address the OS would use to reach `peer`.
pub fn local_addr_for(peer: IpAddr) -> Result<IpAddr> {
    let bind: SocketAddr = match peer {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let sock = UdpSocket::bind(bind).map_err(|e| Error::io("iface: bind UDP socket", e))?;
    // The port is irrelevant; nothing is sent.
    sock.connect((peer, 9))
        .map_err(|e| Error::io(format!("iface: no route to {peer}"), e))?;
    let local = sock
        .local_addr()
        .map_err(|e| Error::io("iface: local address", e))?
        .ip();
    if local.is_unspecified() {
        return Err(Error::not_found(format!(
            "iface: no local address for {peer}"
        )));
    }
    Ok(local)
}

/// The IPv4 address of the default-route interface.
///
/// Tries a public address first (the default route), then the mDNS group
/// (the multicast route, which exists on LAN-only hosts too).
pub fn default_ipv4() -> Result<Ipv4Addr> {
    let probes = [Ipv4Addr::new(192, 0, 2, 1), super::mdns::MDNS_GROUP];
    let mut last = None;
    for peer in probes {
        match local_addr_for(IpAddr::V4(peer)) {
            Ok(IpAddr::V4(a)) if !a.is_loopback() => return Ok(a),
            Ok(_) => {}
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| Error::not_found("iface: no non-loopback IPv4 address")))
}

/// Addresses to browse from: the default interface plus `extra`, without
/// duplicates. Falls back to the unspecified address (OS choice) if there
/// is no default route and nothing was given.
pub fn browse_addrs(extra: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    match default_ipv4() {
        Ok(a) => out.push(a),
        Err(e) => debug!("no default IPv4 interface: {e}"),
    }
    for &a in extra {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    if out.is_empty() {
        out.push(Ipv4Addr::UNSPECIFIED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_reaches_loopback() {
        let a = local_addr_for(IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        assert!(a.is_loopback());
    }

    #[test]
    fn browse_addrs_dedups_and_never_empty() {
        let extra = [Ipv4Addr::new(10, 9, 8, 7), Ipv4Addr::new(10, 9, 8, 7)];
        let addrs = browse_addrs(&extra);
        assert_eq!(addrs.iter().filter(|a| **a == extra[0]).count(), 1);
        assert!(!addrs.is_empty());
    }
}
