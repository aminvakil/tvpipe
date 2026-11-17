//! Shared byte readers and writers. Every codec parses through [`Reader`]
//! so bounds checks and error messages look the same everywhere.

use crate::error::{Error, Result};

/// A bounds-checked cursor over a byte slice. Every read that would run past
/// the end returns [`ErrorKind::Malformed`](crate::error::ErrorKind) instead
/// of panicking.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    what: &'static str,
}

macro_rules! read_int {
    ($name:ident, $ty:ty, $from:ident) => {
        pub fn $name(&mut self) -> Result<$ty> {
            Ok(<$ty>::$from(self.array()?))
        }
    };
}

impl<'a> Reader<'a> {
    /// `what` names the format in error messages (e.g. `"dns"`).
    pub fn new(buf: &'a [u8], what: &'static str) -> Reader<'a> {
        Reader { buf, pos: 0, what }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// The whole underlying buffer (useful for formats with back-pointers,
    /// like DNS name compression).
    pub fn buffer(&self) -> &'a [u8] {
        self.buf
    }

    /// The bytes not yet consumed.
    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.buf.len() {
            return Err(self.eof(pos - self.pos.min(pos)));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.eof(n))?;
        if end > self.buf.len() {
            return Err(self.eof(n));
        }
        let rv = &self.buf[self.pos..end];
        self.pos = end;
        Ok(rv)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.bytes(N)?;
        let mut rv = [0u8; N];
        rv.copy_from_slice(b);
        Ok(rv)
    }

    pub fn peek_u8(&self) -> Result<u8> {
        self.buf.get(self.pos).copied().ok_or_else(|| self.eof(1))
    }

    /// A sub-reader over the next `n` bytes; advances past them.
    pub fn sub(&mut self, n: usize) -> Result<Reader<'a>> {
        Ok(Reader::new(self.bytes(n)?, self.what))
    }

    pub fn u8(&mut self) -> Result<u8> {
        let b = self.peek_u8()?;
        self.pos += 1;
        Ok(b)
    }

    read_int!(u16_be, u16, from_be_bytes);
    read_int!(u16_le, u16, from_le_bytes);
    read_int!(u32_be, u32, from_be_bytes);
    read_int!(u32_le, u32, from_le_bytes);
    read_int!(u64_be, u64, from_be_bytes);
    read_int!(u64_le, u64, from_le_bytes);
    read_int!(i16_be, i16, from_be_bytes);
    read_int!(i32_be, i32, from_be_bytes);
    read_int!(i64_be, i64, from_be_bytes);

    /// Big-endian unsigned integer of `n` bytes (`n <= 8`).
    pub fn uint_be(&mut self, n: usize) -> Result<u64> {
        if n > 8 {
            return Err(self.error("integer wider than 8 bytes"));
        }
        Ok(self
            .bytes(n)?
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | b as u64))
    }

    /// A malformed-data error tagged with the format name and position.
    pub fn error(&self, msg: &str) -> Error {
        Error::malformed(format!("{}: {} at offset {}", self.what, msg, self.pos))
    }

    fn eof(&self, wanted: usize) -> Error {
        Error::malformed(format!(
            "{}: unexpected end of data (wanted {} bytes at offset {}, have {})",
            self.what,
            wanted,
            self.pos,
            self.remaining()
        ))
    }
}

/// An append-only byte writer.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

macro_rules! write_int {
    ($name:ident, $ty:ty, $to:ident) => {
        pub fn $name(&mut self, v: $ty) -> &mut Self {
            self.buf.extend_from_slice(&v.$to());
            self
        }
    };
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn with_capacity(n: usize) -> Writer {
        Writer {
            buf: Vec::with_capacity(n),
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.buf.resize(self.buf.len() + n, 0);
        self
    }

    write_int!(u16_be, u16, to_be_bytes);
    write_int!(u16_le, u16, to_le_bytes);
    write_int!(u32_be, u32, to_be_bytes);
    write_int!(u32_le, u32, to_le_bytes);
    write_int!(u64_be, u64, to_be_bytes);
    write_int!(u64_le, u64, to_le_bytes);
    write_int!(i16_be, i16, to_be_bytes);
    write_int!(i32_be, i32, to_be_bytes);
    write_int!(i64_be, i64, to_be_bytes);

    /// Big-endian unsigned integer in `n` bytes (`n <= 8`); high bits that
    /// do not fit are dropped.
    pub fn uint_be(&mut self, v: u64, n: usize) -> &mut Self {
        assert!(n <= 8, "uint_be: n must be <= 8");
        self.buf.extend_from_slice(&v.to_be_bytes()[8 - n..]);
        self
    }

    /// Overwrites already written bytes at `pos` (for length back-patching).
    /// Panics if out of range: that is a local programming error, not input.
    pub fn patch(&mut self, pos: usize, v: &[u8]) {
        self.buf[pos..pos + v.len()].copy_from_slice(v);
    }
}

impl From<Writer> for Vec<u8> {
    fn from(w: Writer) -> Vec<u8> {
        w.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut w = Writer::new();
        w.u8(1)
            .u16_be(0x0203)
            .u32_le(0x07060504)
            .uint_be(0x08090a, 3)
            .bytes(b"xy");
        let v = w.into_vec();
        assert_eq!(v, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, b'x', b'y']);
        let mut r = Reader::new(&v, "test");
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16_be().unwrap(), 0x0203);
        assert_eq!(r.u32_le().unwrap(), 0x07060504);
        assert_eq!(r.uint_be(3).unwrap(), 0x08090a);
        assert_eq!(r.bytes(2).unwrap(), b"xy");
        assert!(r.is_at_end());
    }

    #[test]
    fn eof_is_error_not_panic() {
        let mut r = Reader::new(&[1, 2, 3], "test");
        let err = r.u32_be().unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Malformed);
        assert!(err.to_string().starts_with("test: unexpected end"));
        assert_eq!(r.position(), 0);
        assert!(r.bytes(usize::MAX).is_err());
        assert!(r.seek(4).is_err());
    }
}
