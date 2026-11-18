//! MSB-first bit writer with Exp-Golomb codes, and NAL unit framing with
//! emulation prevention.
//!
//! Sources: ITU-T H.264 §7.2 (u(n), ue(v), se(v)), §9.1 (Exp-Golomb),
//! §7.3.2.11 (rbsp_trailing_bits), §7.4.1 (emulation_prevention_three_byte).

/// Something bits can be written to: a [`BitWriter`], or a
/// [`BitCounter`] that only adds up lengths (for mode decisions).
pub trait BitSink {
    /// Writes the low `nbits` (≤ 32) bits of `value`.
    fn put(&mut self, nbits: u32, value: u32);
    /// Pads with zero bits to the next byte boundary.
    fn align_zero(&mut self);

    #[inline]
    fn flag(&mut self, b: bool) {
        self.put(1, b as u32);
    }

    /// ue(v) for `v` up to `u32::MAX - 1`.
    #[inline]
    fn ue(&mut self, v: u32) {
        let code = v as u64 + 1;
        let len = 64 - code.leading_zeros();
        // len - 1 leading zeros, then `code` in `len` bits.
        if len <= 16 {
            self.put(2 * len - 1, code as u32);
        } else {
            self.put(len - 1, 0);
            self.put(len, code as u32);
        }
    }

    /// se(v).
    #[inline]
    fn se(&mut self, v: i32) {
        self.ue(se_code_num(v));
    }
}

/// Counts bits.
#[derive(Debug, Default, Clone, Copy)]
pub struct BitCounter {
    pub bits: usize,
}

impl BitSink for BitCounter {
    #[inline]
    fn put(&mut self, nbits: u32, _value: u32) {
        self.bits += nbits as usize;
    }

    #[inline]
    fn align_zero(&mut self) {
        self.bits = self.bits.div_ceil(8) * 8;
    }
}

/// Accumulates bits MSB first.
#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    /// Bits pending in `acc` (always < 32 between calls).
    n: u32,
}

impl BitSink for BitWriter {
    #[inline]
    fn put(&mut self, nbits: u32, value: u32) {
        debug_assert!(nbits <= 32);
        if nbits == 0 {
            return;
        }
        let v = (value as u64) & ((1u64 << nbits) - 1);
        self.acc = (self.acc << nbits) | v;
        self.n += nbits;
        if self.n >= 32 {
            self.n -= 32;
            self.buf
                .extend_from_slice(&((self.acc >> self.n) as u32).to_be_bytes());
            self.acc &= (1u64 << self.n) - 1;
        }
    }

    fn align_zero(&mut self) {
        let r = self.n % 8;
        if r != 0 {
            self.put(8 - r, 0);
        }
        self.flush_bytes();
    }
}

impl BitWriter {
    pub fn new() -> BitWriter {
        BitWriter::default()
    }

    pub fn with_capacity(bytes: usize) -> BitWriter {
        BitWriter {
            buf: Vec::with_capacity(bytes),
            acc: 0,
            n: 0,
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.acc = 0;
        self.n = 0;
    }

    /// Bits written so far.
    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.n as usize
    }

    /// Moves whole pending bytes into the buffer.
    fn flush_bytes(&mut self) {
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    pub fn is_aligned(&self) -> bool {
        self.n % 8 == 0
    }

    /// rbsp_trailing_bits(): a one bit, then zeros to the byte boundary.
    pub fn trailing(&mut self) {
        self.put(1, 1);
        self.align_zero();
    }

    /// The bytes written; only whole bytes after [`trailing`] or
    /// [`align_zero`](BitSink::align_zero).
    ///
    /// [`trailing`]: BitWriter::trailing
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// codeNum of se(v) (§9.1.1, Table 9-3).
#[inline]
pub fn se_code_num(v: i32) -> u32 {
    if v > 0 {
        (v as u32) * 2 - 1
    } else {
        v.unsigned_abs() * 2
    }
}

/// Length in bits of ue(v).
#[inline]
pub fn ue_len(v: u32) -> u32 {
    let code = v as u64 + 1;
    2 * (64 - code.leading_zeros()) - 1
}

/// Length in bits of se(v).
#[inline]
pub fn se_len(v: i32) -> u32 {
    ue_len(se_code_num(v))
}

/// Appends one NAL unit (header byte + RBSP with emulation prevention) to
/// `out` as AVCC: a 4-byte big-endian length, then the payload.
pub fn put_nal_avcc(out: &mut Vec<u8>, nal_ref_idc: u8, nal_unit_type: u8, rbsp: &[u8]) {
    let len_pos = out.len();
    out.extend_from_slice(&[0, 0, 0, 0]);
    let start = out.len();
    out.push(((nal_ref_idc & 3) << 5) | (nal_unit_type & 0x1f));
    escape_into(out, rbsp);
    let len = (out.len() - start) as u32;
    out[len_pos..start].copy_from_slice(&len.to_be_bytes());
}

/// Returns one NAL unit (header + escaped RBSP), without length prefix.
pub fn nal_unit(nal_ref_idc: u8, nal_unit_type: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 2);
    out.push(((nal_ref_idc & 3) << 5) | (nal_unit_type & 0x1f));
    escape_into(&mut out, rbsp);
    out
}

/// Inserts emulation_prevention_three_byte after every `00 00` that is
/// followed by a byte ≤ 3 (§7.4.1).
fn escape_into(out: &mut Vec<u8>, rbsp: &[u8]) {
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_matches_writer() {
        let mut w = BitWriter::new();
        let mut c = BitCounter::default();
        for v in [0u32, 5, 77, 1 << 20, 3] {
            w.ue(v);
            c.ue(v);
            w.put(7, v);
            c.put(7, v);
        }
        w.se(-9);
        c.se(-9);
        assert_eq!(w.bit_len(), c.bits);
        w.align_zero();
        c.align_zero();
        assert_eq!(w.bit_len(), c.bits);
        assert_eq!(w.bytes().len() * 8, c.bits);
    }

    #[test]
    fn exp_golomb() {
        // §9.1 Table 9-2: codeNum 0 = 1, 1 = 010, 2 = 011, 3 = 00100.
        let mut w = BitWriter::new();
        w.ue(0);
        w.ue(1);
        w.ue(2);
        w.ue(3);
        // 1 010 011 00100 + trailing 1 + pad = 1010 0110 0100 1000
        w.trailing();
        assert_eq!(w.bytes(), &[0b1010_0110, 0b0100_1000]);
        assert_eq!(ue_len(0), 1);
        assert_eq!(ue_len(3), 5);
        assert_eq!(se_len(-1), 3);
        let mut w = BitWriter::new();
        // se: 1 → codeNum 1 (010), -1 → 2 (011), 0 → 0 (1)
        w.se(1);
        w.se(-1);
        w.se(0);
        w.align_zero();
        assert_eq!(w.bytes(), &[0b0100_1110]);
    }

    #[test]
    fn large_ue() {
        let mut w = BitWriter::new();
        w.ue(1 << 20);
        assert_eq!(w.bit_len(), 41);
        w.ue(u32::MAX - 1);
        assert_eq!(w.bit_len(), 41 + 63);
    }

    #[test]
    fn emulation_prevention() {
        let nal = nal_unit(3, 5, &[0, 0, 0, 0, 0, 1, 0, 0, 4, 0, 0]);
        assert_eq!(nal, vec![0x65, 0, 0, 3, 0, 0, 3, 0, 1, 0, 0, 4, 0, 0]);
        let mut out = Vec::new();
        put_nal_avcc(&mut out, 0, 6, &[0, 0, 2]);
        assert_eq!(out, vec![0, 0, 0, 5, 6, 0, 0, 3, 2]);
    }
}
