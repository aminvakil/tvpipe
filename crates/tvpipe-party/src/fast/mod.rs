//! The WebRTC fast path (docs/multiscreen-design.md §2.4): data channels
//! to phones on the LAN beside the WebSocket, which stays the source of
//! truth and the fallback.
//!
//! [`FastPath`] is the seam: datagrams and offers in; datagrams, channel
//! messages and state out; timeouts. It owns no socket and reads no
//! clock, so tests drive it in memory and the fuzz test feeds it
//! anything. [`rtc::RtcPath`] implements it on the `rtc` crate.
//!
//! Each phone gets two channels, negotiated out of band (no DCEP), so
//! both sides know them by stream id:
//!
//! - `r` (id 0): reliable and ordered; carries the WebSocket's messages.
//! - `u` (id 1): unordered, no retransmits; carries `in` (latest wins).

// Wired into the party in the next step.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Instant;

pub(crate) mod rtc;
pub(crate) mod stun;

/// Whether the fast path may run in this build: not with `panic =
/// "abort"`, since rtc 0.21.1 has panics reachable by a peer that
/// completed DTLS (any player), which [`rtc`] contains by unwinding
/// (docs/multiscreen-design.md §2.5).
pub(crate) fn available() -> bool {
    !cfg!(panic = "abort")
}

/// A peer: the WebSocket connection that signaled it (its lifetime ends
/// with that socket's).
pub(crate) type PeerId = u64;

/// The stream id of the reliable channel.
pub(crate) const RELIABLE_ID: u16 = 0;
/// The stream id of the unreliable channel.
pub(crate) const UNRELIABLE_ID: u16 = 1;

/// Which channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    /// `r`: reliable, ordered.
    Reliable,
    /// `u`: unordered, no retransmits.
    Unreliable,
}

/// A datagram to send.
#[derive(Debug)]
pub(crate) struct Transmit {
    /// The local address (one of the candidates) to send from.
    pub local: SocketAddr,
    pub to: SocketAddr,
    pub data: Vec<u8>,
}

/// What happened to a peer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FastEvent {
    /// Both channels are open.
    Open(PeerId),
    /// A text message on a channel (binary ones are dropped).
    Message {
        peer: PeerId,
        channel: Channel,
        text: String,
    },
    /// The peer is gone (failed, closed, timed out, too slow); it was
    /// forgotten. Not reported after [`FastPath::close`].
    Down { peer: PeerId, why: String },
}

/// A WebRTC stack for many phones, sans-IO.
pub(crate) trait FastPath: Send {
    /// Answers a phone's offer (SDP); the answer carries the host's
    /// candidates (ICE-lite, no trickle). Replaces the peer if it had one.
    fn answer(&mut self, peer: PeerId, offer: &str, now: Instant) -> Result<String, String>;

    /// A datagram that arrived on `local` from `from`.
    fn datagram(&mut self, local: SocketAddr, from: SocketAddr, data: &[u8], now: Instant);

    /// Sends a text message; false if the channel is not open, or the
    /// peer was dropped as too slow (a [`FastEvent::Down`] follows).
    fn send(&mut self, peer: PeerId, channel: Channel, text: &str, now: Instant) -> bool;

    /// Closes a peer (it is told) and forgets it.
    fn close(&mut self, peer: PeerId, now: Instant);

    /// The next instant [`timeout`](FastPath::timeout) wants to run.
    fn next_timeout(&mut self) -> Option<Instant>;

    /// Runs what is due at `now` (retransmissions, ICE consent, give-ups).
    fn timeout(&mut self, now: Instant);

    /// The next datagram to send.
    fn poll_transmit(&mut self) -> Option<Transmit>;

    /// The next event.
    fn poll_event(&mut self) -> Option<FastEvent>;

    /// Peers now (tests, limits).
    fn peers(&self) -> usize;
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
pub(crate) mod tests;
