//! Google Cast Streaming sender (PLAN §8).
//!
//! Owner: W8.
//!
//! - [`channel`]: TLS control channel, framing, heartbeat.
//! - [`messages`]: `tp.connection` / `tp.heartbeat` / `receiver` payloads.
//! - [`rtp`]: Cast RTP packets, frame ids, AES-CTR frame crypto.
//! - [`rtcp`]: Sender Reports, receiver feedback (ACK/NACK/PLI/logs).

pub mod channel;
pub mod messages;
pub mod rtcp;
pub mod rtp;

#[cfg(test)]
mod testing;
