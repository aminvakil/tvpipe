//! H.264 framing for Cast: receivers decode Annex B byte streams (start
//! codes, parameter sets in band), while our frames are AVCC with the
//! parameter sets in the `avcC` record. Parsing is `media::h264`'s; this
//! only re-frames.
//!
//! Sources: ISO/IEC 14496-15 §5.3.3.1 (AVCDecoderConfigurationRecord),
//! ITU-T H.264 Annex B (byte stream format).

use crate::error::{Error, Result};
pub use crate::media::h264::AvcConfig;
use crate::media::h264::{avcc_nals, nal_type, nal_unit_type};

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Converts one AVCC access unit to Annex B. With `keyframe`, the SPS/PPS
/// from `cfg` are prepended unless the unit already carries an SPS.
pub fn avcc_to_annexb(
    data: &[u8],
    cfg: &AvcConfig,
    keyframe: bool,
    out: &mut Vec<u8>,
) -> Result<()> {
    out.clear();
    let mut nals = Vec::new();
    let mut has_sps = false;
    for nal in avcc_nals(data, cfg.length_size as usize) {
        let nal = nal?;
        if nal.is_empty() {
            continue;
        }
        has_sps |= nal_unit_type(nal) == Some(nal_type::SPS);
        nals.push(nal);
    }
    if nals.is_empty() {
        return Err(Error::malformed("avcc frame: no NAL units"));
    }
    if keyframe && !has_sps {
        for ps in cfg.sps.iter().chain(&cfg.pps) {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(ps);
        }
    }
    for nal in nals {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVCC: [u8; 19] = [
        1, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 4, 0x67, 0x64, 0, 0x1f, 1, 0, 4, 0x68, 0xeb, 0xe3, 0xcb,
    ];

    #[test]
    fn parses_avcc() {
        let c = AvcConfig::parse(&AVCC).unwrap();
        assert_eq!(c.length_size, 4);
        assert_eq!(c.sps, vec![vec![0x67, 0x64, 0, 0x1f]]);
        assert_eq!(c.pps, vec![vec![0x68, 0xeb, 0xe3, 0xcb]]);
        assert!(AvcConfig::parse(&AVCC[..10]).is_err());
        assert!(AvcConfig::parse(&[0]).is_err());
    }

    #[test]
    fn converts_frames() {
        let c = AvcConfig::parse(&AVCC).unwrap();
        let frame = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x06];
        let mut out = Vec::new();
        avcc_to_annexb(&frame, &c, true, &mut out).unwrap();
        assert_eq!(
            out,
            [
                0, 0, 0, 1, 0x67, 0x64, 0, 0x1f, 0, 0, 0, 1, 0x68, 0xeb, 0xe3, 0xcb, 0, 0, 0, 1,
                0x65, 0xaa, 0, 0, 0, 1, 0x06
            ]
        );
        avcc_to_annexb(&frame, &c, false, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x06]);
        // In-band SPS: nothing prepended.
        let with_sps = [0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x65];
        avcc_to_annexb(&with_sps, &c, true, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x65]);
        // Zero-length NAL units are skipped.
        avcc_to_annexb(&[0, 0, 0, 0, 0, 0, 0, 1, 0x41], &c, false, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x41]);
        // Truncated or empty input is an error.
        assert!(avcc_to_annexb(&frame[..5], &c, false, &mut out).is_err());
        assert!(avcc_to_annexb(&[], &c, false, &mut out).is_err());
    }
}
