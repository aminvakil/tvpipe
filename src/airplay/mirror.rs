//! The mirroring video data channel (spec §6): 128-byte packet headers,
//! the codec (avcC) packet, AVCC frame payloads, the two video ciphers
//! and heartbeats.
//!
//! ```text
//! 0   u32le payload_size      4 u8 type (0 frame, 1 codec, 2 heartbeat)
//! 5   u8 0x10 = IDR (frames)   6 u8 option   7 u8 (codec: 0x01)
//! 8   u64le timestamp (32.32 s on the session timeline)
//! 16.. type-specific (codec: f32le sizes; frame: timeline id at 40)
//! ```
//!
//! Sources: docs/spec/airplay-mirroring.md §5.1, §5.2, §6.1–§6.4; vectors
//! tests/vectors/airplay-mirror/video.txt, appletv-session.txt.

use crate::crypto::modes::Ctr;
use crate::crypto::ring::{nonce_counter, ChaCha20Poly1305, CHACHA_KEY_LEN, CHACHA_TAG_LEN};
use crate::error::{Error, Result};
use crate::media::h264::{self, nal_type};

pub const HEADER_LEN: usize = 128;
pub type Header = [u8; HEADER_LEN];

pub const TYPE_FRAME: u8 = 0x00;
pub const TYPE_CODEC: u8 = 0x01;
pub const TYPE_HEARTBEAT: u8 = 0x02;
const FLAG_IDR: u8 = 0x10;
const OPTION_H264: u8 = 0x16;
const OPTION_HEARTBEAT: u8 = 0x1e;

fn base(payload_len: usize, kind: u8, ts: u64) -> Result<Header> {
    let len = u32::try_from(payload_len)
        .map_err(|_| Error::unsupported("airplay: video payload over 4 GiB"))?;
    let mut h = [0u8; HEADER_LEN];
    h[0..4].copy_from_slice(&len.to_le_bytes());
    h[4] = kind;
    h[8..16].copy_from_slice(&ts.to_le_bytes());
    Ok(h)
}

/// The header of an H.264 codec packet (§6.1 type 1): encoded size in all
/// three size pairs, zero origin.
pub fn codec_header(payload_len: usize, ts: u64, width: u32, height: u32) -> Result<Header> {
    let mut h = base(payload_len, TYPE_CODEC, ts)?;
    h[6] = OPTION_H264;
    h[7] = 0x01;
    let (w, ht) = ((width as f32).to_le_bytes(), (height as f32).to_le_bytes());
    for off in [16, 40, 56] {
        h[off..off + 4].copy_from_slice(&w);
        h[off + 4..off + 8].copy_from_slice(&ht);
    }
    Ok(h)
}

/// The header of a video frame (§6.1 type 0). `payload_len` is what
/// follows on the wire (ChaCha: ciphertext + tag).
pub fn frame_header(payload_len: usize, ts: u64, idr: bool, timeline: u64) -> Result<Header> {
    let mut h = base(payload_len, TYPE_FRAME, ts)?;
    if idr {
        h[5] = FLAG_IDR;
    }
    h[40..48].copy_from_slice(&timeline.to_le_bytes());
    Ok(h)
}

/// A heartbeat packet (§6.4).
pub fn heartbeat() -> Header {
    let mut h = [0u8; HEADER_LEN];
    h[4] = TYPE_HEARTBEAT;
    h[6] = OPTION_HEARTBEAT;
    h
}

/// The codec packet payload (§6.2): avcC with one SPS and one PPS,
/// 4-byte NAL lengths, plus the `02 00 00 00` trailer.
pub fn avcc_record(sps: &[u8], pps: &[u8]) -> Result<Vec<u8>> {
    if sps.len() < 4 || pps.is_empty() {
        return Err(Error::malformed("airplay: SPS or PPS too short"));
    }
    let len16 = |n: usize| {
        u16::try_from(n)
            .map(u16::to_be_bytes)
            .map_err(|_| Error::malformed("airplay: parameter set over 64 KiB"))
    };
    let mut out = Vec::with_capacity(15 + sps.len() + pps.len());
    out.extend_from_slice(&[0x01, sps[1], sps[2], sps[3], 0xff, 0xe1]);
    out.extend_from_slice(&len16(sps.len())?);
    out.extend_from_slice(sps);
    out.push(0x01);
    out.extend_from_slice(&len16(pps.len())?);
    out.extend_from_slice(pps);
    out.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
    Ok(out)
}

/// One access unit ready for the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePayload {
    /// AVCC (4-byte lengths) without SPS, PPS and AUD.
    pub data: Vec<u8>,
    pub idr: bool,
    /// In-band parameter sets, if the access unit carried them.
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

/// Converts an AVCC access unit (`length_size`-byte lengths) to the
/// frame payload of §6.3.
pub fn frame_payload(au: &[u8], length_size: usize) -> Result<FramePayload> {
    let mut out = FramePayload {
        data: Vec::with_capacity(au.len() + 16),
        idr: false,
        sps: None,
        pps: None,
    };
    for nal in h264::avcc_nals(au, length_size) {
        let nal = nal?;
        match h264::nal_unit_type(nal) {
            Some(nal_type::SPS) => {
                out.sps.get_or_insert_with(|| nal.to_vec());
            }
            Some(nal_type::PPS) => {
                out.pps.get_or_insert_with(|| nal.to_vec());
            }
            Some(nal_type::AUD) | None => {}
            Some(t) => {
                out.idr |= t == nal_type::IDR;
                let len = u32::try_from(nal.len())
                    .map_err(|_| Error::malformed("airplay: NAL unit over 4 GiB"))?;
                out.data.extend_from_slice(&len.to_be_bytes());
                out.data.extend_from_slice(nal);
            }
        }
    }
    Ok(out)
}

/// The video payload cipher (§6.3).
pub enum VideoCipher {
    /// HAP profile: ChaCha20-Poly1305, AAD = header, counter nonce.
    ChaCha {
        cipher: ChaCha20Poly1305,
        counter: u64,
    },
    /// Legacy profile: one AES-128-CTR keystream over all frame payloads.
    AesCtr(Ctr),
}

impl std::fmt::Debug for VideoCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VideoCipher::ChaCha { .. } => "VideoCipher::ChaCha",
            VideoCipher::AesCtr(_) => "VideoCipher::AesCtr",
        })
    }
}

impl VideoCipher {
    pub fn chacha(key: &[u8; CHACHA_KEY_LEN]) -> VideoCipher {
        VideoCipher::ChaCha {
            cipher: ChaCha20Poly1305::new(key),
            counter: 0,
        }
    }

    pub fn aes_ctr(key: &[u8; 16], iv: &[u8; 16]) -> Result<VideoCipher> {
        Ok(VideoCipher::AesCtr(Ctr::with_key(key, iv)?))
    }

    /// Bytes the cipher adds to a payload.
    pub fn overhead(&self) -> usize {
        match self {
            VideoCipher::ChaCha { .. } => CHACHA_TAG_LEN,
            VideoCipher::AesCtr(_) => 0,
        }
    }

    /// Builds the whole frame packet (header ‖ encrypted payload).
    pub fn frame_packet(
        &mut self,
        mut payload: Vec<u8>,
        ts: u64,
        idr: bool,
        timeline: u64,
    ) -> Result<Vec<u8>> {
        let header = frame_header(payload.len() + self.overhead(), ts, idr, timeline)?;
        match self {
            VideoCipher::ChaCha { cipher, counter } => {
                cipher.seal_in_place(&nonce_counter(*counter), &header, &mut payload)?;
                *counter = counter
                    .checked_add(1)
                    .ok_or_else(|| Error::crypto("airplay: video nonce exhausted"))?;
            }
            VideoCipher::AesCtr(ctr) => ctr.apply(&mut payload),
        }
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&payload);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    fn block(name: &str) -> vectors::Block {
        vectors::load("airplay-mirror/video.txt")
            .into_iter()
            .find(|b| b.name() == name)
            .unwrap_or_else(|| panic!("no block {name}"))
    }

    fn u64le(v: &[u8]) -> u64 {
        u64::from_le_bytes(v.try_into().unwrap())
    }

    #[test]
    fn codec_packet() {
        let b = block("avcC record for codec packet");
        let rec = avcc_record(&b.hex("sps"), &b.hex("pps")).unwrap();
        assert_eq!(rec, b.hex("payload"));
        let b = block("codec packet (H.264), 1920x1080");
        let h = codec_header(rec.len(), u64le(&b.hex("timestamp")), 1920, 1080).unwrap();
        assert_eq!(h.to_vec(), b.hex("header"));
        assert_eq!(rec, b.hex("payload"));
    }

    #[test]
    fn chacha_frames() {
        let mut cipher = None;
        for (i, name) in [
            "ChaCha20-Poly1305 video frame 0 (nonce counter 0)",
            "ChaCha20-Poly1305 video frame 1 (nonce counter 1)",
        ]
        .into_iter()
        .enumerate()
        {
            let b = block(name);
            let key: [u8; 32] = b.hex("key").try_into().unwrap();
            let c = cipher.get_or_insert_with(|| VideoCipher::chacha(&key));
            assert_eq!(b.parse::<u64>("nonce_counter"), i as u64);
            let header = b.hex("header");
            let ts = u64le(&header[8..16]);
            let timeline = u64le(&header[40..48]);
            let idr = header[5] == 0x10;
            let pkt = c
                .frame_packet(b.hex("plaintext"), ts, idr, timeline)
                .unwrap();
            assert_eq!(pkt[..HEADER_LEN].to_vec(), header, "{name}");
            assert_eq!(
                pkt[HEADER_LEN..].to_vec(),
                b.hex("ciphertext_and_tag"),
                "{name}"
            );
        }
    }

    #[test]
    fn aes_ctr_continuous_keystream() {
        let mut cipher = None;
        for i in 0..5 {
            let b = block(&format!(
                "AES-128-CTR video payload {i} (continuous keystream)"
            ));
            let key: [u8; 16] = b.hex("key").try_into().unwrap();
            let iv: [u8; 16] = b.hex("iv").try_into().unwrap();
            let c = cipher.get_or_insert_with(|| VideoCipher::aes_ctr(&key, &iv).unwrap());
            let pt = b.hex("plaintext");
            let pkt = c.frame_packet(pt.clone(), 1, false, 0).unwrap();
            assert_eq!(
                u32::from_le_bytes(pkt[..4].try_into().unwrap()) as usize,
                pt.len()
            );
            assert_eq!(
                pkt[HEADER_LEN..].to_vec(),
                b.hex("ciphertext"),
                "payload {i}"
            );
        }
    }

    #[test]
    fn plain_headers() {
        let b = block(
            "video frame header, NTP timing (timeline 0), non-IDR, AES-CTR mode (payload as-is)",
        );
        let want = b.hex("header");
        let h = frame_header(5, u64le(&want[8..16]), false, 0).unwrap();
        assert_eq!(h.to_vec(), want);
        assert_eq!(
            heartbeat().to_vec(),
            block("heartbeat packet").hex("header")
        );
    }

    #[test]
    fn appletv_accepted_headers() {
        let blocks = vectors::load("airplay-mirror/appletv-session.txt");
        let get = |n: &str| blocks.iter().find(|b| b.name() == n).unwrap().hex("bytes");
        let codec = get("sent codec packet header (accepted by the Apple TV)");
        let payload = get("sent codec packet payload (avcC) (accepted by the Apple TV)");
        let h = codec_header(payload.len(), u64le(&codec[8..16]), 1920, 1080).unwrap();
        assert_eq!(h.to_vec(), codec);
        let frame = get("sent video frame header (accepted by the Apple TV)");
        let h = frame_header(
            66802 + 16,
            u64le(&frame[8..16]),
            true,
            0xaabb_ccff_fe00_000a,
        )
        .unwrap();
        assert_eq!(h.to_vec(), frame);
        assert_eq!(
            heartbeat().to_vec(),
            get("sent heartbeat (accepted by the Apple TV)")
        );
    }

    #[test]
    fn strips_parameter_sets_and_delimiters() {
        let nal = |t: u8, n: usize| {
            let mut v = [(n as u32 + 1).to_be_bytes().to_vec(), vec![0x60 | t]].concat();
            v.extend(std::iter::repeat_n(0xab, n));
            v
        };
        let au = [
            nal(9, 1),
            nal(7, 4),
            nal(8, 2),
            nal(6, 3),
            nal(5, 10),
            nal(5, 4),
        ]
        .concat();
        let p = frame_payload(&au, 4).unwrap();
        assert!(p.idr);
        assert_eq!(p.data, [nal(6, 3), nal(5, 10), nal(5, 4)].concat());
        assert_eq!(p.sps.as_deref().map(<[u8]>::len), Some(5));
        assert_eq!(p.pps.as_deref().map(<[u8]>::len), Some(3));
        let p = frame_payload(&nal(1, 3), 4).unwrap();
        assert!(!p.idr && p.sps.is_none());
        // Truncated input is an error, never a panic.
        assert!(frame_payload(&au[..au.len() - 1], 4).is_err());
        // 2-byte lengths are rewritten to 4-byte ones.
        let p = frame_payload(&[0, 2, 0x65, 1], 2).unwrap();
        assert_eq!(p.data, [0, 0, 0, 2, 0x65, 1]);
        assert!(avcc_record(&[0x67], &[0x68]).is_err());
    }
}
