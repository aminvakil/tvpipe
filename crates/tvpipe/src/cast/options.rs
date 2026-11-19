//! What a Cast connection offers ([`CastOptions`]): always compiled, so
//! sessions can plan for Cast receivers without the `cast` feature.

use std::net::SocketAddr;
use std::time::Duration;

use crate::types::AudioCodec;
use crate::util::ratio::Ratio;

/// Default [`CastOptions::feedback_timeout`]: receivers answer every
/// frame with RTCP within a round trip, so this much silence while media
/// goes out means the receiver is gone (it changed its address, left the
/// network), not a Wi-Fi hiccup.
pub const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(5);

/// How to connect and what to offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastOptions {
    /// Target playout delay (`--latency`).
    pub latency_ms: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: Ratio,
    pub max_bitrate_kbps: u32,
    /// Offer audio: [`CastOptions::audio_codecs`] at
    /// [`CastOptions::audio_sample_rate`] / [`CastOptions::audio_channels`].
    pub audio: bool,
    /// Audio codecs to offer, preferred first (Opus or AAC-LC). Default:
    /// Opus, then AAC. Offer only AAC when that is all the input has
    /// (passthrough of an AAC file, `aacenc`); receivers tend to pick Opus.
    pub audio_codecs: Vec<AudioCodec>,
    /// How AAC access units go on the wire.
    pub aac_framing: AacFraming,
    pub audio_sample_rate: u32,
    pub audio_channels: u8,
    /// Payload types 127/96 like Chrome (some Android TV receivers).
    pub android_rtp_hack: bool,
    /// Pacing rate; default `max(24 Mibit/s, 3 × video bitrate)`.
    pub burst_bitrate: Option<u32>,
    pub connect_timeout: Duration,
    /// Send media to this address instead of the receiver's ANSWERed
    /// port (a relay, e.g. the lossy one in the receiver loop test).
    pub media_relay: Option<SocketAddr>,
    /// No RTCP for this long while RTP goes out: the receiver is lost
    /// (`push` fails with [`ErrorKind::Timeout`]).
    pub feedback_timeout: Duration,
}

impl Default for CastOptions {
    fn default() -> CastOptions {
        CastOptions {
            latency_ms: 200,
            max_width: 1920,
            max_height: 1080,
            max_fps: Ratio::new(60, 1),
            max_bitrate_kbps: 10_000,
            audio: true,
            audio_codecs: vec![AudioCodec::Opus, AudioCodec::AacLc],
            aac_framing: AacFraming::Adts,
            audio_sample_rate: 48_000,
            audio_channels: 2,
            android_rtp_hack: false,
            burst_bitrate: None,
            connect_timeout: Duration::from_secs(5),
            media_relay: None,
            feedback_timeout: FEEDBACK_TIMEOUT,
        }
    }
}

/// AAC framing inside Cast frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AacFraming {
    /// ADTS-framed units (Chromium; openscreen's receiver needs them).
    #[default]
    Adts,
    /// Raw access units, ADTS headers stripped.
    Raw,
}
