//! Reading zip archives (`playshare` bundles): the central directory,
//! stored and deflate entries, CRC-32 checks, bounds against zip bombs.
//!
//! What is refused: zip64, encryption, compression methods other than
//! stored (0) and deflate (8), multi-disk archives, entries whose local
//! header disagrees with the central directory. Names are returned as
//! stored (UTF-8 or, without the UTF-8 flag, CP437 read as Latin-1 for
//! the ASCII range only); checking them is the caller's job.
//!
//! Sources: PKWARE APPNOTE.TXT 6.3.10 (§4.3 the layout, §4.4 the fields),
//! ISO 3309 / ITU-T V.42 CRC-32 (the reflected 0xEDB88320 polynomial).

use crate::error::{Error, Result};
use crate::util::bytes::Reader;
use crate::util::inflate::inflate;

const EOCD_SIG: u32 = 0x0605_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
/// The end record and the longest comment.
const EOCD_SEARCH: usize = 22 + 0xffff;

/// Bounds for reading an archive.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Entries at most.
    pub entries: usize,
    /// Uncompressed bytes at most, all entries together.
    pub total: usize,
    /// Uncompressed bytes per compressed byte at most (deflate reaches
    /// about 1032; a larger claim is not deflate).
    pub ratio: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            entries: 10_000,
            total: 256 << 20,
            ratio: 1100,
        }
    }
}

/// One file of an archive (directories are left out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::malformed(format!("zip: {}", msg.into()))
}

/// CRC-32 (the zip, gzip and PNG one).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// Reads every file of a zip archive.
pub fn read(data: &[u8], limits: Limits) -> Result<Vec<Entry>> {
    let start = data.len().saturating_sub(EOCD_SEARCH);
    let eocd = (start..data.len().saturating_sub(21))
        .rev()
        .find(|&i| data[i..i + 4] == EOCD_SIG.to_le_bytes())
        .ok_or_else(|| bad("not a zip archive (no end of central directory)"))?;
    if eocd >= 20 && data[eocd - 20..eocd - 16] == ZIP64_LOCATOR_SIG.to_le_bytes() {
        return Err(Error::unsupported("zip: zip64 archives are not supported"));
    }
    let mut r = Reader::new(&data[eocd..], "zip");
    r.skip(4)?;
    let disk = r.u16_le()?;
    let cd_disk = r.u16_le()?;
    let on_disk = r.u16_le()?;
    let count = r.u16_le()?;
    let cd_size = r.u32_le()? as usize;
    let cd_offset = r.u32_le()? as usize;
    if disk != 0 || cd_disk != 0 || on_disk != count {
        return Err(Error::unsupported(
            "zip: multi-disk archives are not supported",
        ));
    }
    if count == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff {
        return Err(Error::unsupported("zip: zip64 archives are not supported"));
    }
    if usize::from(count) > limits.entries {
        return Err(bad(format!("more than {} entries", limits.entries)));
    }
    let cd = data
        .get(cd_offset..cd_offset.saturating_add(cd_size))
        .filter(|_| cd_offset.saturating_add(cd_size) <= eocd)
        .ok_or_else(|| bad("central directory out of bounds"))?;
    let mut r = Reader::new(cd, "zip");
    let mut out = Vec::new();
    let mut total = 0usize;
    for _ in 0..count {
        if r.u32_le()? != CENTRAL_SIG {
            return Err(bad("bad central directory entry"));
        }
        r.skip(4)?; // versions
        let flags = r.u16_le()?;
        let method = r.u16_le()?;
        r.skip(4)?; // time, date
        let crc = r.u32_le()?;
        let csize = r.u32_le()? as usize;
        let usize_ = r.u32_le()? as usize;
        let name_len = usize::from(r.u16_le()?);
        let extra_len = usize::from(r.u16_le()?);
        let comment_len = usize::from(r.u16_le()?);
        r.skip(8)?; // disk, internal and external attributes
        let local = r.u32_le()? as usize;
        let name_bytes = r.bytes(name_len)?;
        r.skip(extra_len + comment_len)?;
        let name = if flags & 0x0800 != 0 {
            std::str::from_utf8(name_bytes)
                .map_err(|_| bad("a name that is not UTF-8"))?
                .to_owned()
        } else if name_bytes.is_ascii() {
            String::from_utf8_lossy(name_bytes).into_owned()
        } else {
            return Err(bad("a name with non-ASCII characters and no UTF-8 flag"));
        };
        if flags & 0x0001 != 0 {
            return Err(Error::unsupported(format!(
                "zip: {name}: encrypted entries are not supported"
            )));
        }
        if csize == 0xffff_ffff || usize_ == 0xffff_ffff || local == 0xffff_ffff {
            return Err(Error::unsupported("zip: zip64 archives are not supported"));
        }
        if name.ends_with('/') {
            continue;
        }
        total = total.saturating_add(usize_);
        if total > limits.total {
            return Err(bad(format!(
                "more than {} MiB uncompressed",
                limits.total >> 20
            )));
        }
        // The local header: its own name and extra lengths.
        let mut l = Reader::new(data.get(local..).unwrap_or(&[]), "zip");
        if l.u32_le()? != LOCAL_SIG {
            return Err(bad(format!("{name}: bad local header")));
        }
        l.skip(2)?;
        let _ = l.u16_le()?;
        let lmethod = l.u16_le()?;
        l.skip(16)?; // time, date, crc, sizes (maybe in a data descriptor)
        let lname = usize::from(l.u16_le()?);
        let lextra = usize::from(l.u16_le()?);
        if lmethod != method || l.bytes(lname)? != name_bytes {
            return Err(bad(format!("{name}: local header disagrees")));
        }
        l.skip(lextra)?;
        let at = local + l.position();
        let raw = data
            .get(at..at.saturating_add(csize))
            .filter(|_| at.saturating_add(csize) <= cd_offset)
            .ok_or_else(|| bad(format!("{name}: data out of bounds")))?;
        let body = match method {
            0 => {
                if csize != usize_ {
                    return Err(bad(format!("{name}: stored sizes differ")));
                }
                raw.to_vec()
            }
            8 => {
                if usize_ > csize.saturating_mul(limits.ratio).max(1 << 16) {
                    return Err(bad(format!("{name}: compression ratio too high")));
                }
                let out = inflate(raw, usize_).map_err(|e| bad(format!("{name}: {e}")))?;
                if out.len() != usize_ {
                    return Err(bad(format!("{name}: size differs")));
                }
                out
            }
            m => {
                return Err(Error::unsupported(format!(
                    "zip: {name}: compression method {m} (only stored and deflate)"
                )))
            }
        };
        if crc32(&body) != crc {
            return Err(bad(format!("{name}: CRC mismatch")));
        }
        out.push(Entry { name, data: body });
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
