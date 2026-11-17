//! SLRW: the framing a game writes to `streamlib play ... -` (PLAN §5.2).
//!
//! ```text
//! "SLRW"                                   magic, once
//! repeated:
//!   u8   kind     1 = video, 2 = audio, 0 = end of stream
//!   u64  pts_us   presentation time in microseconds (little-endian)
//!   u32  len      payload length in bytes (little-endian)
//!   [len] payload one raw video frame, or interleaved PCM samples
//! ```
//!
//! An `end` record has `len = 0`. Multi-byte fields are little-endian
//! (docs/decisions/0002-slrw.md).
//!
//! This file has no dependencies on the rest of the crate so the example
//! generator (`examples/slrw-gen.rs`) can include it with `#[path]`.

#![allow(dead_code)]

use std::io::{self, Read, Write};

pub const MAGIC: &[u8; 4] = b"SLRW";
/// Largest payload accepted (an 8K RGBA frame is 133 MB).
pub const MAX_PAYLOAD: u32 = 256 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    End,
    Video,
    Audio,
}

impl Kind {
    pub fn code(self) -> u8 {
        match self {
            Kind::End => 0,
            Kind::Video => 1,
            Kind::Audio => 2,
        }
    }

    pub fn from_code(c: u8) -> Option<Kind> {
        Some(match c {
            0 => Kind::End,
            1 => Kind::Video,
            2 => Kind::Audio,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub kind: Kind,
    pub pts_us: u64,
    pub payload: Vec<u8>,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("slrw: {msg}"))
}

/// Writes the magic.
pub fn write_magic(w: &mut impl Write) -> io::Result<()> {
    w.write_all(MAGIC)
}

/// Writes one record.
pub fn write_packet(w: &mut impl Write, kind: Kind, pts_us: u64, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|&l| l <= MAX_PAYLOAD)
        .ok_or_else(|| invalid("payload too large".into()))?;
    let mut head = [0u8; 13];
    head[0] = kind.code();
    head[1..9].copy_from_slice(&pts_us.to_le_bytes());
    head[9..13].copy_from_slice(&len.to_le_bytes());
    w.write_all(&head)?;
    w.write_all(payload)
}

/// Reads SLRW records from a stream.
pub struct SlrwReader<R> {
    inner: R,
    started: bool,
    done: bool,
    /// Per-kind payload limits (video, audio).
    max_video: u32,
    max_audio: u32,
}

impl<R: Read> SlrwReader<R> {
    pub fn new(inner: R) -> SlrwReader<R> {
        SlrwReader {
            inner,
            started: false,
            done: false,
            max_video: MAX_PAYLOAD,
            max_audio: MAX_PAYLOAD,
        }
    }

    pub fn with_limits(mut self, max_video: u32, max_audio: u32) -> SlrwReader<R> {
        self.max_video = max_video.min(MAX_PAYLOAD);
        self.max_audio = max_audio.min(MAX_PAYLOAD);
        self
    }

    /// Fills `buf`; `Ok(false)` if the stream ended before the first byte.
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<bool> {
        let mut got = 0;
        while got < buf.len() {
            match self.inner.read(&mut buf[got..]) {
                Ok(0) if got == 0 => return Ok(false),
                Ok(0) => return Err(invalid("truncated record".into())),
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }

    /// The next record. `None` after an `end` record or at EOF on a record
    /// boundary; the `end` record itself is returned once.
    pub fn next_packet(&mut self) -> io::Result<Option<Packet>> {
        if self.done {
            return Ok(None);
        }
        if !self.started {
            let mut magic = [0u8; 4];
            if !self.fill(&mut magic)? {
                self.done = true;
                return Ok(None);
            }
            if &magic != MAGIC {
                return Err(invalid("missing SLRW magic".into()));
            }
            self.started = true;
        }
        let mut head = [0u8; 13];
        if !self.fill(&mut head)? {
            self.done = true;
            return Ok(None);
        }
        let kind = Kind::from_code(head[0])
            .ok_or_else(|| invalid(format!("unknown record kind {}", head[0])))?;
        let mut pts = [0u8; 8];
        pts.copy_from_slice(&head[1..9]);
        let mut len = [0u8; 4];
        len.copy_from_slice(&head[9..13]);
        let (pts_us, len) = (u64::from_le_bytes(pts), u32::from_le_bytes(len));
        let max = match kind {
            Kind::End => 0,
            Kind::Video => self.max_video,
            Kind::Audio => self.max_audio,
        };
        if len > max {
            return Err(invalid(format!(
                "{kind:?} record of {len} bytes exceeds {max}"
            )));
        }
        let mut payload = vec![0u8; len as usize];
        if len > 0 && !self.fill(&mut payload)? {
            return Err(invalid("truncated record".into()));
        }
        if kind == Kind::End {
            self.done = true;
        }
        Ok(Some(Packet {
            kind,
            pts_us,
            payload,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_bytes() {
        let mut out = Vec::new();
        write_magic(&mut out).unwrap();
        write_packet(&mut out, Kind::Video, 0x0102, b"ab").unwrap();
        write_packet(&mut out, Kind::End, 7, b"").unwrap();
        assert_eq!(
            out,
            [
                b'S', b'L', b'R', b'W', // magic
                1, 2, 1, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, b'a', b'b', // video
                0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // end
            ]
        );
        let mut r = SlrwReader::new(&out[..]);
        let p = r.next_packet().unwrap().unwrap();
        assert_eq!(
            (p.kind, p.pts_us, p.payload.as_slice()),
            (Kind::Video, 0x102, &b"ab"[..])
        );
        assert_eq!(r.next_packet().unwrap().unwrap().kind, Kind::End);
        assert_eq!(r.next_packet().unwrap(), None);
    }

    #[test]
    fn errors() {
        assert!(SlrwReader::new(&b"XXXX"[..]).next_packet().is_err());
        assert_eq!(SlrwReader::new(&b""[..]).next_packet().unwrap(), None);
        // EOF on a record boundary is a clean end.
        assert_eq!(SlrwReader::new(&b"SLRW"[..]).next_packet().unwrap(), None);
        // Truncated header / payload.
        assert!(SlrwReader::new(&b"SLRW\x01\0\0"[..]).next_packet().is_err());
        let mut short = b"SLRW\x02".to_vec();
        short.extend([0; 8]);
        short.extend(9u32.to_le_bytes());
        short.extend(b"abc");
        assert!(SlrwReader::new(&short[..]).next_packet().is_err());
        // Unknown kind; oversized payload.
        let mut bad = b"SLRW\x07".to_vec();
        bad.extend([0; 12]);
        assert!(SlrwReader::new(&bad[..]).next_packet().is_err());
        let mut big = b"SLRW\x01".to_vec();
        big.extend([0; 8]);
        big.extend(100u32.to_le_bytes());
        assert!(SlrwReader::new(&big[..])
            .with_limits(99, 99)
            .next_packet()
            .is_err());
    }
}
