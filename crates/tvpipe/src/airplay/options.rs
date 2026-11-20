//! What a mirroring connection is set up with ([`MirrorOptions`]): always
//! compiled, so sessions can plan for AirPlay receivers without the
//! `airplay` feature.

use std::time::Duration;

/// Options for a mirroring session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorOptions {
    /// `--latency`: explicit leads (else the normal profile, §2.7).
    pub latency_ms: Option<u32>,
    /// Whether the encode will carry audio for this receiver.
    pub audio: bool,
    /// Our name in SETUP.
    pub name: String,
    /// Allow PTP (else always NTP).
    pub ptp: bool,
    /// No answer to `POST /feedback` (sent every 2 s) for this long: the
    /// receiver is lost (`push` fails with a timeout).
    pub feedback_timeout: Duration,
    /// Plan up to the receiver's decoder maximum (`widthPixelsMax`),
    /// not only its nominal canvas: a resolution chosen explicitly
    /// (`share URL --resolution 1080p|4k`; docs/airplay-mirroring.md §2.6).
    pub decoder_max: bool,
}

/// Default [`MirrorOptions::feedback_timeout`]: three missed `/feedback`
/// rounds.
pub const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(7);

impl Default for MirrorOptions {
    fn default() -> MirrorOptions {
        MirrorOptions {
            latency_ms: None,
            audio: true,
            name: "tvpipe".into(),
            ptp: true,
            feedback_timeout: FEEDBACK_TIMEOUT,
            decoder_max: false,
        }
    }
}

/// A receiver as its `/info` names it (`deviceID`, `sourceVersion`): what
/// the PTP memory keeps (docs/airplay-mirroring.md §2.5). A software
/// update gives it a new key, so it gets PTP again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverKey {
    pub device_id: String,
    pub source_version: String,
}

/// Samples per ALAC frame on the wire.
pub const ALAC_FRAME_SAMPLES: u32 = 352;
