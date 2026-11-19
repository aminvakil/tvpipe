//! Binary property lists (`bplist00`): a [`Plist`] value tree, a
//! bounds-checked reader and a writer.
//!
//! Layout of a file:
//!
//! ```text
//! "bplist00" | objects ... | offset table | trailer (32 bytes)
//! trailer = 5 unused | sort version u8 | offset size u8 | ref size u8
//!         | object count u64be | top object u64be | offset table offset u64be
//! ```
//!
//! The offset table holds, for every object number, the byte offset of the
//! object (`offset size` bytes, big endian). Containers refer to objects by
//! number (`ref size` bytes). Each object starts with a marker byte: high
//! nibble = type, low nibble = size or count (`0xF`: the count follows as an
//! integer object).
//!
//! | marker  | type                                         |
//! |---------|----------------------------------------------|
//! | `00`    | null                                         |
//! | `08/09` | false / true                                 |
//! | `1n`    | integer, 2^n bytes (1/2/4 unsigned, 8/16 signed) |
//! | `2n`    | real, 2^n bytes (4 or 8)                     |
//! | `33`    | date, f64 seconds since 2001-01-01 00:00 UTC |
//! | `4n`    | data, n bytes                                |
//! | `5n`    | ASCII string, n bytes                        |
//! | `6n`    | UTF-16BE string, n code units                |
//! | `An`    | array, n refs                                |
//! | `Dn`    | dict, n key refs then n value refs           |
//!
//! UIDs (`8n`) and sets (`Cn`) are not supported. The reader rejects
//! reference cycles, nesting deeper than [`MAX_DEPTH`], and inputs whose
//! shared references would expand beyond [`MAX_NODES`] values or
//! [`MAX_BYTES`] of strings and data.
//!
//! The writer emits objects depth first (a dict's keys, then its values),
//! shares equal scalars (strings, numbers, data, ...) but never containers,
//! and uses the smallest offset and reference sizes. This matches Python's
//! `plistlib.dumps(fmt=FMT_BINARY, sort_keys=False)` byte for byte, except
//! that reals are shared by bit pattern (so `0.0` and `-0.0` stay distinct).
//!
//! Sources: Apple's public description of the bplist00 format (the comment
//! block in CF's CFBinaryPList.c, used for facts only); Python `plistlib`
//! (PSF licence) as the fixture oracle.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::util::bytes::{Reader, Writer};

/// Maximum nesting of arrays and dicts.
pub const MAX_DEPTH: usize = 128;
/// Maximum number of values a decode may produce (shared references count
/// every time they are expanded).
pub const MAX_NODES: usize = 1 << 20;
/// Maximum total bytes of strings and data a decode may produce.
pub const MAX_BYTES: usize = 64 << 20;

const MAGIC: &[u8; 8] = b"bplist00";
const TRAILER_LEN: usize = 32;

/// A property list value.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Plist {
    #[default]
    Null,
    Bool(bool),
    /// Integers read from 1-, 2-, 4-byte (unsigned) and 8-, 16-byte (signed)
    /// encodings.
    Integer(i128),
    Real(f64),
    /// Seconds since 2001-01-01 00:00:00 UTC.
    Date(f64),
    Data(Vec<u8>),
    String(String),
    Array(Vec<Plist>),
    Dict(Dict),
}

static NULL: Plist = Plist::Null;

/// An ordered dictionary with string keys.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict(Vec<(String, Plist)>);

impl Dict {
    pub fn new() -> Dict {
        Dict::default()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value for `key` (the last one if the key repeats).
    pub fn get(&self, key: &str) -> Option<&Plist> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Plist> {
        self.0
            .iter_mut()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Sets `key`, replacing an existing entry in place or appending.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Plist>) {
        let key = key.into();
        let value = value.into();
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.0.push((key, value)),
        }
    }

    /// Removes every entry for `key`, returning the last value.
    pub fn remove(&mut self, key: &str) -> Option<Plist> {
        let mut rv = None;
        self.0.retain_mut(|(k, v)| {
            if k == key {
                rv = Some(std::mem::take(v));
                false
            } else {
                true
            }
        });
        rv
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Plist)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
}

impl<K: Into<String>, V: Into<Plist>> FromIterator<(K, V)> for Dict {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Dict {
        let mut d = Dict::new();
        for (k, v) in iter {
            d.insert(k, v);
        }
        d
    }
}

impl Plist {
    /// An empty dict, for use with [`Plist::with`].
    pub fn dict() -> Plist {
        Plist::Dict(Dict::new())
    }

    pub fn array<T: Into<Plist>>(items: impl IntoIterator<Item = T>) -> Plist {
        Plist::Array(items.into_iter().map(Into::into).collect())
    }

    /// Builder: sets `key` on a dict. A non-dict is replaced by a new dict
    /// holding just this entry.
    pub fn with(self, key: impl Into<String>, value: impl Into<Plist>) -> Plist {
        let mut d = match self {
            Plist::Dict(d) => d,
            _ => Dict::new(),
        };
        d.insert(key, value);
        Plist::Dict(d)
    }

    /// Builder: like [`Plist::with`] but skips `None`.
    pub fn with_opt<T: Into<Plist>>(self, key: impl Into<String>, value: Option<T>) -> Plist {
        match value {
            Some(v) => self.with(key, v),
            None => self,
        }
    }

    /// A data value.
    pub fn data(bytes: impl Into<Vec<u8>>) -> Plist {
        Plist::Data(bytes.into())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Plist::Null)
    }

    pub fn get(&self, key: &str) -> Option<&Plist> {
        self.as_dict().and_then(|d| d.get(key))
    }

    pub fn at(&self, idx: usize) -> Option<&Plist> {
        self.as_array().and_then(|a| a.get(idx))
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Plist::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i128(&self) -> Option<i128> {
        match self {
            Plist::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_i128().and_then(|i| i64::try_from(i).ok())
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_i128().and_then(|i| u64::try_from(i).ok())
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_i128().and_then(|i| u32::try_from(i).ok())
    }

    /// Reals, and integers converted to `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Plist::Real(f) => Some(*f),
            Plist::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_date(&self) -> Option<f64> {
        match self {
            Plist::Date(d) => Some(*d),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Plist::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Plist::Data(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Plist]> {
        match self {
            Plist::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Plist::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match self {
            Plist::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// The value for `key`, or a protocol error naming it.
    pub fn require(&self, key: &str) -> Result<&Plist> {
        self.get(key)
            .ok_or_else(|| Error::protocol(format!("bplist: missing key {key:?}")))
    }

    pub fn require_str(&self, key: &str) -> Result<&str> {
        self.require(key)?
            .as_str()
            .ok_or_else(|| wrong_type(key, "a string"))
    }

    pub fn require_data(&self, key: &str) -> Result<&[u8]> {
        self.require(key)?
            .as_data()
            .ok_or_else(|| wrong_type(key, "data"))
    }

    pub fn require_u64(&self, key: &str) -> Result<u64> {
        self.require(key)?
            .as_u64()
            .ok_or_else(|| wrong_type(key, "an unsigned 64-bit integer"))
    }

    pub fn require_i64(&self, key: &str) -> Result<i64> {
        self.require(key)?
            .as_i64()
            .ok_or_else(|| wrong_type(key, "a 64-bit integer"))
    }

    pub fn require_bool(&self, key: &str) -> Result<bool> {
        self.require(key)?
            .as_bool()
            .ok_or_else(|| wrong_type(key, "a boolean"))
    }

    pub fn require_array(&self, key: &str) -> Result<&[Plist]> {
        self.require(key)?
            .as_array()
            .ok_or_else(|| wrong_type(key, "an array"))
    }

    pub fn require_dict(&self, key: &str) -> Result<&Dict> {
        self.require(key)?
            .as_dict()
            .ok_or_else(|| wrong_type(key, "a dict"))
    }

    /// Parses a `bplist00` file (shorthand for [`decode`]).
    pub fn from_bytes(data: &[u8]) -> Result<Plist> {
        decode(data)
    }

    /// Serializes as `bplist00` (shorthand for [`encode`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        encode(self)
    }
}

fn wrong_type(key: &str, what: &str) -> Error {
    Error::protocol(format!("bplist: value for {key:?} is not {what}"))
}

impl std::ops::Index<&str> for Plist {
    type Output = Plist;

    /// `p["key"]`: the value, or `Null` when missing or not a dict.
    fn index(&self, key: &str) -> &Plist {
        self.get(key).unwrap_or(&NULL)
    }
}

impl std::ops::Index<usize> for Plist {
    type Output = Plist;

    /// `p[3]`: the element, or `Null` when out of range or not an array.
    fn index(&self, idx: usize) -> &Plist {
        self.at(idx).unwrap_or(&NULL)
    }
}

macro_rules! from_int {
    ($($ty:ty),*) => {$(
        impl From<$ty> for Plist {
            fn from(v: $ty) -> Plist {
                Plist::Integer(v as i128)
            }
        }
    )*};
}

from_int!(i8, i16, i32, i64, i128, u8, u16, u32, u64, usize);

impl From<bool> for Plist {
    fn from(v: bool) -> Plist {
        Plist::Bool(v)
    }
}

impl From<f64> for Plist {
    fn from(v: f64) -> Plist {
        Plist::Real(v)
    }
}

impl From<f32> for Plist {
    fn from(v: f32) -> Plist {
        Plist::Real(v as f64)
    }
}

impl From<&str> for Plist {
    fn from(v: &str) -> Plist {
        Plist::String(v.to_owned())
    }
}

impl From<String> for Plist {
    fn from(v: String) -> Plist {
        Plist::String(v)
    }
}

impl From<&String> for Plist {
    fn from(v: &String) -> Plist {
        Plist::String(v.clone())
    }
}

impl From<Dict> for Plist {
    fn from(v: Dict) -> Plist {
        Plist::Dict(v)
    }
}

impl<T: Into<Plist>> From<Vec<T>> for Plist {
    fn from(v: Vec<T>) -> Plist {
        Plist::array(v)
    }
}

impl<T: Into<Plist>> From<Option<T>> for Plist {
    fn from(v: Option<T>) -> Plist {
        v.map_or(Plist::Null, Into::into)
    }
}

// --- reader ---

/// Parses a `bplist00` file.
pub fn decode(data: &[u8]) -> Result<Plist> {
    if data.len() < MAGIC.len() + TRAILER_LEN + 2 || &data[..MAGIC.len()] != MAGIC {
        return Err(Error::malformed("bplist: not a bplist00 file"));
    }
    let mut t = Reader::new(&data[data.len() - TRAILER_LEN..], "bplist");
    t.skip(6)?; // unused + sort version
    let offset_size = t.u8()? as usize;
    let ref_size = t.u8()? as usize;
    let num_objects = t.u64_be()?;
    let top = t.u64_be()?;
    let table_offset = t.u64_be()?;
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) {
        return Err(Error::malformed("bplist: invalid offset or reference size"));
    }
    let body_end = data.len() - TRAILER_LEN;
    let table_offset = usize::try_from(table_offset)
        .ok()
        .filter(|&o| o > MAGIC.len() && o <= body_end)
        .ok_or_else(|| Error::malformed("bplist: offset table out of range"))?;
    let num_objects = usize::try_from(num_objects)
        .ok()
        .filter(|&n| n > 0)
        .filter(|&n| {
            n.checked_mul(offset_size)
                .and_then(|len| len.checked_add(table_offset))
                .is_some_and(|end| end <= body_end)
        })
        .ok_or_else(|| Error::malformed("bplist: object count out of range"))?;
    if top >= num_objects as u64 {
        return Err(Error::malformed("bplist: top object out of range"));
    }
    let mut d = Decoder {
        objects: &data[..table_offset],
        table: &data[table_offset..table_offset + num_objects * offset_size],
        offset_size,
        ref_size,
        num_objects,
        path: Vec::new(),
        nodes: 0,
        bytes: 0,
    };
    d.object(top as usize)
}

struct Decoder<'a> {
    /// Header and objects (everything before the offset table).
    objects: &'a [u8],
    table: &'a [u8],
    offset_size: usize,
    ref_size: usize,
    num_objects: usize,
    /// Object numbers of the containers being decoded (cycle detection).
    path: Vec<usize>,
    nodes: usize,
    bytes: usize,
}

impl<'a> Decoder<'a> {
    fn offset_of(&self, idx: usize) -> Result<usize> {
        let mut r = Reader::new(self.table, "bplist");
        r.seek(idx * self.offset_size)?;
        let off = r.uint_be(self.offset_size)?;
        usize::try_from(off)
            .ok()
            .filter(|&o| o >= MAGIC.len() && o < self.objects.len())
            .ok_or_else(|| Error::malformed(format!("bplist: object {idx} offset out of range")))
    }

    /// The count in a marker's low nibble, or the integer object after it.
    fn count(r: &mut Reader<'a>, low: u8) -> Result<usize> {
        if low != 0x0f {
            return Ok(low as usize);
        }
        let marker = r.u8()?;
        if marker & 0xf0 != 0x10 || marker & 0x0f > 3 {
            return Err(r.error("invalid count"));
        }
        let n = r.uint_be(1 << (marker & 0x0f))?;
        usize::try_from(n).map_err(|_| r.error("count too large"))
    }

    fn take_bytes(&mut self, r: &mut Reader<'a>, n: usize) -> Result<&'a [u8]> {
        let b = r.bytes(n)?;
        self.bytes += n;
        if self.bytes > MAX_BYTES {
            return Err(Error::malformed("bplist: too much string/data content"));
        }
        Ok(b)
    }

    fn refs(&self, r: &mut Reader<'a>, count: usize) -> Result<Vec<usize>> {
        let len = count
            .checked_mul(self.ref_size)
            .ok_or_else(|| r.error("container too large"))?;
        let mut refs = r.sub(len)?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let v = refs.uint_be(self.ref_size)?;
            match usize::try_from(v) {
                Ok(i) if i < self.num_objects => out.push(i),
                _ => return Err(refs.error("object reference out of range")),
            }
        }
        Ok(out)
    }

    fn object(&mut self, idx: usize) -> Result<Plist> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(Error::malformed("bplist: too many objects after expansion"));
        }
        let offset = self.offset_of(idx)?;
        let mut r = Reader::new(self.objects, "bplist");
        r.seek(offset)?;
        let marker = r.u8()?;
        let low = marker & 0x0f;
        Ok(match marker >> 4 {
            0x0 => match marker {
                0x00 => Plist::Null,
                0x08 => Plist::Bool(false),
                0x09 => Plist::Bool(true),
                _ => return Err(r.error("unsupported marker")),
            },
            0x1 => match low {
                0 => Plist::Integer(r.u8()? as i128),
                1 => Plist::Integer(r.u16_be()? as i128),
                2 => Plist::Integer(r.u32_be()? as i128),
                3 => Plist::Integer(r.i64_be()? as i128),
                4 => Plist::Integer(i128::from_be_bytes(r.array()?)),
                _ => return Err(r.error("invalid integer size")),
            },
            0x2 => match low {
                2 => Plist::Real(f32::from_bits(r.u32_be()?) as f64),
                3 => Plist::Real(f64::from_bits(r.u64_be()?)),
                _ => return Err(r.error("invalid real size")),
            },
            0x3 if marker == 0x33 => Plist::Date(f64::from_bits(r.u64_be()?)),
            0x4 => {
                let n = Self::count(&mut r, low)?;
                Plist::Data(self.take_bytes(&mut r, n)?.to_vec())
            }
            0x5 => {
                let n = Self::count(&mut r, low)?;
                let b = self.take_bytes(&mut r, n)?;
                // Should be ASCII; other bytes are read as Latin-1.
                Plist::String(b.iter().map(|&c| c as char).collect())
            }
            0x6 => {
                let n = Self::count(&mut r, low)?;
                let len = n.checked_mul(2).ok_or_else(|| r.error("string too long"))?;
                let b = self.take_bytes(&mut r, len)?;
                let units = b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                Plist::String(
                    char::decode_utf16(units)
                        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                        .collect(),
                )
            }
            0xa => {
                let n = Self::count(&mut r, low)?;
                let refs = self.refs(&mut r, n)?;
                self.enter(idx)?;
                let mut items = Vec::with_capacity(refs.len());
                for i in refs {
                    items.push(self.object(i)?);
                }
                self.path.pop();
                Plist::Array(items)
            }
            0xd => {
                let n = Self::count(&mut r, low)?;
                let total = n.checked_mul(2).ok_or_else(|| r.error("dict too large"))?;
                let refs = self.refs(&mut r, total)?;
                self.enter(idx)?;
                let mut entries = Vec::with_capacity(n);
                for i in 0..n {
                    let key = match self.object(refs[i])? {
                        Plist::String(s) => s,
                        _ => return Err(Error::malformed("bplist: dict key is not a string")),
                    };
                    let value = self.object(refs[n + i])?;
                    entries.push((key, value));
                }
                self.path.pop();
                Plist::Dict(Dict(entries))
            }
            0x8 => return Err(Error::unsupported("bplist: UID objects are not supported")),
            0xc => return Err(Error::unsupported("bplist: sets are not supported")),
            _ => return Err(r.error("unsupported marker")),
        })
    }

    fn enter(&mut self, idx: usize) -> Result<()> {
        if self.path.contains(&idx) {
            return Err(Error::malformed("bplist: reference cycle"));
        }
        if self.path.len() >= MAX_DEPTH {
            return Err(Error::malformed("bplist: nesting too deep"));
        }
        self.path.push(idx);
        Ok(())
    }
}

// --- writer ---

/// Identity of a shareable scalar.
#[derive(PartialEq, Eq, Hash)]
enum Key<'a> {
    Null,
    Bool(bool),
    Int(i128),
    Real(u64),
    Date(u64),
    Data(&'a [u8]),
    Str(&'a str),
}

enum Flat<'a> {
    Scalar(&'a Plist),
    Str(&'a str),
    Array(Vec<usize>),
    Dict(Vec<usize>, Vec<usize>),
}

struct Flattener<'a> {
    objects: Vec<Flat<'a>>,
    shared: HashMap<Key<'a>, usize>,
}

impl<'a> Flattener<'a> {
    fn scalar(&mut self, key: Key<'a>, flat: Flat<'a>) -> usize {
        if let Some(&i) = self.shared.get(&key) {
            return i;
        }
        let i = self.objects.len();
        self.objects.push(flat);
        self.shared.insert(key, i);
        i
    }

    fn value(&mut self, v: &'a Plist) -> usize {
        let key = match v {
            Plist::Null => Key::Null,
            Plist::Bool(b) => Key::Bool(*b),
            Plist::Integer(i) => Key::Int(*i),
            Plist::Real(f) => Key::Real(f.to_bits()),
            Plist::Date(f) => Key::Date(f.to_bits()),
            Plist::Data(d) => Key::Data(d),
            Plist::String(s) => return self.scalar(Key::Str(s), Flat::Str(s)),
            Plist::Array(items) => {
                let i = self.objects.len();
                self.objects.push(Flat::Array(Vec::new()));
                let refs = items.iter().map(|v| self.value(v)).collect();
                self.objects[i] = Flat::Array(refs);
                return i;
            }
            Plist::Dict(d) => {
                let i = self.objects.len();
                self.objects.push(Flat::Dict(Vec::new(), Vec::new()));
                let keys =
                    d.0.iter()
                        .map(|(k, _)| self.scalar(Key::Str(k), Flat::Str(k)))
                        .collect();
                let values = d.0.iter().map(|(_, v)| self.value(v)).collect();
                self.objects[i] = Flat::Dict(keys, values);
                return i;
            }
        };
        self.scalar(key, Flat::Scalar(v))
    }
}

/// Bytes needed for values below `n`'s range (1, 2, 4 or 8).
fn size_for(n: u64) -> usize {
    if n < 1 << 8 {
        1
    } else if n < 1 << 16 {
        2
    } else if n < 1 << 32 {
        4
    } else {
        8
    }
}

fn write_count(w: &mut Writer, kind: u8, n: usize) {
    if n < 15 {
        w.u8(kind | n as u8);
    } else {
        let size = size_for(n as u64);
        w.u8(kind | 0x0f)
            .u8(0x10 | size.trailing_zeros() as u8)
            .uint_be(n as u64, size);
    }
}

fn write_string(w: &mut Writer, s: &str) {
    if s.is_ascii() {
        write_count(w, 0x50, s.len());
        w.bytes(s.as_bytes());
    } else {
        let units: Vec<u16> = s.encode_utf16().collect();
        write_count(w, 0x60, units.len());
        for u in units {
            w.u16_be(u);
        }
    }
}

fn write_int(w: &mut Writer, i: i128) {
    match i {
        0..=0xff => w.u8(0x10).u8(i as u8),
        0x100..=0xffff => w.u8(0x11).u16_be(i as u16),
        0x1_0000..=0xffff_ffff => w.u8(0x12).u32_be(i as u32),
        _ => match i64::try_from(i) {
            Ok(v) => w.u8(0x13).i64_be(v),
            Err(_) => w.u8(0x14).bytes(&i.to_be_bytes()),
        },
    };
}

/// Serializes as `bplist00`.
pub fn encode(v: &Plist) -> Vec<u8> {
    let mut f = Flattener {
        objects: Vec::new(),
        shared: HashMap::new(),
    };
    let top = f.value(v);
    let num = f.objects.len();
    let ref_size = size_for(num as u64);
    let mut w = Writer::new();
    w.bytes(MAGIC);
    let mut offsets = Vec::with_capacity(num);
    for obj in &f.objects {
        offsets.push(w.len() as u64);
        match obj {
            Flat::Str(s) => write_string(&mut w, s),
            Flat::Array(refs) => {
                write_count(&mut w, 0xa0, refs.len());
                for &r in refs {
                    w.uint_be(r as u64, ref_size);
                }
            }
            Flat::Dict(keys, values) => {
                write_count(&mut w, 0xd0, keys.len());
                for &r in keys.iter().chain(values) {
                    w.uint_be(r as u64, ref_size);
                }
            }
            Flat::Scalar(p) => match p {
                Plist::Null => {
                    w.u8(0x00);
                }
                Plist::Bool(b) => {
                    w.u8(if *b { 0x09 } else { 0x08 });
                }
                Plist::Integer(i) => write_int(&mut w, *i),
                Plist::Real(f) => {
                    w.u8(0x23).u64_be(f.to_bits());
                }
                Plist::Date(f) => {
                    w.u8(0x33).u64_be(f.to_bits());
                }
                Plist::Data(d) => {
                    write_count(&mut w, 0x40, d.len());
                    w.bytes(d);
                }
                // Strings and containers are never `Flat::Scalar`.
                Plist::String(s) => write_string(&mut w, s),
                Plist::Array(_) | Plist::Dict(_) => {}
            },
        }
    }
    let table_offset = w.len() as u64;
    let offset_size = size_for(table_offset);
    for off in offsets {
        w.uint_be(off, offset_size);
    }
    w.zeros(6)
        .u8(offset_size as u8)
        .u8(ref_size as u8)
        .u64_be(num as u64)
        .u64_be(top as u64)
        .u64_be(table_offset);
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::fuzz::Rng;
    use crate::codec::json::Value;
    use crate::util::{hex, vectors};

    /// The tagged-JSON form used by the fixture manifests (see
    /// scripts/vectors/bplist_fixtures.py).
    fn tagged(p: &Plist) -> Value {
        match p {
            Plist::Null => Value::Null,
            Plist::Bool(b) => Value::Bool(*b),
            Plist::Integer(i) => Value::object().with("int", i.to_string()),
            Plist::Real(f) => Value::object().with("real", hex::encode(&f.to_bits().to_be_bytes())),
            Plist::Date(f) => Value::object().with("date", hex::encode(&f.to_bits().to_be_bytes())),
            Plist::Data(d) => Value::object().with("data", hex::encode(d)),
            Plist::String(s) => Value::from(s),
            Plist::Array(a) => Value::array(a.iter().map(tagged)),
            Plist::Dict(d) => Value::object().with(
                "dict",
                Value::array(
                    d.iter()
                        .map(|(k, v)| Value::array([Value::from(k), tagged(v)])),
                ),
            ),
        }
    }

    fn untagged(v: &Value) -> Plist {
        match v {
            Value::Null => Plist::Null,
            Value::Bool(b) => Plist::Bool(*b),
            Value::String(s) => Plist::String(s.clone()),
            Value::Array(a) => Plist::Array(a.iter().map(untagged).collect()),
            Value::Object(o) => {
                let (tag, inner) = o.iter().next().expect("tag");
                let bits = |s: &Value| {
                    let b = hex::decode(s.as_str().unwrap()).unwrap();
                    f64::from_bits(u64::from_be_bytes(b.try_into().unwrap()))
                };
                match tag {
                    "int" => Plist::Integer(inner.as_str().unwrap().parse().unwrap()),
                    "real" => Plist::Real(bits(inner)),
                    "date" => Plist::Date(bits(inner)),
                    "data" => Plist::Data(hex::decode(inner.as_str().unwrap()).unwrap()),
                    "dict" => Plist::Dict(
                        inner
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|kv| (kv[0].as_str().unwrap().to_owned(), untagged(&kv[1])))
                            .collect(),
                    ),
                    _ => panic!("bad tag {tag}"),
                }
            }
            Value::Number(_) => panic!("bare number in tagged plist"),
        }
    }

    fn input_of(b: &vectors::Block) -> Vec<u8> {
        match b.get("file") {
            Some(f) => vectors::fixture(&format!("codec/bplist/{f}")),
            None => b.hex("encoded"),
        }
    }

    #[test]
    fn vectors() {
        let blocks = vectors::load("codec/bplist.txt");
        assert!(blocks.len() >= 20);
        let mut plutil = 0;
        let mut exact = 0;
        for b in &blocks {
            let name = b.name();
            let input = input_of(b);
            if let Some(msg) = b.get("error") {
                let err = decode(&input).expect_err(&name);
                assert!(err.to_string().contains(msg), "{name}: {err}");
                continue;
            }
            let expected_text = match b.get("expected_file") {
                Some(f) => {
                    String::from_utf8(vectors::fixture(&format!("codec/bplist/{f}"))).unwrap()
                }
                None => b.str("expected").to_owned(),
            };
            let expected = untagged(&crate::codec::json::parse(&expected_text).unwrap());
            let got = decode(&input).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(tagged(&got), tagged(&expected), "{name}: decode");
            // Our encoding must decode to the same value.
            let ours = encode(&got);
            assert_eq!(
                tagged(&decode(&ours).unwrap()),
                tagged(&expected),
                "{name}: roundtrip"
            );
            match b.str("origin") {
                // plistlib (sort_keys=False) output must match byte for byte.
                "plistlib" => {
                    assert_eq!(hex::encode(&ours), hex::encode(&input), "{name}: encode");
                    exact += 1;
                }
                "plutil" => plutil += 1,
                _ => {}
            }
        }
        assert!(plutil >= 3 && exact >= 5, "plutil {plutil} exact {exact}");
    }

    /// With `TVPIPE_BPLIST_OUT=<dir>`, writes our re-encoding of every
    /// fixture there so an external tool can check it, e.g.
    /// `plutil -lint <dir>/*.bplist` (done by scripts/vectors/bplist_check.sh).
    #[test]
    fn write_reencodings_for_external_check() {
        let Some(dir) = std::env::var_os("TVPIPE_BPLIST_OUT") else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        for b in vectors::load("codec/bplist.txt") {
            if b.get("error").is_some() {
                continue;
            }
            let p = decode(&input_of(&b)).unwrap();
            let name = b.name().replace(' ', "-");
            let path = std::path::Path::new(&dir).join(format!("{name}.bplist"));
            std::fs::write(path, encode(&p)).unwrap();
        }
    }

    #[test]
    fn info_like_getters() {
        let p = Plist::dict()
            .with("deviceID", "AA:BB:CC:DD:EE:FF")
            .with("features", 0x1_c340_405f_4a00_u64)
            .with("pk", Plist::data(vec![1u8; 32]))
            .with("statusFlags", 4)
            .with(
                "displays",
                vec![Plist::dict()
                    .with("widthPixels", 1920)
                    .with("heightPixels", 1080)
                    .with("refreshRate", 60.0)],
            )
            .with_opt("missing", None::<i32>);
        let back = decode(&encode(&p)).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.require_str("deviceID").unwrap(), "AA:BB:CC:DD:EE:FF");
        assert_eq!(back.require_u64("features").unwrap(), 0x1_c340_405f_4a00);
        assert_eq!(back.require_data("pk").unwrap().len(), 32);
        assert_eq!(back["displays"][0]["widthPixels"].as_u32(), Some(1920));
        assert_eq!(back["displays"][0]["refreshRate"].as_f64(), Some(60.0));
        assert!(back["nope"][3].is_null());
        assert!(back.require("missing").is_err());
        let err = back.require_str("statusFlags").unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Protocol);
    }

    #[test]
    fn integer_encodings() {
        let cases: &[(i128, &str)] = &[
            (0, "1000"),
            (255, "10ff"),
            (256, "110100"),
            (65_535, "11ffff"),
            (65_536, "1200010000"),
            (0xffff_ffff, "12ffffffff"),
            (0x1_0000_0000, "130000000100000000"),
            (-1, "13ffffffffffffffff"),
            (i64::MAX as i128, "137fffffffffffffff"),
            (i64::MIN as i128, "138000000000000000"),
            (1 << 63, "1400000000000000008000000000000000"),
            (u64::MAX as i128, "140000000000000000ffffffffffffffff"),
            (i128::MIN, "1480000000000000000000000000000000"),
        ];
        for &(v, enc) in cases {
            let mut w = Writer::new();
            write_int(&mut w, v);
            assert_eq!(hex::encode(w.as_slice()), enc, "{v}");
            let file = encode(&Plist::Integer(v));
            assert_eq!(decode(&file).unwrap(), Plist::Integer(v));
        }
    }

    #[test]
    fn count_encodings() {
        for (n, prefix) in [
            (0usize, "40"),
            (14, "4e"),
            (15, "4f100f"),
            (255, "4f10ff"),
            (256, "4f110100"),
            (70_000, "4f1200011170"),
        ] {
            let file = encode(&Plist::Data(vec![7; n]));
            assert!(hex::encode(&file[8..]).starts_with(prefix), "{n}");
            assert_eq!(decode(&file).unwrap(), Plist::Data(vec![7; n]));
        }
    }

    #[test]
    fn many_objects_use_wide_refs() {
        // > 255 distinct objects need 2-byte refs; > 64 KiB needs 4-byte
        // offsets.
        let p = Plist::array((0..70_000u32).map(|i| format!("s{i}")));
        let file = encode(&p);
        let t = &file[file.len() - 32..];
        assert_eq!((t[6], t[7]), (4, 4));
        assert_eq!(decode(&file).unwrap(), p);
        let p = Plist::array((0..300u32).map(Plist::from));
        let file = encode(&p);
        let t = &file[file.len() - 32..];
        assert_eq!((t[6], t[7]), (2, 2));
        assert_eq!(decode(&file).unwrap(), p);
    }

    #[test]
    fn strings() {
        for s in ["", "plain", "grüße", "日本語", "😀 emoji", "\u{0}"] {
            let p = Plist::from(s);
            assert_eq!(decode(&encode(&p)).unwrap(), p, "{s:?}");
        }
        // Lone surrogate in a UTF-16 string decodes to U+FFFD.
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(&[0x62, 0xd8, 0x00, 0x00, 0x41]);
        raw.push(8);
        raw.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 1]);
        raw.extend_from_slice(&1u64.to_be_bytes());
        raw.extend_from_slice(&0u64.to_be_bytes());
        raw.extend_from_slice(&13u64.to_be_bytes());
        assert_eq!(decode(&raw).unwrap(), Plist::from("\u{fffd}A"));
    }

    #[test]
    fn shared_reference_blowup_is_bounded() {
        // Object i is an array [i+1, i+1]; 40 levels expand to 2^40 nodes.
        let levels = 40usize;
        let mut objects: Vec<Vec<u8>> = (0..levels)
            .map(|i| vec![0xa2, (i + 1) as u8, (i + 1) as u8])
            .collect();
        objects.push(vec![0x09]);
        let mut raw = MAGIC.to_vec();
        let mut offsets = Vec::new();
        for o in &objects {
            offsets.push(raw.len() as u8);
            raw.extend_from_slice(o);
        }
        let table = raw.len() as u64;
        raw.extend_from_slice(&offsets);
        raw.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 1]);
        raw.extend_from_slice(&(objects.len() as u64).to_be_bytes());
        raw.extend_from_slice(&0u64.to_be_bytes());
        raw.extend_from_slice(&table.to_be_bytes());
        let err = decode(&raw).unwrap_err();
        assert!(err.to_string().contains("too many objects"), "{err}");
    }

    #[test]
    fn fuzz_smoke() {
        let seeds: Vec<Vec<u8>> = vectors::load("codec/bplist.txt")
            .iter()
            .map(input_of)
            .filter(|s| s.len() < 4096)
            .collect();
        assert!(seeds.len() > 10);
        let mut rng = Rng::new(0x6270_6c73);
        for _ in 0..10_000 {
            let input = rng.mutate_any(&seeds);
            if let Ok(p) = decode(&input) {
                assert_eq!(tagged(&decode(&encode(&p)).unwrap()), tagged(&p));
            }
        }
    }
}
