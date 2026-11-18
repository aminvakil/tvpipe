//! Networking: mDNS/DNS, interface selection, HTTP/RTSP framing, discovery,
//! and the loopback HTTP server and WebSocket used by the browser share (W12).
//!
//! Owner: W3 (PLAN §3, §6).

pub mod base64;
pub mod cftunnel;
pub mod device;
pub mod discover;
pub mod dns;
pub mod http;
pub mod httpd;
pub mod iface;
pub mod mdns;
pub mod opentunnel;
pub mod rtsp;
pub mod tunnel;
pub mod websocket;
