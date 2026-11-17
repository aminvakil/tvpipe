//! Networking: mDNS/DNS, interface selection, HTTP/RTSP framing, discovery.
//!
//! Owner: W3 (PLAN §3, §6).

pub mod device;
pub mod discover;
pub mod dns;
pub mod http;
pub mod iface;
pub mod mdns;
pub mod rtsp;
