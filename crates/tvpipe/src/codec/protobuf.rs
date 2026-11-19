//! Protocol Buffers wire format (the subset we need) and the Cast
//! `CastMessage`.
//!
//! The wire helpers ([`put_varint`], [`Fields`], ...) are generic so other
//! small messages (e.g. Cast device auth) can reuse them. Unknown fields are
//! skipped, including deprecated groups (nested at most [`MAX_GROUP_DEPTH`]
//! deep).
//!
//! `CastMessage` from `cast_channel.proto` (proto2):
//!
//! | # | field            | type                                  |
//! |---|------------------|---------------------------------------|
//! | 1 | protocol_version | enum, `CASTV2_1_0 = 0`                |
//! | 2 | source_id        | string                                |
//! | 3 | destination_id   | string                                |
//! | 4 | namespace        | string                                |
//! | 5 | payload_type     | enum, `STRING = 0`, `BINARY = 1`      |
//! | 6 | payload_utf8     | optional string                       |
//! | 7 | payload_binary   | optional bytes                        |
//!
//! Sources: protobuf.dev "Encoding" (wire format); pychromecast
//! `chromecast_protobuf/cast_channel.proto` (MIT, from Chromium, BSD) for the
//! field numbers.

use crate::error::{Error, Result};
use crate::util::bytes::{Reader, Writer};

/// Protobuf wire types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireType {
    Varint = 0,
    Fixed64 = 1,
    Len = 2,
    StartGroup = 3,
    Fixed32 = 5,
}

/// Maximum nesting of (skipped) groups.
pub const MAX_GROUP_DEPTH: usize = 32;

/// One decoded field value, borrowing from the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldValue<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    /// A deprecated group: the raw bytes between its start and end keys.
    Group(&'a [u8]),
    Fixed32(u32),
}

impl FieldValue<'_> {
    pub fn wire_type(&self) -> WireType {
        match self {
            FieldValue::Varint(_) => WireType::Varint,
            FieldValue::Fixed64(_) => WireType::Fixed64,
            FieldValue::Len(_) => WireType::Len,
            FieldValue::Group(_) => WireType::StartGroup,
            FieldValue::Fixed32(_) => WireType::Fixed32,
        }
    }
}

/// Appends a base-128 varint.
pub fn put_varint(w: &mut Writer, mut v: u64) {
    while v >= 0x80 {
        w.u8((v as u8) | 0x80);
        v >>= 7;
    }
    w.u8(v as u8);
}

/// Reads a base-128 varint (at most 10 bytes, no bits beyond 64).
pub fn read_varint(r: &mut Reader<'_>) -> Result<u64> {
    let mut v = 0u64;
    for i in 0..10 {
        let b = r.u8()?;
        if i == 9 && b > 0x01 {
            return Err(r.error("varint overflows 64 bits"));
        }
        v |= ((b & 0x7f) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(r.error("varint longer than 10 bytes"))
}

/// Appends a field key (tag).
pub fn put_key(w: &mut Writer, field: u32, wt: WireType) {
    put_varint(w, ((field as u64) << 3) | wt as u64);
}

pub fn put_varint_field(w: &mut Writer, field: u32, v: u64) {
    put_key(w, field, WireType::Varint);
    put_varint(w, v);
}

pub fn put_bytes_field(w: &mut Writer, field: u32, v: &[u8]) {
    put_key(w, field, WireType::Len);
    put_varint(w, v.len() as u64);
    w.bytes(v);
}

/// Iterator over the fields of an encoded message: `(field number, value)`.
/// Yields an error (and then stops) on malformed input.
pub struct Fields<'a> {
    r: Reader<'a>,
    failed: bool,
}

impl<'a> Fields<'a> {
    pub fn new(data: &'a [u8]) -> Fields<'a> {
        Fields {
            r: Reader::new(data, "protobuf"),
            failed: false,
        }
    }

    /// Reads a key: `(field number, wire type bits)`.
    fn key(&mut self) -> Result<(u32, u64)> {
        let key = read_varint(&mut self.r)?;
        let field = key >> 3;
        if field == 0 || field > (1 << 29) - 1 {
            return Err(self.r.error("invalid field number"));
        }
        Ok((field as u32, key & 7))
    }

    /// Skips a value of wire type `wt` (not end-group).
    fn skip_value(&mut self, wt: u64) -> Result<()> {
        match wt {
            0 => read_varint(&mut self.r).map(|_| ()),
            1 => self.r.skip(8),
            2 => {
                let len = read_varint(&mut self.r)?;
                let len = usize::try_from(len).map_err(|_| self.r.error("length too large"))?;
                self.r.skip(len)
            }
            5 => self.r.skip(4),
            _ => Err(self.r.error("invalid wire type")),
        }
    }

    /// Consumes a group body up to the end key matching `field`, returning
    /// the body bytes.
    fn group(&mut self, field: u32) -> Result<&'a [u8]> {
        let start = self.r.position();
        let mut open = vec![field];
        loop {
            let end = self.r.position();
            let (f, wt) = self.key()?;
            match wt {
                3 => {
                    if open.len() >= MAX_GROUP_DEPTH {
                        return Err(self.r.error("groups nested too deep"));
                    }
                    open.push(f);
                }
                4 => {
                    if open.pop() != Some(f) {
                        return Err(self.r.error("mismatched end-group"));
                    }
                    if open.is_empty() {
                        return Ok(&self.r.buffer()[start..end]);
                    }
                }
                _ => self.skip_value(wt)?,
            }
        }
    }

    fn next_field(&mut self) -> Result<(u32, FieldValue<'a>)> {
        let (field, wt) = self.key()?;
        let value = match wt {
            0 => FieldValue::Varint(read_varint(&mut self.r)?),
            1 => FieldValue::Fixed64(self.r.u64_le()?),
            2 => {
                let len = read_varint(&mut self.r)?;
                let len = usize::try_from(len).map_err(|_| self.r.error("length too large"))?;
                FieldValue::Len(self.r.bytes(len)?)
            }
            3 => FieldValue::Group(self.group(field)?),
            5 => FieldValue::Fixed32(self.r.u32_le()?),
            4 => return Err(self.r.error("unexpected end-group")),
            _ => return Err(self.r.error("invalid wire type")),
        };
        Ok((field, value))
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u32, FieldValue<'a>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.r.is_at_end() {
            return None;
        }
        let rv = self.next_field();
        self.failed = rv.is_err();
        Some(rv)
    }
}

/// `CastMessage.protocol_version` value `CASTV2_1_0`.
pub const CASTV2_1_0: u32 = 0;

/// The payload of a [`CastMessage`] (selected by `payload_type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// `payload_type = STRING`, `payload_utf8` (JSON for all namespaces we
    /// use).
    String(String),
    /// `payload_type = BINARY`, `payload_binary`.
    Binary(Vec<u8>),
}

/// A Cast channel message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastMessage {
    pub protocol_version: u32,
    pub source_id: String,
    pub destination_id: String,
    pub namespace: String,
    pub payload: Payload,
}

const F_PROTOCOL_VERSION: u32 = 1;
const F_SOURCE_ID: u32 = 2;
const F_DESTINATION_ID: u32 = 3;
const F_NAMESPACE: u32 = 4;
const F_PAYLOAD_TYPE: u32 = 5;
const F_PAYLOAD_UTF8: u32 = 6;
const F_PAYLOAD_BINARY: u32 = 7;

const PAYLOAD_STRING: u64 = 0;
const PAYLOAD_BINARY: u64 = 1;

impl CastMessage {
    /// A `STRING` message with protocol version `CASTV2_1_0`.
    pub fn text(
        source_id: impl Into<String>,
        destination_id: impl Into<String>,
        namespace: impl Into<String>,
        payload: impl Into<String>,
    ) -> CastMessage {
        CastMessage {
            protocol_version: CASTV2_1_0,
            source_id: source_id.into(),
            destination_id: destination_id.into(),
            namespace: namespace.into(),
            payload: Payload::String(payload.into()),
        }
    }

    /// A `STRING` message carrying a JSON value.
    pub fn json(
        source_id: impl Into<String>,
        destination_id: impl Into<String>,
        namespace: impl Into<String>,
        payload: &crate::codec::json::Value,
    ) -> CastMessage {
        CastMessage::text(source_id, destination_id, namespace, payload.to_string())
    }

    /// A `BINARY` message with protocol version `CASTV2_1_0`.
    pub fn binary(
        source_id: impl Into<String>,
        destination_id: impl Into<String>,
        namespace: impl Into<String>,
        payload: impl Into<Vec<u8>>,
    ) -> CastMessage {
        CastMessage {
            protocol_version: CASTV2_1_0,
            source_id: source_id.into(),
            destination_id: destination_id.into(),
            namespace: namespace.into(),
            payload: Payload::Binary(payload.into()),
        }
    }

    /// The UTF-8 payload, if this is a `STRING` message.
    pub fn payload_str(&self) -> Option<&str> {
        match &self.payload {
            Payload::String(s) => Some(s),
            Payload::Binary(_) => None,
        }
    }

    /// Parses the `STRING` payload as JSON.
    pub fn payload_json(&self) -> Result<crate::codec::json::Value> {
        match &self.payload {
            Payload::String(s) => crate::codec::json::parse(s),
            Payload::Binary(_) => Err(Error::protocol(format!(
                "cast: expected a string payload on {}",
                self.namespace
            ))),
        }
    }

    /// Serializes in field-number order, as protobuf encoders do.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(
            32 + self.source_id.len()
                + self.destination_id.len()
                + self.namespace.len()
                + match &self.payload {
                    Payload::String(s) => s.len(),
                    Payload::Binary(b) => b.len(),
                },
        );
        put_varint_field(&mut w, F_PROTOCOL_VERSION, self.protocol_version as u64);
        put_bytes_field(&mut w, F_SOURCE_ID, self.source_id.as_bytes());
        put_bytes_field(&mut w, F_DESTINATION_ID, self.destination_id.as_bytes());
        put_bytes_field(&mut w, F_NAMESPACE, self.namespace.as_bytes());
        match &self.payload {
            Payload::String(s) => {
                put_varint_field(&mut w, F_PAYLOAD_TYPE, PAYLOAD_STRING);
                put_bytes_field(&mut w, F_PAYLOAD_UTF8, s.as_bytes());
            }
            Payload::Binary(b) => {
                put_varint_field(&mut w, F_PAYLOAD_TYPE, PAYLOAD_BINARY);
                put_bytes_field(&mut w, F_PAYLOAD_BINARY, b);
            }
        }
        w.into_vec()
    }

    /// Serializes with the channel's `u32be length` prefix.
    pub fn encode_framed(&self) -> Vec<u8> {
        let body = self.encode();
        let mut w = Writer::with_capacity(body.len() + 4);
        w.u32_be(body.len() as u32).bytes(&body);
        w.into_vec()
    }

    /// Parses a message body (without the length prefix). Fields may come in
    /// any order; for repeated scalars the last one wins; unknown fields are
    /// skipped. The required string fields and `payload_type` must be
    /// present; a missing `protocol_version` reads as `CASTV2_1_0`. A
    /// missing payload field reads as empty.
    pub fn decode(data: &[u8]) -> Result<CastMessage> {
        let mut version = None;
        let mut source = None;
        let mut dest = None;
        let mut namespace = None;
        let mut payload_type = None;
        let mut utf8: Option<&[u8]> = None;
        let mut binary: Option<&[u8]> = None;
        for field in Fields::new(data) {
            let (num, value) = field?;
            match (num, value) {
                (F_PROTOCOL_VERSION, FieldValue::Varint(v)) => version = Some(v),
                (F_SOURCE_ID, FieldValue::Len(b)) => source = Some(b),
                (F_DESTINATION_ID, FieldValue::Len(b)) => dest = Some(b),
                (F_NAMESPACE, FieldValue::Len(b)) => namespace = Some(b),
                (F_PAYLOAD_TYPE, FieldValue::Varint(v)) => payload_type = Some(v),
                (F_PAYLOAD_UTF8, FieldValue::Len(b)) => utf8 = Some(b),
                (F_PAYLOAD_BINARY, FieldValue::Len(b)) => binary = Some(b),
                (1..=7, v) => {
                    return Err(Error::malformed(format!(
                        "cast message: field {num} has wrong wire type {:?}",
                        v.wire_type()
                    )))
                }
                _ => {}
            }
        }
        let text = |b: Option<&[u8]>, name: &str| -> Result<String> {
            let b = b.ok_or_else(|| {
                Error::malformed(format!("cast message: missing required field {name}"))
            })?;
            String::from_utf8(b.to_vec())
                .map_err(|_| Error::malformed(format!("cast message: {name} is not UTF-8")))
        };
        let protocol_version = u32::try_from(version.unwrap_or(0))
            .map_err(|_| Error::malformed("cast message: protocol_version out of range"))?;
        let source_id = text(source, "source_id")?;
        let destination_id = text(dest, "destination_id")?;
        let namespace = text(namespace, "namespace")?;
        let payload = match payload_type {
            // CASTV2_1_3 ("binary payload over utf8") carries STRING
            // payloads in payload_binary; real receivers use it once the
            // sender lists version 3 in CONNECT's protocolVersionList.
            Some(PAYLOAD_STRING) => {
                Payload::String(text(Some(utf8.or(binary).unwrap_or(b"")), "payload_utf8")?)
            }
            Some(PAYLOAD_BINARY) => Payload::Binary(binary.unwrap_or(b"").to_vec()),
            Some(other) => {
                return Err(Error::malformed(format!(
                    "cast message: unknown payload_type {other}"
                )))
            }
            None => return Err(Error::malformed("cast message: missing payload_type")),
        };
        Ok(CastMessage {
            protocol_version,
            source_id,
            destination_id,
            namespace,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::fuzz::Rng;
    use crate::util::vectors;

    fn message_of(b: &vectors::Block) -> CastMessage {
        let payload = match b.str("payload_type") {
            "STRING" => Payload::String(b.get("payload_utf8").unwrap_or("").to_owned()),
            "BINARY" => Payload::Binary(b.hex("payload_binary")),
            other => panic!("bad payload_type {other}"),
        };
        CastMessage {
            protocol_version: b.parse("protocol_version"),
            source_id: b.str("source_id").to_owned(),
            destination_id: b.str("destination_id").to_owned(),
            namespace: b.str("namespace").to_owned(),
            payload,
        }
    }

    #[test]
    fn vectors() {
        let blocks = vectors::load("codec/protobuf.txt");
        assert!(blocks.len() >= 8);
        for b in &blocks {
            let name = b.name();
            let encoded = b.hex("encoded");
            if b.get("error").is_some() {
                let err = CastMessage::decode(&encoded).expect_err(&name);
                assert_eq!(err.kind(), crate::error::ErrorKind::Malformed, "{name}");
                assert!(err.to_string().contains(b.str("error")), "{name}: {err}");
                continue;
            }
            let msg = message_of(b);
            assert_eq!(
                CastMessage::decode(&encoded).unwrap(),
                msg,
                "{name}: decode"
            );
            if b.get("decode_only").is_none() {
                assert_eq!(msg.encode(), encoded, "{name}: encode");
            }
        }
    }

    #[test]
    fn varints() {
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (150, &[0x96, 0x01]),
            (300, &[0xac, 0x02]),
            (16_384, &[0x80, 0x80, 0x01]),
            (
                u64::MAX,
                &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
            ),
        ];
        for &(v, enc) in cases {
            let mut w = Writer::new();
            put_varint(&mut w, v);
            assert_eq!(w.as_slice(), enc, "{v}");
            let mut r = Reader::new(enc, "protobuf");
            assert_eq!(read_varint(&mut r).unwrap(), v);
            assert!(r.is_at_end());
        }
        // Non-minimal encodings are accepted.
        let mut r = Reader::new(&[0x80, 0x00], "protobuf");
        assert_eq!(read_varint(&mut r).unwrap(), 0);
        // Overflow and over-length.
        let mut r = Reader::new(
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
            "protobuf",
        );
        assert!(read_varint(&mut r).is_err());
        let mut r = Reader::new(&[0x80; 11], "protobuf");
        assert!(read_varint(&mut r).is_err());
        let mut r = Reader::new(&[0x80], "protobuf");
        assert!(read_varint(&mut r).is_err());
    }

    #[test]
    fn fields_iterator() {
        // field 1 varint 150, field 2 "testing", field 3 fixed32, field 4 fixed64
        let data =
            crate::util::hex::decode("089601 120774657374696e67 1d01000000 210200000000000000")
                .unwrap();
        let fields: Vec<_> = Fields::new(&data).collect::<Result<_>>().unwrap();
        assert_eq!(
            fields,
            [
                (1, FieldValue::Varint(150)),
                (2, FieldValue::Len(b"testing")),
                (3, FieldValue::Fixed32(1)),
                (4, FieldValue::Fixed64(2)),
            ]
        );
        // A malformed field ends iteration after one error.
        let mut it = Fields::new(&[0x08]);
        assert!(it.next().unwrap().is_err());
        assert!(it.next().is_none());
    }

    #[test]
    fn json_helpers() {
        let v = crate::codec::json::Value::object().with("type", "PING");
        let m = CastMessage::json(
            "sender-0",
            "receiver-0",
            "urn:x-cast:com.google.cast.tp.heartbeat",
            &v,
        );
        assert_eq!(m.payload_str(), Some(r#"{"type":"PING"}"#));
        assert_eq!(m.payload_json().unwrap(), v);
        let framed = m.encode_framed();
        assert_eq!(&framed[..4], &(framed.len() as u32 - 4).to_be_bytes());
        assert_eq!(CastMessage::decode(&framed[4..]).unwrap(), m);
        let b = CastMessage::binary("a", "b", "c", vec![1, 2]);
        assert!(b.payload_json().is_err());
        assert_eq!(b.payload_str(), None);
    }

    #[test]
    fn fuzz_smoke() {
        let seeds: Vec<Vec<u8>> = vectors::load("codec/protobuf.txt")
            .iter()
            .map(|b| b.hex("encoded"))
            .collect();
        let mut rng = Rng::new(0x7072_6f74);
        for _ in 0..10_000 {
            let input = rng.mutate_any(&seeds);
            if let Ok(m) = CastMessage::decode(&input) {
                assert_eq!(CastMessage::decode(&m.encode()).unwrap(), m);
            }
            for f in Fields::new(&input) {
                if f.is_err() {
                    break;
                }
            }
        }
    }

    #[test]
    fn v1_3_string_payload_in_binary_field() {
        // Captured from a Google TV (Sony XR) after CONNECT with
        // protocolVersionList [0,1,2,3]: protocol_version 3, payload_type
        // STRING, JSON in field 7.
        let data = crate::util::hex::decode(
            "0803120a72656365697665722d301a0873656e6465722d30222875726e3a782d636173743a\
             636f6d2e676f6f676c652e636173742e74702e636f6e6e656374696f6e28003a287b227072\
             6f746f636f6c56657273696f6e223a332c2274797065223a22434f4e4e4543544544227d",
        )
        .unwrap();
        let m = CastMessage::decode(&data).unwrap();
        assert_eq!(m.protocol_version, 3);
        assert_eq!(
            m.payload,
            Payload::String(r#"{"protocolVersion":3,"type":"CONNECTED"}"#.into())
        );
    }
}
