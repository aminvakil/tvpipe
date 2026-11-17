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

pub mod channel;
pub mod messages;
pub mod retransmit;
pub mod rtcp;
pub mod rtp;
pub mod streaming;

#[cfg(test)]
mod testing;
