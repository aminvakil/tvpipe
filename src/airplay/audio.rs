//! Realtime mirroring audio (spec §8): verbatim ALAC frames, RTP packets,
//! the two payload ciphers, RFC 2198 redundancy, sync (TimeAnnounce)
//! packets and retransmission.
//!
//! Byte layouts only; the sending loop is in `airplay::sink`.
//!
//! Sources: docs/airplay-mirroring.md §8.1–§8.7; RFC 3550 §5.1 (RTP
//! header); RFC 2198 §3 (redundant block headers); vectors
//! tests/vectors/airplay-mirror/audio.txt, appletv-session.txt.

use std::collections::VecDeque;

use crate::crypto::aes::{Aes, BLOCK_LEN};
use crate::crypto::modes::cbc_encrypt;
use crate::crypto::ring::{nonce_counter, ChaCha20Poly1305, CHACHA_KEY_LEN};
use crate::error::{Error, Result};

/// Samples per ALAC frame on the wire.
pub const ALAC_FRAME_SAMPLES: u32 = 352;
pub const SAMPLE_RATE: u32 = 44_100;
pub const PT_AUDIO: u8 = 96;
pub const PT_REDUNDANT: u8 = 97;
pub const RTP_HEADER_LEN: usize = 12;
pub const MAX_DATAGRAM: usize = 1472;
/// Tag (16) + nonce counter (8).
pub const CHACHA_OVERHEAD: usize = 24;
pub const HISTORY_PACKETS: usize = 512;
const MAX_RED_BLOCKS: usize = 2;

// ---------------------------------------------------------------------------
// ALAC

/// MSB-first bit writer.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn with_capacity(n: usize) -> Bits {
        Bits {
            out: Vec::with_capacity(n),
            acc: 0,
            n: 0,
        }
    }

    /// Appends the low `width` (≤ 32) bits of `v`.
    fn put(&mut self, v: u32, width: u32) {
        self.acc = (self.acc << width) | (v as u64 & ((1u64 << width) - 1));
        self.n += width;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

/// Wraps interleaved s16le PCM as one verbatim (uncompressed) ALAC frame
/// (§8.7). `channels` is 1 or 2; a short input is padded with silence to
/// [`ALAC_FRAME_SAMPLES`].
pub fn alac_verbatim(pcm_s16le: &[u8], channels: u8) -> Result<Vec<u8>> {
    let ch = channels as usize;
    if !(1..=2).contains(&ch) {
        return Err(Error::unsupported(format!(
            "alac: {channels} channels (only mono and stereo)"
        )));
    }
    let frame_bytes = ALAC_FRAME_SAMPLES as usize * ch * 2;
    if pcm_s16le.len() > frame_bytes || pcm_s16le.len() % (2 * ch) != 0 {
        return Err(Error::malformed(format!(
            "alac: {} bytes of PCM is not at most one {}-sample frame",
            pcm_s16le.len(),
            ALAC_FRAME_SAMPLES
        )));
    }
    let mut b = Bits::with_capacity(frame_bytes + 8);
    b.put(if ch == 2 { 1 } else { 0 }, 3); // element tag: CPE / SCE
    b.put(0, 4); // element instance
    b.put(0, 12); // unused
    b.put(1, 1); // has size
    b.put(0, 2); // no shifted bytes
    b.put(1, 1); // not compressed
    b.put(ALAC_FRAME_SAMPLES, 32);
    for i in 0..ALAC_FRAME_SAMPLES as usize * ch {
        let s = match pcm_s16le.get(2 * i..2 * i + 2) {
            Some(s) => u16::from_le_bytes([s[0], s[1]]),
            None => 0,
        };
        b.put(s as u32, 16);
    }
    b.put(7, 3); // end tag
    Ok(b.finish())
}

// ---------------------------------------------------------------------------
// RTP

/// A 12-byte RTP header with SSRC 0 and no marker (§8.2).
pub fn rtp_header(pt: u8, seq: u16, ts: u32) -> [u8; RTP_HEADER_LEN] {
    let mut h = [0u8; RTP_HEADER_LEN];
    h[0] = 0x80;
    h[1] = pt & 0x7f;
    h[2..4].copy_from_slice(&seq.to_be_bytes());
    h[4..8].copy_from_slice(&ts.to_be_bytes());
    h
}

/// The audio payload cipher (§8.3).
pub enum AudioCipher {
    /// HAP profile: ChaCha20-Poly1305 with the random audio key.
    ChaCha {
        cipher: ChaCha20Poly1305,
        counter: u64,
    },
    /// Legacy profile: AES-128-CBC over whole blocks, IV per packet.
    AesCbc { aes: Aes, iv: [u8; 16] },
    /// No FairPlay, no HAP.
    Clear,
}

impl std::fmt::Debug for AudioCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AudioCipher::ChaCha { .. } => "AudioCipher::ChaCha",
            AudioCipher::AesCbc { .. } => "AudioCipher::AesCbc",
            AudioCipher::Clear => "AudioCipher::Clear",
        })
    }
}

impl AudioCipher {
    pub fn chacha(key: &[u8; CHACHA_KEY_LEN]) -> AudioCipher {
        AudioCipher::ChaCha {
            cipher: ChaCha20Poly1305::new(key),
            counter: 0,
        }
    }

    pub fn aes_cbc(key: &[u8; 16], iv: &[u8; 16]) -> Result<AudioCipher> {
        Ok(AudioCipher::AesCbc {
            aes: Aes::new(key)?,
            iv: *iv,
        })
    }

    pub fn overhead(&self) -> usize {
        match self {
            AudioCipher::ChaCha { .. } => CHACHA_OVERHEAD,
            _ => 0,
        }
    }

    /// Builds a packet: `header ‖ clear ‖ seal(secret)`. `clear` is the
    /// RFC 2198 header area (empty for PT 96).
    pub fn packet(
        &mut self,
        header: &[u8; RTP_HEADER_LEN],
        clear: &[u8],
        secret: &[u8],
    ) -> Result<Vec<u8>> {
        let mut out =
            Vec::with_capacity(RTP_HEADER_LEN + clear.len() + secret.len() + self.overhead());
        out.extend_from_slice(header);
        out.extend_from_slice(clear);
        match self {
            AudioCipher::ChaCha { cipher, counter } => {
                let mut buf = secret.to_vec();
                cipher.seal_in_place(&nonce_counter(*counter), &header[4..12], &mut buf)?;
                out.extend_from_slice(&buf);
                out.extend_from_slice(&counter.to_le_bytes());
                *counter = counter
                    .checked_add(1)
                    .ok_or_else(|| Error::crypto("airplay: audio nonce exhausted"))?;
            }
            AudioCipher::AesCbc { aes, iv } => {
                let mut buf = secret.to_vec();
                let whole = buf.len() / BLOCK_LEN * BLOCK_LEN;
                let mut iv = *iv;
                cbc_encrypt(aes, &mut iv, &mut buf[..whole])?;
                out.extend_from_slice(&buf);
            }
            AudioCipher::Clear => out.extend_from_slice(secret),
        }
        Ok(out)
    }
}

/// A frame already sent, kept for RFC 2198 blocks.
#[derive(Debug, Clone)]
struct Sent {
    ts: u32,
    payload: Vec<u8>,
}

/// How frames go out (§8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redundancy {
    /// Every frame once as PT 96.
    None,
    /// Feature 61: PT 97 packets with up to two RFC 2198 blocks.
    Rfc2198,
    /// No feature 61: packets N, N−1, N−2 back to back.
    Resend,
}

/// Packetizes one audio stream: sequence numbers, ciphers, redundancy
/// and the retransmit history.
#[derive(Debug)]
pub struct Packetizer {
    cipher: AudioCipher,
    redundancy: Redundancy,
    seq: u16,
    recent: VecDeque<Sent>,
    /// The last [`HISTORY_PACKETS`] PT 96 packets by sequence.
    history: VecDeque<(u16, Vec<u8>)>,
}

impl Packetizer {
    pub fn new(cipher: AudioCipher, redundancy: Redundancy) -> Packetizer {
        Packetizer {
            cipher,
            redundancy,
            seq: 0,
            recent: VecDeque::new(),
            history: VecDeque::new(),
        }
    }

    /// The datagrams to send for the next frame, in order. Sequence
    /// numbers start at 1.
    pub fn frame(&mut self, ts: u32, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        let mut out = Vec::new();
        match self.redundancy {
            Redundancy::Rfc2198 => {
                let (clear, secret) = self.red_payload(ts, payload);
                let red =
                    self.cipher
                        .packet(&rtp_header(PT_REDUNDANT, seq, ts), &clear, &secret)?;
                let primary = self
                    .cipher
                    .packet(&rtp_header(PT_AUDIO, seq, ts), &[], payload)?;
                out.push(red);
                self.remember(seq, primary);
            }
            Redundancy::None | Redundancy::Resend => {
                let p = self
                    .cipher
                    .packet(&rtp_header(PT_AUDIO, seq, ts), &[], payload)?;
                out.push(p.clone());
                if self.redundancy == Redundancy::Resend {
                    for back in 1..=MAX_RED_BLOCKS as u16 {
                        if let Some(old) = self.lookup(seq.wrapping_sub(back)) {
                            out.push(old.to_vec());
                        }
                    }
                }
                self.remember(seq, p);
            }
        }
        if self.redundancy == Redundancy::Rfc2198 {
            self.recent.push_back(Sent {
                ts,
                payload: payload.to_vec(),
            });
            while self.recent.len() > MAX_RED_BLOCKS {
                self.recent.pop_front();
            }
        }
        Ok(out)
    }

    /// RFC 2198 payload split into the clear header area and the part
    /// that is encrypted (block payloads, then the primary).
    fn red_payload(&self, ts: u32, primary: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut budget = MAX_DATAGRAM
            .saturating_sub(RTP_HEADER_LEN + self.cipher.overhead() + 1 + primary.len());
        let mut chosen: Vec<&Sent> = Vec::new();
        for s in self.recent.iter().rev() {
            let offset = ts.wrapping_sub(s.ts);
            let need = 4 + s.payload.len();
            if offset > 0x3fff || s.payload.len() > 0x3ff || need > budget {
                break;
            }
            budget -= need;
            chosen.push(s);
        }
        chosen.reverse();
        let mut clear = Vec::with_capacity(4 * chosen.len() + 1);
        let mut secret = Vec::new();
        for s in &chosen {
            let offset = ts.wrapping_sub(s.ts);
            let len = s.payload.len() as u32;
            clear.push(0x80 | PT_AUDIO);
            clear.push((offset >> 6) as u8);
            clear.push((((offset & 0x3f) << 2) | (len >> 8)) as u8);
            clear.push(len as u8);
            secret.extend_from_slice(&s.payload);
        }
        clear.push(PT_AUDIO);
        secret.extend_from_slice(primary);
        (clear, secret)
    }

    fn remember(&mut self, seq: u16, packet: Vec<u8>) {
        self.history.push_back((seq, packet));
        while self.history.len() > HISTORY_PACKETS {
            self.history.pop_front();
        }
    }

    fn lookup(&self, seq: u16) -> Option<&[u8]> {
        self.history
            .iter()
            .rev()
            .find(|(s, _)| *s == seq)
            .map(|(_, p)| p.as_slice())
    }

    /// Answers a retransmit request (§8.6); empty for anything else.
    pub fn retransmit(&self, req: &[u8]) -> Vec<Vec<u8>> {
        let Some(r) = RetransmitRequest::parse(req) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0..r.count {
            let seq = r.first.wrapping_add(i);
            let mut pkt = vec![0x80, 0xd6];
            pkt.extend_from_slice(&r.id.to_be_bytes());
            match self.lookup(seq) {
                Some(p) => {
                    pkt.extend_from_slice(p);
                    out.push(pkt);
                }
                None => {
                    pkt.extend_from_slice(&seq.to_be_bytes());
                    pkt.extend_from_slice(&[0, 0]);
                    out.push(pkt);
                    break;
                }
            }
        }
        out
    }
}

/// A receiver's retransmit request (§8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetransmitRequest {
    pub id: u16,
    pub first: u16,
    pub count: u16,
}

impl RetransmitRequest {
    pub fn parse(b: &[u8]) -> Option<RetransmitRequest> {
        if b.len() != 8 || b[0] != 0x80 || b[1] != 0xd5 {
            return None;
        }
        let u = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
        Some(RetransmitRequest {
            id: u(2),
            first: u(4),
            count: u(6),
        })
    }
}

// ---------------------------------------------------------------------------
// Sync packets

/// PTP TimeAnnounce (§8.5): `T` in ns on the receiver timeline.
pub fn sync_ptp(first: bool, rtp_play: u32, t_ns: u64, timeline: u64) -> [u8; 28] {
    let mut p = [0u8; 28];
    p[0] = if first { 0x90 } else { 0x80 };
    p[1] = 0xd7;
    p[2..4].copy_from_slice(&4u16.to_be_bytes());
    p[4..8].copy_from_slice(&rtp_play.to_be_bytes());
    p[8..16].copy_from_slice(&t_ns.to_be_bytes());
    p[16..20].copy_from_slice(&rtp_play.to_be_bytes());
    p[20..28].copy_from_slice(&timeline.to_be_bytes());
    p
}

/// NTP sync (§8.5): `T` is `C + epoch` in 32.32.
pub fn sync_ntp(first: bool, rtp_play: u32, t_ntp: u64, rtp_now: u32) -> [u8; 20] {
    let mut p = [0u8; 20];
    p[0] = if first { 0x90 } else { 0x80 };
    p[1] = 0xd4;
    p[2..4].copy_from_slice(&4u16.to_be_bytes());
    p[4..8].copy_from_slice(&rtp_play.to_be_bytes());
    p[8..16].copy_from_slice(&t_ntp.to_be_bytes());
    p[16..20].copy_from_slice(&rtp_now.to_be_bytes());
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::vectors;

    fn blocks() -> Vec<vectors::Block> {
        vectors::load("airplay-mirror/audio.txt")
    }

    fn block(prefix: &str) -> vectors::Block {
        blocks()
            .into_iter()
            .find(|b| b.name().starts_with(prefix))
            .unwrap_or_else(|| panic!("no block {prefix}"))
    }

    fn key32(b: &vectors::Block) -> [u8; 32] {
        b.hex("key").try_into().unwrap()
    }

    #[test]
    fn alac_verbatim_vector() {
        let b = block("ALAC verbatim frame");
        let frame = alac_verbatim(&b.hex("pcm_s16le"), 2).unwrap();
        assert_eq!(frame.len(), 1416);
        assert_eq!(frame, b.hex("alac"));
        // A short last frame is padded with silence.
        let short = alac_verbatim(&b.hex("pcm_s16le")[..400], 2).unwrap();
        assert_eq!(short.len(), 1416);
        assert_eq!(short[..300], frame[..300]);
        assert!(alac_verbatim(&[0; 1409], 2).is_err());
        assert!(alac_verbatim(&[0; 1412], 2).is_err());
        assert!(alac_verbatim(&[0; 4], 3).is_err());
        assert_eq!(
            alac_verbatim(&[], 1).unwrap().len(),
            (23 + 32 + 352 * 16 + 3usize).div_ceil(8)
        );
    }

    #[test]
    fn chacha_packets() {
        let a = block("ChaCha20-Poly1305 audio packet seq 1");
        let b = block("ChaCha20-Poly1305 audio packet seq 2");
        let mut p = Packetizer::new(AudioCipher::chacha(&key32(&a)), Redundancy::None);
        for blk in [&a, &b] {
            let out = p
                .frame(blk.parse("rtp_timestamp"), &blk.hex("payload"))
                .unwrap();
            assert_eq!(out, vec![blk.hex("packet")], "{}", blk.name());
        }
    }

    #[test]
    fn rfc2198_packet() {
        let b = block("RFC 2198 audio packet seq 3");
        let mut p = Packetizer::new(AudioCipher::chacha(&key32(&b)), Redundancy::Rfc2198);
        // Two earlier frames, so their nonces are 0..3 and this one's 4/5;
        // the vector assumes nonces 2 and 3, so feed the history directly.
        p.seq = 2;
        p.recent.push_back(Sent {
            ts: b.parse("redundant_1_rtp"),
            payload: b.hex("redundant_1"),
        });
        p.recent.push_back(Sent {
            ts: b.parse("redundant_2_rtp"),
            payload: b.hex("redundant_2"),
        });
        if let AudioCipher::ChaCha { counter, .. } = &mut p.cipher {
            *counter = b.parse("nonce_counter_red");
        }
        let ts: u32 = b.parse("rtp_timestamp");
        let (clear, secret) = p.red_payload(ts, &b.hex("primary"));
        assert_eq!([clear.clone(), secret].concat(), b.hex("red_payload_plain"));
        assert_eq!(clear.len(), 9);
        let out = p.frame(ts, &b.hex("primary")).unwrap();
        assert_eq!(out, vec![b.hex("packet_pt97")]);
        assert_eq!(p.lookup(3).unwrap(), b.hex("retransmit_copy_pt96"));
        assert_eq!(
            b.parse::<u64>("nonce_counter_primary_pt96_for_retransmit"),
            3
        );
    }

    #[test]
    fn rfc2198_first_packet_and_budget() {
        let mut p = Packetizer::new(AudioCipher::Clear, Redundancy::Rfc2198);
        let out = p.frame(352, &[1, 2, 3]).unwrap();
        assert_eq!(
            out[0][..],
            [&rtp_header(97, 1, 352)[..], &[0x60, 1, 2, 3]].concat()[..]
        );
        let out = p.frame(704, &[4, 5]).unwrap();
        assert_eq!(out[0][12..], [0xe0, 0x05, 0x80, 0x03, 0x60, 1, 2, 3, 4, 5]);
        // Verbatim-sized frames never fit a redundant block.
        let big = vec![0u8; 1416];
        let mut p = Packetizer::new(AudioCipher::chacha(&[7; 32]), Redundancy::Rfc2198);
        for i in 1..4u32 {
            let out = p.frame(352 * i, &big).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].len(), 12 + 1 + 1416 + 24);
            assert_eq!(out[0][12], 0x60);
        }
    }

    #[test]
    fn resend_scheme() {
        let mut p = Packetizer::new(AudioCipher::Clear, Redundancy::Resend);
        let a = p.frame(352, &[1]).unwrap();
        let b = p.frame(704, &[2]).unwrap();
        let c = p.frame(1056, &[3]).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b, vec![b[0].clone(), a[0].clone()]);
        assert_eq!(c, vec![c[0].clone(), b[0].clone(), a[0].clone()]);
    }

    #[test]
    fn aes_cbc_packet() {
        let b = block("AES-128-CBC audio packet");
        let key: [u8; 16] = b.hex("key").try_into().unwrap();
        let iv: [u8; 16] = b.hex("iv").try_into().unwrap();
        let mut p = Packetizer::new(AudioCipher::aes_cbc(&key, &iv).unwrap(), Redundancy::None);
        let out = p.frame(352, &b.hex("payload")).unwrap();
        assert_eq!(out, vec![b.hex("packet")]);
        // The IV restarts for every packet.
        let again = p.frame(352, &b.hex("payload")).unwrap();
        assert_eq!(again[0][4..], out[0][4..]);
    }

    #[test]
    fn sync_packets() {
        for b in blocks() {
            let name = b.name();
            let rtp_now: u32 = match b.get("rtp_now") {
                Some(v) => v.parse().unwrap(),
                None => continue,
            };
            let l: u32 = b.parse::<u32>("latency_samples").wrapping_neg();
            let play = rtp_now.wrapping_add(l);
            let first = name.contains("first");
            let t = u64::from_be_bytes(b.hex("t_32_32").try_into().unwrap());
            let pkt = if name.starts_with("PTP") {
                let ns: u64 = b.parse("t_ns");
                assert_eq!(crate::airplay::timing::fp_to_ns(t), ns);
                let id = u64::from_be_bytes(b.hex("timeline_id").try_into().unwrap());
                sync_ptp(first, play, ns, id).to_vec()
            } else {
                sync_ntp(first, play, t, rtp_now).to_vec()
            };
            assert_eq!(pkt, b.hex("packet"), "{name}");
        }
    }

    #[test]
    fn retransmit_vector() {
        let b = block("retransmit request");
        let mut p = Packetizer::new(AudioCipher::Clear, Redundancy::None);
        p.remember(1, b.hex("original_seq1"));
        let out = p.retransmit(&b.hex("request"));
        assert_eq!(out, vec![b.hex("response_1"), b.hex("response_2")]);
        assert!(p.retransmit(&[0x80, 0xd5, 0, 1, 0, 1]).is_empty());
        assert!(p.retransmit(&[0x80, 0xd5, 0, 1, 0, 1, 0, 0]).is_empty());
        // At most 512 stored; older ones expire.
        for s in 2..600u16 {
            p.remember(s, vec![s as u8]);
        }
        assert!(p.lookup(1).is_none());
        assert_eq!(p.lookup(599), Some(&[599u16 as u8][..]));
        assert_eq!(p.history.len(), HISTORY_PACKETS);
    }
}
