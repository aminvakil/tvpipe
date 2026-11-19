//! MSB-first bit sink: a writer, or a counter that only adds up lengths so
//! the rate loop counts exactly what the writer would emit.

pub trait BitSink {
    /// Appends the low `n` bits of `value`, most significant first.
    /// `n <= 32`.
    fn put(&mut self, value: u32, n: u32);
    fn bits(&self) -> usize;
}

/// Counts bits without storing them.
#[derive(Debug, Default)]
pub struct BitCounter(pub usize);

impl BitSink for BitCounter {
    #[inline]
    fn put(&mut self, _value: u32, n: u32) {
        self.0 += n as usize;
    }

    fn bits(&self) -> usize {
        self.0
    }
}

/// Writes bits into a byte vector.
#[derive(Debug, Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nacc: u32,
    total: usize,
}

impl BitWriter {
    pub fn new() -> BitWriter {
        BitWriter::default()
    }

    /// Pads with zero bits to a byte boundary.
    pub fn align(&mut self) {
        let pad = (8 - self.total % 8) % 8;
        self.put(0, pad as u32);
    }

    /// Returns the bytes; a partial last byte is zero-padded.
    pub fn finish(mut self) -> Vec<u8> {
        self.align();
        self.buf
    }
}

impl BitSink for BitWriter {
    #[inline]
    fn put(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        let v = (value as u64) & ((1u64 << n) - 1);
        self.acc = (self.acc << n) | v;
        self.nacc += n;
        self.total += n as usize;
        while self.nacc >= 8 {
            self.nacc -= 8;
            self.buf.push((self.acc >> self.nacc) as u8);
        }
        self.acc &= (1u64 << self.nacc) - 1;
    }

    fn bits(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_msb_first() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0x1f, 5);
        w.put(0xabcd, 16);
        w.put(1, 1);
        assert_eq!(w.bits(), 25);
        assert_eq!(w.finish(), vec![0xbf, 0xab, 0xcd, 0x80]);
    }

    #[test]
    fn put_32_bits() {
        let mut w = BitWriter::new();
        w.put(1, 4);
        w.put(0xdeadbeef, 32);
        assert_eq!(w.finish(), vec![0x1d, 0xea, 0xdb, 0xee, 0xf0]);
    }
}
