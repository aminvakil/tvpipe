//! Audio format helpers every input and sink shares (always compiled,
//! unlike the file demuxers they came from).

use crate::error::{Error, Result};
use crate::types::{AudioCodec, AudioFamily, AudioFormat, Capabilities};

/// (audio object type, sample rate, channels, samples per frame) from an
/// AudioSpecificConfig (ISO/IEC 14496-3 §1.6.2.1).
pub fn parse_asc(asc: &[u8]) -> Result<(u32, u32, u8, u32)> {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let mut at = 0usize;
    let mut bits = |n: usize| -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let b = asc
                .get(at / 8)
                .ok_or_else(|| Error::malformed("aac: truncated AudioSpecificConfig"))?;
            v = (v << 1) | ((b >> (7 - at % 8)) & 1) as u32;
            at += 1;
        }
        Ok(v)
    };
    let mut aot = bits(5)?;
    if aot == 31 {
        aot = 32 + bits(6)?;
    }
    let idx = bits(4)?;
    let rate = if idx == 15 {
        bits(24)?
    } else {
        *RATES
            .get(idx as usize)
            .ok_or_else(|| Error::malformed("aac: reserved sampling frequency index"))?
    };
    let channels = bits(4)? as u8;
    let frame = match aot {
        // GASpecificConfig frameLengthFlag.
        1..=4 | 6 | 7 | 17 | 19..=23 => {
            if bits(1)? == 1 {
                960
            } else {
                1024
            }
        }
        // ELDSpecificConfig frameLengthFlag.
        39 => {
            if bits(1)? == 1 {
                480
            } else {
                512
            }
        }
        _ => 1024,
    };
    Ok((aot, rate, channels, frame))
}

/// The audio formats a sink's `start()` takes as input, which is not
/// always what it advertises: the AirPlay sink lists ALAC (what goes on
/// the wire) but takes 44.1 kHz s16le PCM and wraps it as verbatim ALAC
/// (`airplay::sink`, and the session's ffmpeg plan does the same).
pub fn sink_input_formats(c: &Capabilities) -> Vec<AudioFormat> {
    c.audio_formats
        .iter()
        .map(|f| match (c.audio_family, f.codec) {
            (AudioFamily::AirPlay, AudioCodec::Alac) => AudioFormat {
                codec: AudioCodec::PcmS16le,
                ..*f
            },
            _ => *f,
        })
        .collect()
}

/// Whether a sink's `start()` takes `fmt` (see [`sink_input_formats`]).
pub fn sink_takes(c: &Capabilities, fmt: &AudioFormat) -> bool {
    sink_input_formats(c).contains(fmt)
}
