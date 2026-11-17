//! H.264 framing for Cast: receivers decode Annex B byte streams (start
//! codes, parameter sets in band), while our frames are AVCC with the
//! parameter sets in the `avcC` record.
//!
//! This is a deliberately small, private converter; the general H.264
//! helpers belong to `media::h264` (W4).
//!
//! Sources: ISO/IEC 14496-15 §5.3.3.1 (AVCDecoderConfigurationRecord),
//! ITU-T H.264 Annex B (byte stream format).

use crate::error::{Error, Result};
use crate::util::bytes::Reader;

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;

/// The parts of an `avcC` record we need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvcConfig {
    /// Bytes per NAL length prefix (1, 2 or 4).
    pub length_size: usize,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

impl AvcConfig {
    pub fn parse(avcc: &[u8]) -> Result<AvcConfig> {
        let mut r = Reader::new(avcc, "avcC");
        if r.u8()? != 1 {
            return Err(r.error("unsupported configurationVersion"));
        }
        r.skip(3)?; // profile, compatibility, level
        let length_size = (r.u8()? & 0x03) as usize + 1;
        if length_size == 3 {
            return Err(r.error("3-byte NAL lengths are not allowed"));
        }
        let mut sps = Vec::new();
        for _ in 0..(r.u8()? & 0x1f) {
            let n = r.u16_be()? as usize;
            sps.push(r.bytes(n)?.to_vec());
        }
        let mut pps = Vec::new();
        for _ in 0..r.u8()? {
            let n = r.u16_be()? as usize;
            pps.push(r.bytes(n)?.to_vec());
        }
        Ok(AvcConfig {
            length_size,
            sps,
            pps,
        })
    }
}

impl Default for AvcConfig {
    fn default() -> AvcConfig {
        AvcConfig {
            length_size: 4,
            sps: Vec::new(),
            pps: Vec::new(),
        }
    }
}

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
    let mut r = Reader::new(data, "avcc frame");
    let mut has_sps = false;
    while !r.is_at_end() {
        let n = r.uint_be(cfg.length_size)? as usize;
        if n == 0 {
            continue;
        }
        let nal = r.bytes(n)?;
        has_sps |= nal[0] & 0x1f == NAL_SPS;
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

/// True if an Annex B or AVCC unit's first NAL looks like a parameter set
/// (diagnostics only).
pub fn is_parameter_set(nal: &[u8]) -> bool {
    nal.first()
        .is_some_and(|b| matches!(b & 0x1f, NAL_SPS | NAL_PPS))
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
        // Truncated or empty input is an error.
        assert!(avcc_to_annexb(&frame[..5], &c, false, &mut out).is_err());
        assert!(avcc_to_annexb(&[], &c, false, &mut out).is_err());
        assert!(is_parameter_set(&[0x68]));
        assert!(!is_parameter_set(&[0x65]));
    }
}
