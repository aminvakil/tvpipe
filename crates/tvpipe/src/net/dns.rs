//! DNS message codec (RFC 1035 §4) with the Multicast DNS extensions we
//! need (RFC 6762 §18): the top bit of the class field is the
//! unicast-response ("QU") bit in questions and the cache-flush bit in
//! resource records.
//!
//! Parsing never panics: every read goes through [`Reader`], names are
//! length-checked (RFC 1035 §2.3.4), and compression pointers must point
//! strictly backwards past every previous jump, which makes pointer loops
//! impossible.
//!
//! Record types we understand are decoded into [`RData`]; everything else
//! is kept as raw bytes and written back unchanged.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::error::{Error, Result};
use crate::util::bytes::{Reader, Writer};

pub const TYPE_A: u16 = 1;
pub const TYPE_PTR: u16 = 12;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_SRV: u16 = 33;
pub const TYPE_NSEC: u16 = 47;
pub const TYPE_ANY: u16 = 255;

pub const CLASS_IN: u16 = 1;

/// Top bit of the class field: QU in questions, cache-flush in records
/// (RFC 6762 §5.4, §10.2).
const CLASS_FLAG: u16 = 0x8000;

/// Header flag bits (RFC 1035 §4.1.1).
pub const FLAG_QR: u16 = 0x8000;
pub const FLAG_AA: u16 = 0x0400;
pub const FLAG_TC: u16 = 0x0200;
pub const FLAG_RD: u16 = 0x0100;
pub const FLAG_RA: u16 = 0x0080;

const MAX_NAME_WIRE: usize = 255;
const MAX_LABEL: usize = 63;

/// A domain name as a list of raw labels (without the root label).
///
/// Labels are bytes, not text: DNS-SD instance names are UTF-8 and may
/// contain dots or spaces. Comparison and hashing ignore ASCII case, as DNS
/// does (RFC 1035 §2.3.3, RFC 6762 §16).
#[derive(Clone, Default)]
pub struct Name {
    labels: Vec<Vec<u8>>,
}

impl Name {
    pub fn root() -> Name {
        Name::default()
    }

    /// Builds a name from raw labels, validating lengths.
    pub fn from_labels<I, L>(labels: I) -> Result<Name>
    where
        I: IntoIterator<Item = L>,
        L: AsRef<[u8]>,
    {
        let mut name = Name::root();
        for label in labels {
            name.push(label.as_ref())?;
        }
        Ok(name)
    }

    /// Parses dotted presentation form (`_airplay._tcp.local`, trailing dot
    /// optional). `\.` and `\\` escape a literal dot or backslash, `\DDD` a
    /// decimal byte.
    pub fn parse(s: &str) -> Result<Name> {
        let mut name = Name::root();
        let mut label = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        if s == "." {
            return Ok(name);
        }
        while i < bytes.len() {
            match bytes[i] {
                b'.' => {
                    if label.is_empty() {
                        return Err(Error::malformed(format!("dns: empty label in {s:?}")));
                    }
                    name.push(&label)?;
                    label.clear();
                    i += 1;
                }
                b'\\' => {
                    let digits = bytes
                        .get(i + 1..i + 4)
                        .filter(|d| d.iter().all(u8::is_ascii_digit));
                    if let Some(d) = digits {
                        let v = d.iter().fold(0u32, |acc, &c| acc * 10 + (c - b'0') as u32);
                        let v = u8::try_from(v)
                            .map_err(|_| Error::malformed(format!("dns: bad escape in {s:?}")))?;
                        label.push(v);
                        i += 4;
                    } else if let Some(&c) = bytes.get(i + 1) {
                        label.push(c);
                        i += 2;
                    } else {
                        return Err(Error::malformed(format!(
                            "dns: trailing backslash in {s:?}"
                        )));
                    }
                }
                c => {
                    label.push(c);
                    i += 1;
                }
            }
        }
        if !label.is_empty() {
            name.push(&label)?;
        }
        Ok(name)
    }

    fn push(&mut self, label: &[u8]) -> Result<()> {
        if label.is_empty() || label.len() > MAX_LABEL {
            return Err(Error::malformed(format!(
                "dns: label length {} out of range",
                label.len()
            )));
        }
        if self.wire_len() + 1 + label.len() > MAX_NAME_WIRE {
            return Err(Error::malformed("dns: name longer than 255 bytes"));
        }
        self.labels.push(label.to_vec());
        Ok(())
    }

    /// `label` prepended to this name (e.g. an instance under a service).
    pub fn child(&self, label: &[u8]) -> Result<Name> {
        let mut name = Name::from_labels([label])?;
        for l in &self.labels {
            name.push(l)?;
        }
        Ok(name)
    }

    pub fn labels(&self) -> &[Vec<u8>] {
        &self.labels
    }

    pub fn first_label(&self) -> Option<&[u8]> {
        self.labels.first().map(Vec::as_slice)
    }

    /// The name without its first label.
    pub fn parent(&self) -> Name {
        Name {
            labels: self.labels.iter().skip(1).cloned().collect(),
        }
    }

    pub fn is_root(&self) -> bool {
        self.labels.is_empty()
    }

    /// Whether `suffix` is this name or one of its ancestors (ASCII case
    /// insensitive).
    pub fn ends_with(&self, suffix: &Name) -> bool {
        self.labels.len() >= suffix.labels.len()
            && self.labels[self.labels.len() - suffix.labels.len()..]
                .iter()
                .zip(&suffix.labels)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }

    /// Encoded length without compression, including the root byte.
    pub fn wire_len(&self) -> usize {
        self.labels.iter().map(|l| l.len() + 1).sum::<usize>() + 1
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Name) -> bool {
        self.labels.len() == other.labels.len()
            && self
                .labels
                .iter()
                .zip(&other.labels)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }
}

impl Eq for Name {}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.labels.len());
        for label in &self.labels {
            state.write_usize(label.len());
            for b in label {
                state.write_u8(b.to_ascii_lowercase());
            }
        }
    }
}

/// Writes one label in presentation form. Valid UTF-8 is printed as text;
/// dots, backslashes, control characters and invalid bytes are escaped.
pub(crate) fn fmt_label(f: &mut dyn fmt::Write, label: &[u8]) -> fmt::Result {
    for chunk in label.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '.' | '\\' => write!(f, "\\{c}")?,
                c if (c as u32) < 0x20 || c == '\x7f' => write!(f, "\\{:03}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        for b in chunk.invalid() {
            write!(f, "\\{b:03}")?;
        }
    }
    Ok(())
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.labels.is_empty() {
            return f.write_str(".");
        }
        for (i, label) in self.labels.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            fmt_label(f, label)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub name: Name,
    pub qtype: u16,
    /// Class without the QU bit.
    pub qclass: u16,
    /// mDNS "QU" bit: the querier asks for a unicast response.
    pub unicast_response: bool,
}

impl Question {
    pub fn new(name: Name, qtype: u16) -> Question {
        Question {
            name,
            qtype,
            qclass: CLASS_IN,
            unicast_response: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RData {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Ptr(Name),
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        target: Name,
    },
    /// TXT character-strings, each at most 255 bytes.
    Txt(Vec<Vec<u8>>),
    Nsec {
        next: Name,
        /// Types present in the type bitmap, ascending.
        types: Vec<u16>,
    },
    /// Any other type, kept verbatim. (Names inside it are not
    /// decompressed, so it is only meaningful for types without names.)
    Other(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: Name,
    pub rtype: u16,
    /// Class without the cache-flush bit.
    pub class: u16,
    /// mDNS cache-flush bit (RFC 6762 §10.2).
    pub cache_flush: bool,
    pub ttl: u32,
    pub data: RData,
}

impl Record {
    /// A record whose type is taken from `data` (`Other` needs `rtype`
    /// set by the caller afterwards).
    pub fn new(name: Name, ttl: u32, data: RData) -> Record {
        let rtype = match data {
            RData::A(_) => TYPE_A,
            RData::Aaaa(_) => TYPE_AAAA,
            RData::Ptr(_) => TYPE_PTR,
            RData::Srv { .. } => TYPE_SRV,
            RData::Txt(_) => TYPE_TXT,
            RData::Nsec { .. } => TYPE_NSEC,
            RData::Other(_) => 0,
        };
        Record {
            name,
            rtype,
            class: CLASS_IN,
            cache_flush: false,
            ttl,
            data,
        }
    }
}

/// A DNS message (RFC 1035 §4.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    pub id: u16,
    pub flags: u16,
    pub questions: Vec<Question>,
    pub answers: Vec<Record>,
    pub authorities: Vec<Record>,
    pub additionals: Vec<Record>,
}

impl Message {
    /// A standard query with the given questions.
    pub fn query(id: u16, questions: Vec<Question>) -> Message {
        Message {
            id,
            questions,
            ..Message::default()
        }
    }

    pub fn is_response(&self) -> bool {
        self.flags & FLAG_QR != 0
    }

    pub fn is_truncated(&self) -> bool {
        self.flags & FLAG_TC != 0
    }

    pub fn opcode(&self) -> u8 {
        ((self.flags >> 11) & 0xf) as u8
    }

    pub fn rcode(&self) -> u8 {
        (self.flags & 0xf) as u8
    }

    /// Answers, authorities and additionals in order.
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.answers
            .iter()
            .chain(&self.authorities)
            .chain(&self.additionals)
    }

    pub fn parse(buf: &[u8]) -> Result<Message> {
        let mut r = Reader::new(buf, "dns");
        let id = r.u16_be()?;
        let flags = r.u16_be()?;
        let qd = r.u16_be()?;
        let an = r.u16_be()?;
        let ns = r.u16_be()?;
        let ar = r.u16_be()?;
        let mut msg = Message {
            id,
            flags,
            ..Message::default()
        };
        for _ in 0..qd {
            let name = read_name(&mut r)?;
            let qtype = r.u16_be()?;
            let class = r.u16_be()?;
            msg.questions.push(Question {
                name,
                qtype,
                qclass: class & !CLASS_FLAG,
                unicast_response: class & CLASS_FLAG != 0,
            });
        }
        for (count, list) in [
            (an, &mut msg.answers),
            (ns, &mut msg.authorities),
            (ar, &mut msg.additionals),
        ] {
            for _ in 0..count {
                list.push(read_record(&mut r)?);
            }
        }
        // Trailing bytes are tolerated: some responders pad packets.
        Ok(msg)
    }

    /// Encodes the message, compressing names if `compress` is set.
    pub fn to_bytes(&self, compress: bool) -> Result<Vec<u8>> {
        let mut e = Encoder {
            w: Writer::with_capacity(512),
            compress,
            offsets: HashMap::new(),
        };
        let count = |n: usize| {
            u16::try_from(n).map_err(|_| Error::malformed("dns: too many entries for one message"))
        };
        e.w.u16_be(self.id).u16_be(self.flags);
        e.w.u16_be(count(self.questions.len())?);
        e.w.u16_be(count(self.answers.len())?);
        e.w.u16_be(count(self.authorities.len())?);
        e.w.u16_be(count(self.additionals.len())?);
        for q in &self.questions {
            e.name(&q.name);
            e.w.u16_be(q.qtype);
            e.w.u16_be(q.qclass | if q.unicast_response { CLASS_FLAG } else { 0 });
        }
        for rec in self.records() {
            e.record(rec)?;
        }
        Ok(e.w.into_vec())
    }
}

/// Reads a possibly compressed name at the reader's position and advances
/// past it (RFC 1035 §4.1.4).
fn read_name(r: &mut Reader<'_>) -> Result<Name> {
    let buf = r.buffer();
    let mut pos = r.position();
    // Every pointer must point strictly before this; it then becomes the
    // new bound, so the walk always terminates.
    let mut bound = pos;
    let mut resume = None;
    let mut name = Name::root();
    loop {
        let len = *buf
            .get(pos)
            .ok_or_else(|| Error::malformed(format!("dns: name runs past end at offset {pos}")))?;
        match len & 0xc0 {
            0x00 if len == 0 => {
                pos += 1;
                break;
            }
            0x00 => {
                let start = pos + 1;
                let label = buf.get(start..start + len as usize).ok_or_else(|| {
                    Error::malformed(format!("dns: label runs past end at offset {pos}"))
                })?;
                name.push(label)?;
                pos = start + len as usize;
            }
            0xc0 => {
                let lo = *buf.get(pos + 1).ok_or_else(|| {
                    Error::malformed(format!("dns: truncated pointer at offset {pos}"))
                })?;
                let target = (((len & 0x3f) as usize) << 8) | lo as usize;
                if target >= bound {
                    return Err(Error::malformed(format!(
                        "dns: compression pointer at offset {pos} does not point backwards"
                    )));
                }
                resume.get_or_insert(pos + 2);
                bound = target;
                pos = target;
            }
            _ => {
                return Err(Error::malformed(format!(
                    "dns: unsupported label type {len:#04x} at offset {pos}"
                )))
            }
        }
    }
    r.seek(resume.unwrap_or(pos))?;
    Ok(name)
}

fn read_record(r: &mut Reader<'_>) -> Result<Record> {
    let name = read_name(r)?;
    let rtype = r.u16_be()?;
    let class = r.u16_be()?;
    let ttl = r.u32_be()?;
    let rdlen = r.u16_be()? as usize;
    let start = r.position();
    let end = start + rdlen;
    if end > r.len() {
        return Err(r.error("record data runs past end"));
    }
    let data = match rtype {
        TYPE_A => RData::A(Ipv4Addr::from(fixed_rdata::<4>(r, rdlen)?)),
        TYPE_AAAA => RData::Aaaa(Ipv6Addr::from(fixed_rdata::<16>(r, rdlen)?)),
        TYPE_PTR => RData::Ptr(read_name(r)?),
        TYPE_SRV => RData::Srv {
            priority: r.u16_be()?,
            weight: r.u16_be()?,
            port: r.u16_be()?,
            target: read_name(r)?,
        },
        TYPE_TXT => {
            let mut sub = r.sub(rdlen)?;
            let mut strings = Vec::new();
            while !sub.is_at_end() {
                let n = sub.u8()? as usize;
                strings.push(sub.bytes(n)?.to_vec());
            }
            RData::Txt(strings)
        }
        TYPE_NSEC => {
            let next = read_name(r)?;
            if r.position() > end {
                return Err(r.error("NSEC name overruns record"));
            }
            let mut sub = r.sub(end - r.position())?;
            let mut types = Vec::new();
            let mut last_window = None;
            while !sub.is_at_end() {
                let window = sub.u8()?;
                let len = sub.u8()? as usize;
                if len == 0 || len > 32 || last_window.is_some_and(|w| w >= window) {
                    return Err(sub.error("bad NSEC type bitmap"));
                }
                last_window = Some(window);
                for (i, byte) in sub.bytes(len)?.iter().enumerate() {
                    for bit in 0..8 {
                        if byte & (0x80 >> bit) != 0 {
                            types.push((window as u16) << 8 | (i * 8 + bit) as u16);
                        }
                    }
                }
            }
            RData::Nsec { next, types }
        }
        _ => RData::Other(r.bytes(rdlen)?.to_vec()),
    };
    if r.position() != end {
        return Err(Error::malformed(format!(
            "dns: record data length mismatch for type {rtype} at offset {start}"
        )));
    }
    Ok(Record {
        name,
        rtype,
        class: class & !CLASS_FLAG,
        cache_flush: class & CLASS_FLAG != 0,
        ttl,
        data,
    })
}

fn fixed_rdata<const N: usize>(r: &mut Reader<'_>, rdlen: usize) -> Result<[u8; N]> {
    if rdlen != N {
        return Err(r.error("address record has wrong length"));
    }
    r.array()
}

struct Encoder {
    w: Writer,
    compress: bool,
    /// Offsets of already written name suffixes.
    offsets: HashMap<Name, u16>,
}

impl Encoder {
    fn name(&mut self, name: &Name) {
        for i in 0..name.labels.len() {
            let suffix = Name {
                labels: name.labels[i..].to_vec(),
            };
            if self.compress {
                if let Some(&off) = self.offsets.get(&suffix) {
                    self.w.u16_be(0xc000 | off);
                    return;
                }
                if let Ok(off) = u16::try_from(self.w.len()) {
                    if off < 0x4000 {
                        self.offsets.insert(suffix, off);
                    }
                }
            }
            let label = &name.labels[i];
            self.w.u8(label.len() as u8).bytes(label);
        }
        self.w.u8(0);
    }

    fn record(&mut self, rec: &Record) -> Result<()> {
        self.name(&rec.name);
        self.w.u16_be(rec.rtype);
        self.w
            .u16_be(rec.class | if rec.cache_flush { CLASS_FLAG } else { 0 });
        self.w.u32_be(rec.ttl);
        let len_pos = self.w.len();
        self.w.u16_be(0);
        match &rec.data {
            RData::A(a) => {
                self.w.bytes(&a.octets());
            }
            RData::Aaaa(a) => {
                self.w.bytes(&a.octets());
            }
            RData::Ptr(n) => self.name(n),
            RData::Srv {
                priority,
                weight,
                port,
                target,
            } => {
                self.w.u16_be(*priority).u16_be(*weight).u16_be(*port);
                self.name(target);
            }
            RData::Txt(strings) => {
                for s in strings {
                    let n = u8::try_from(s.len())
                        .map_err(|_| Error::malformed("dns: TXT string longer than 255 bytes"))?;
                    self.w.u8(n).bytes(s);
                }
                if strings.is_empty() {
                    // RFC 6763 §6.1: an empty TXT record is a single zero byte.
                    self.w.u8(0);
                }
            }
            RData::Nsec { next, types } => {
                self.name(next);
                let mut types = types.clone();
                types.sort_unstable();
                types.dedup();
                let mut i = 0;
                while i < types.len() {
                    let window = types[i] >> 8;
                    let mut bitmap = [0u8; 32];
                    let mut used = 0;
                    while i < types.len() && types[i] >> 8 == window {
                        let low = (types[i] & 0xff) as usize;
                        bitmap[low / 8] |= 0x80 >> (low % 8);
                        used = low / 8 + 1;
                        i += 1;
                    }
                    self.w
                        .u8(window as u8)
                        .u8(used as u8)
                        .bytes(&bitmap[..used]);
                }
            }
            RData::Other(b) => {
                self.w.bytes(b);
            }
        }
        let rdlen = u16::try_from(self.w.len() - len_pos - 2)
            .map_err(|_| Error::malformed("dns: record data longer than 65535 bytes"))?;
        self.w.patch(len_pos, &rdlen.to_be_bytes());
        Ok(())
    }
}

/// Decodes TXT character-strings into key/value pairs (RFC 6763 §6.3–6.4).
///
/// Keys are lowercased (they are case-insensitive), only the first
/// occurrence of a key counts, strings without `=` are boolean attributes
/// with value `None`, and empty strings or strings starting with `=` are
/// ignored.
pub fn txt_pairs(strings: &[Vec<u8>]) -> Vec<(String, Option<Vec<u8>>)> {
    let mut out: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for s in strings {
        let (key, value) = match s.iter().position(|&b| b == b'=') {
            Some(i) => (&s[..i], Some(s[i + 1..].to_vec())),
            None => (&s[..], None),
        };
        if key.is_empty() {
            continue;
        }
        let key = String::from_utf8_lossy(key).to_ascii_lowercase();
        if out.iter().any(|(k, _)| *k == key) {
            continue;
        }
        out.push((key, value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;
    use crate::util::vectors;

    fn n(s: &str) -> Name {
        Name::parse(s).unwrap()
    }

    #[test]
    fn name_parse_and_display() {
        let name = n("My\\.TV._airplay._tcp.local.");
        assert_eq!(name.labels().len(), 4);
        assert_eq!(name.first_label().unwrap(), b"My.TV");
        assert_eq!(name.to_string(), "My\\.TV._airplay._tcp.local");
        assert_eq!(n("a\\032b").first_label().unwrap(), b"a b");
        assert_eq!(n(".").to_string(), ".");
        assert!(Name::parse("a..b").is_err());
        assert!(Name::parse(&"x".repeat(64)).is_err());
        let long = vec!["a".repeat(63); 4].join(".");
        assert!(Name::parse(&long).is_err()); // 4*64+1 = 257 > 255
        assert!(Name::parse(&long[..long.len() - 2]).is_ok());
    }

    #[test]
    fn name_case_insensitive() {
        use std::collections::HashSet;
        assert_eq!(n("Foo.LOCAL"), n("foo.local"));
        let set: HashSet<Name> = [n("Foo.local")].into_iter().collect();
        assert!(set.contains(&n("fOO.Local")));
        assert!(n("TV._airplay._tcp.local").ends_with(&n("_AirPlay._tcp.local")));
        assert!(!n("local").ends_with(&n("_tcp.local")));
        assert_eq!(n("a.b.c").child(b"x").unwrap().parent().parent(), n("b.c"));
    }

    /// RFC 1035 §4.1.4 example: F.ISI.ARPA, FOO.F.ISI.ARPA, ARPA, root.
    #[test]
    fn rfc1035_compression_example() {
        let mut buf = vec![0u8; 20];
        buf.extend_from_slice(b"\x01F\x03ISI\x04ARPA\x00");
        buf.resize(40, 0);
        buf.extend_from_slice(b"\x03FOO\xc0\x14");
        buf.resize(64, 0);
        buf.extend_from_slice(b"\xc0\x1a");
        buf.resize(92, 0);
        buf.push(0);
        for (pos, want, end) in [
            (20, "F.ISI.ARPA", 32),
            (40, "FOO.F.ISI.ARPA", 46),
            (64, "ARPA", 66),
            (92, ".", 93),
        ] {
            let mut r = Reader::new(&buf, "dns");
            r.seek(pos).unwrap();
            assert_eq!(read_name(&mut r).unwrap().to_string(), want);
            assert_eq!(r.position(), end);
        }
    }

    #[test]
    fn pointer_loops_are_rejected() {
        // Self pointer, forward pointer, and a two-step loop.
        for buf in [
            &b"\xc0\x00"[..],
            &b"\xc0\x02\x00"[..],
            &b"\x01a\xc0\x00"[..],
            &b"\x01a\xc0\x04\x01b\xc0\x00"[..],
        ] {
            let mut r = Reader::new(buf, "dns");
            assert!(read_name(&mut r).is_err(), "{buf:?}");
        }
        // Reserved label types and truncation.
        for buf in [
            &b"\x40"[..],
            &b"\x80"[..],
            &b"\x05ab"[..],
            &b"\x01a"[..],
            &b"\xc0"[..],
        ] {
            let mut r = Reader::new(buf, "dns");
            assert!(read_name(&mut r).is_err(), "{buf:?}");
        }
    }

    #[test]
    fn overlong_name_via_pointers_is_rejected() {
        // Chain of 63-byte labels, each pointing back to the previous one,
        // so the expanded name exceeds 255 bytes.
        let mut buf = Vec::new();
        let mut prev: Option<usize> = None;
        for _ in 0..5 {
            let at = buf.len();
            buf.push(63);
            buf.extend_from_slice(&[b'x'; 63]);
            match prev {
                Some(p) => buf.extend_from_slice(&(0xc000 | p as u16).to_be_bytes()),
                None => buf.push(0),
            }
            prev = Some(at);
        }
        let mut r = Reader::new(&buf, "dns");
        r.seek(prev.unwrap()).unwrap();
        let err = read_name(&mut r).unwrap_err();
        assert!(err.to_string().contains("255"), "{err}");
    }

    fn sample_message() -> Message {
        let svc = n("_airplay._tcp.local");
        let inst = svc.child("Living Room".as_bytes()).unwrap();
        let host = n("Living-Room.local");
        let mut srv = Record::new(
            inst.clone(),
            120,
            RData::Srv {
                priority: 0,
                weight: 0,
                port: 7000,
                target: host.clone(),
            },
        );
        srv.cache_flush = true;
        Message {
            id: 0x1234,
            flags: FLAG_QR | FLAG_AA,
            questions: vec![Question::new(svc.clone(), TYPE_PTR)],
            answers: vec![Record::new(svc, 4500, RData::Ptr(inst.clone()))],
            authorities: vec![],
            additionals: vec![
                srv,
                Record::new(
                    inst.clone(),
                    4500,
                    RData::Txt(vec![
                        b"deviceid=AA:BB:CC:00:00:01".to_vec(),
                        b"pw=false".to_vec(),
                    ]),
                ),
                Record::new(host.clone(), 120, RData::A(Ipv4Addr::new(192, 168, 1, 34))),
                Record::new(host.clone(), 120, RData::Aaaa("fe80::1".parse().unwrap())),
                Record::new(
                    host,
                    120,
                    RData::Nsec {
                        next: n("Living-Room.local"),
                        types: vec![TYPE_A, TYPE_AAAA, TYPE_NSEC, 0x0101],
                    },
                ),
                {
                    let mut r = Record::new(inst, 10, RData::Other(vec![1, 2, 3]));
                    r.rtype = 99;
                    r
                },
            ],
        }
    }

    #[test]
    fn roundtrip_with_and_without_compression() {
        let msg = sample_message();
        let plain = msg.to_bytes(false).unwrap();
        let packed = msg.to_bytes(true).unwrap();
        assert!(packed.len() < plain.len());
        assert_eq!(Message::parse(&plain).unwrap(), msg);
        assert_eq!(Message::parse(&packed).unwrap(), msg);
        assert!(Message::parse(&packed).unwrap().is_response());
    }

    #[test]
    fn query_known_answer() {
        // A legacy-unicast PTR query as we send it.
        let mut q = Question::new(n("_googlecast._tcp.local"), TYPE_PTR);
        let bytes = Message::query(0xbeef, vec![q.clone()])
            .to_bytes(true)
            .unwrap();
        assert_eq!(
            hex::encode(&bytes),
            "beef00000001000000000000\
             0b5f676f6f676c6563617374045f746370056c6f63616c00000c0001"
        );
        q.unicast_response = true;
        let bytes = Message::query(0, vec![q.clone()]).to_bytes(true).unwrap();
        assert!(bytes.ends_with(&[0x00, 0x0c, 0x80, 0x01]));
        let parsed = Message::parse(&bytes).unwrap();
        assert_eq!(parsed.questions, vec![q]);
    }

    #[test]
    fn truncation_never_panics() {
        let msg = sample_message().to_bytes(true).unwrap();
        for len in 0..msg.len() {
            let _ = Message::parse(&msg[..len]);
        }
        // Flip every byte to a few values; must never panic.
        for i in 0..msg.len() {
            for v in [0x00, 0x3f, 0xc0, 0xff] {
                let mut m = msg.clone();
                m[i] = v;
                let _ = Message::parse(&m);
            }
        }
    }

    #[test]
    fn bad_rdata_lengths() {
        let mut msg = Message {
            id: 0,
            flags: FLAG_QR,
            answers: vec![Record::new(n("x.local"), 1, RData::A(Ipv4Addr::LOCALHOST))],
            ..Message::default()
        };
        let mut bytes = msg.to_bytes(false).unwrap();
        // rdlength 4 -> 3
        let at = bytes.len() - 6;
        bytes[at + 1] = 3;
        assert!(Message::parse(&bytes[..bytes.len() - 1]).is_err());
        // PTR whose name is shorter than rdlength.
        msg.answers = vec![Record::new(n("x.local"), 1, RData::Ptr(n("y.local")))];
        let mut bytes = msg.to_bytes(false).unwrap();
        bytes.push(0);
        let at = bytes.len() - 1 - 9 - 2;
        bytes[at + 1] += 1;
        assert!(Message::parse(&bytes).is_err());
    }

    #[test]
    fn txt_pairs_rules() {
        let strings: Vec<Vec<u8>> = [
            &b"Model=AppleTV"[..],
            b"model=ignored",
            b"pw",
            b"",
            b"=nokey",
            b"empty=",
            b"eq=a=b",
        ]
        .iter()
        .map(|s| s.to_vec())
        .collect();
        let pairs = txt_pairs(&strings);
        assert_eq!(
            pairs,
            vec![
                ("model".to_owned(), Some(b"AppleTV".to_vec())),
                ("pw".to_owned(), None),
                ("empty".to_owned(), Some(vec![])),
                ("eq".to_owned(), Some(b"a=b".to_vec())),
            ]
        );
    }

    #[test]
    fn empty_txt_encodes_as_zero_byte() {
        let msg = Message {
            answers: vec![Record::new(n("x.local"), 1, RData::Txt(vec![]))],
            ..Message::default()
        };
        let bytes = msg.to_bytes(false).unwrap();
        assert!(bytes.ends_with(&[0, 1, 0]));
        let parsed = Message::parse(&bytes).unwrap();
        assert_eq!(parsed.answers[0].data, RData::Txt(vec![vec![]]));
    }

    /// Known-answer vectors: hex messages with their expected decoding.
    #[test]
    fn vectors() {
        for b in vectors::load("dns/messages.txt") {
            let msg = Message::parse(&b.hex("msg")).unwrap_or_else(|e| panic!("{}: {e}", b.name()));
            let mut lines = Vec::new();
            for q in &msg.questions {
                lines.push(format!(
                    "q {} {}{}",
                    q.name,
                    q.qtype,
                    if q.unicast_response { " qu" } else { "" }
                ));
            }
            for rec in msg.records() {
                lines.push(format!("r {}", describe(rec)));
            }
            let want: Vec<&str> = b.str("expect").split(" | ").collect();
            assert_eq!(lines, want, "{}", b.name());
            if let Some(re) = b.get("reencoded") {
                let want = if re == "same" {
                    b.hex("msg")
                } else {
                    b.hex("reencoded")
                };
                assert_eq!(msg.to_bytes(true).unwrap(), want, "{}", b.name());
            }
        }
    }

    pub(crate) fn describe(rec: &Record) -> String {
        let data = match &rec.data {
            RData::A(a) => format!("A {a}"),
            RData::Aaaa(a) => format!("AAAA {a}"),
            RData::Ptr(p) => format!("PTR {p}"),
            RData::Srv {
                priority,
                weight,
                port,
                target,
            } => format!("SRV {priority} {weight} {port} {target}"),
            RData::Txt(s) => format!(
                "TXT {}",
                s.iter()
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            RData::Nsec { next, types } => format!("NSEC {next} {types:?}"),
            RData::Other(b) => format!("TYPE{} {}", rec.rtype, hex::encode(b)),
        };
        format!(
            "{} {}{} {}",
            rec.name,
            rec.ttl,
            if rec.cache_flush { " flush" } else { "" },
            data
        )
    }
}
