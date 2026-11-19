//! Writing zip archives (`tvpipe game pack`): deterministic ones. Entries
//! are sorted by name, every timestamp is 1980-01-01 00:00, every file
//! is `rw-r--r--`, each entry is deflated ([`super::deflate`]) or stored
//! when that is not smaller; no extra fields, comments or data
//! descriptors. The same files give the same bytes.
//!
//! Sources: PKWARE APPNOTE.TXT 6.3.10 (§4.3 the layout, §4.4 the fields).

use crate::error::{Error, Result};
use crate::util::deflate::deflate;
use crate::util::zip::crc32;

/// The DOS date of 1980-01-01 (the format's first day; the time is 0).
const DOS_DATE: u16 = (1 << 5) | 1;
/// Made by: UNIX (3), spec 2.0, so the mode in the external attributes
/// counts.
const MADE_BY: u16 = (3 << 8) | 20;
const NEEDED: u16 = 20;
/// UTF-8 names (bit 11).
const UTF8: u16 = 0x0800;
/// A regular file, `rw-r--r--`.
const MODE: u32 = 0o100_644;

/// An archive of `entries` (name, bytes): names are `/`-separated paths,
/// unique. Too many entries or more than 4 GiB need zip64: an error.
pub fn write(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&(String, Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    if let Some(w) = sorted.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(Error::usage(format!("zip: {} twice", w[0].0)));
    }
    if sorted.len() >= 0xffff {
        return Err(Error::unsupported(
            "zip: 65535 entries or more need zip64, which tvpipe does not write",
        ));
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in sorted {
        let deflated = deflate(data);
        let (method, body) = if deflated.len() < data.len() {
            (8u16, deflated.as_slice())
        } else {
            (0u16, data.as_slice())
        };
        let offset = u32::try_from(out.len())
            .ok()
            .filter(|_| data.len() < u32::MAX as usize)
            .ok_or_else(|| Error::unsupported("zip: more than 4 GiB needs zip64"))?;
        let crc = crc32(data);
        let n = name.as_bytes();
        let name_len = u16::try_from(n.len())
            .map_err(|_| Error::usage(format!("zip: a name too long: {name}")))?;
        let flags = if name.is_ascii() { 0 } else { UTF8 };
        // The local header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&NEEDED.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(n);
        out.extend_from_slice(body);
        // Its central directory entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&MADE_BY.to_le_bytes());
        central.extend_from_slice(&NEEDED.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(body.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&[0; 8]); // extra, comment, disk, internal attributes
        central.extend_from_slice(&(MODE << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(n);
    }
    let cd_offset = u32::try_from(out.len())
        .ok()
        .filter(|o| (*o as usize).saturating_add(central.len()) < u32::MAX as usize)
        .ok_or_else(|| Error::unsupported("zip: more than 4 GiB needs zip64"))?;
    let count = entries.len() as u16;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::zip;

    #[test]
    fn round_trip_sorted_and_deterministic() {
        let files = vec![
            ("b/two.txt".to_owned(), b"two two two two two two".to_vec()),
            ("a.bin".to_owned(), vec![1, 2, 3]),
            ("ü/ñ.txt".to_owned(), Vec::new()),
            ("c.html".to_owned(), "<p>hi</p>".repeat(500).into_bytes()),
        ];
        let z = write(&files).unwrap();
        let mut rev = files.clone();
        rev.reverse();
        assert_eq!(write(&rev).unwrap(), z, "the order of the input counts not");
        let back = zip::read(&z, zip::Limits::default()).unwrap();
        let names: Vec<&str> = back.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.bin", "b/two.txt", "c.html", "ü/ñ.txt"]);
        for e in &back {
            let orig = files.iter().find(|f| f.0 == e.name).unwrap();
            assert_eq!(e.data, orig.1);
        }
        // The big one is deflated, the tiny one stored.
        assert!(z.len() < 1000, "{}", z.len());
    }

    #[test]
    fn empty_and_duplicates() {
        let z = write(&[]).unwrap();
        assert_eq!(z.len(), 22);
        assert!(zip::read(&z, zip::Limits::default()).unwrap().is_empty());
        let dup = vec![("x".to_owned(), vec![]), ("x".to_owned(), vec![1])];
        assert!(write(&dup).is_err());
    }

    /// The bytes are pinned: a change to the writer or the compressor that
    /// changes them changes every bundle's hash (on purpose, then).
    #[test]
    fn known_answer() {
        let z = write(&[("tvpipe.json".to_owned(), b"{\"name\":\"x\"}\n".to_vec())]).unwrap();
        assert_eq!(
            crate::util::hex::encode(&crate::crypto::ring::sha256(&z)),
            "7b581e22e2c08a1c46feea139137c1a69af6c61169450c9c3d96659b9bdc5722"
        );
    }
}
