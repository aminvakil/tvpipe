//! Google Cast Streaming sender (PLAN §8).
//!
//! Owner: W8.
//!
//! - [`channel`]: TLS control channel, framing, heartbeat.
//! - [`messages`]: `tp.connection` / `tp.heartbeat` / `receiver` payloads.

pub mod channel;
pub mod messages;

#[cfg(test)]
mod testing;
