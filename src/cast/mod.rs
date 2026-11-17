//! Google Cast Streaming sender (PLAN §8).
//!
//! Owner: W8.
//!
//! - [`channel`]: TLS control channel, framing, heartbeat.
//! - [`messages`]: `tp.connection` / `tp.heartbeat` / `receiver` payloads.
//! - [`rtp`]: Cast RTP packets, frame ids, AES-CTR frame crypto.
//! - [`rtcp`]: Sender Reports, receiver feedback (ACK/NACK/PLI/logs).
//! - [`streaming`]: OFFER/ANSWER and the negotiated session.
//! - [`retransmit`]: per-stream frame history, ACK/NACK, kickstart, limits.
//! - [`sink`]: [`StreamingSink`] (the `types::Sink`) and [`connect`].
//! - `annexb`: AVCC → Annex B for H.264 frames.

mod annexb;
pub mod channel;
pub mod messages;
pub mod retransmit;
pub mod rtcp;
pub mod rtp;
pub mod sink;
pub mod streaming;

#[cfg(test)]
mod fake_receiver;
#[cfg(test)]
mod testing;

#[allow(unused_imports)] // wired into session.rs by the lead
pub use sink::{connect, connect_device, CastOptions, StreamingSink};
