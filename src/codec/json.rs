//! Minimal JSON (RFC 8259): a [`Value`] tree, a strict parser, and a
//! compact (or, with `{:#}`, pretty) serializer.
//!
//! - Numbers keep an exact `i64` when the literal is integral and fits;
//!   otherwise they are `f64`.
//! - Objects keep insertion order and are stored as a list of pairs. When a
//!   parsed object repeats a key, lookups see the last occurrence.
//! - Lone UTF-16 surrogates in `\u` escapes decode to U+FFFD.
//! - Nesting deeper than [`MAX_DEPTH`] is rejected, so hostile input cannot
//!   exhaust the stack.
//!
//! Building messages:
//!
//! ```ignore
//! let msg = Value::object()
//!     .with("type", "LAUNCH")
//!     .with("requestId", 1)
//!     .with("appId", "0F5096E8");
//! assert_eq!(msg.to_string(), r#"{"type":"LAUNCH","requestId":1,"appId":"0F5096E8"}"#);
//! ```
//!
//! Sources: RFC 8259.

use std::fmt::{self, Write as _};

use crate::error::{Error, Result};
use crate::util::bytes::Reader;

/// Maximum nesting of arrays and objects accepted by the parser.
pub const MAX_DEPTH: usize = 128;

/// A JSON value.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Value>),
    Object(Object),
}

static NULL: Value = Value::Null;

/// 2^63, exactly representable.
const TWO_63: f64 = (1u64 << 63) as f64;

/// A JSON number: an exact `i64` when integral, else an `f64`.
#[derive(Debug, Clone, Copy)]
pub struct Number(N);

#[derive(Debug, Clone, Copy)]
enum N {
    Int(i64),
    Float(f64),
}

impl Number {
    pub fn from_i64(v: i64) -> Number {
        Number(N::Int(v))
    }

    /// Non-finite floats serialize as `null`.
    pub fn from_f64(v: f64) -> Number {
        Number(N::Float(v))
    }

    /// Whether this number was an integral literal (or built from an int).
    pub fn is_integer(&self) -> bool {
        matches!(self.0, N::Int(_))
    }

    /// The value as `i64` if it is integral and in range (an `f64` such as
    /// `3.0` qualifies).
    pub fn as_i64(&self) -> Option<i64> {
        match self.0 {
            N::Int(i) => Some(i),
            N::Float(f) => {
                // i64::MAX as f64 rounds up to 2^63, so use a strict bound.
                if f.fract() == 0.0 && (-TWO_63..TWO_63).contains(&f) {
                    Some(f as i64)
                } else {
                    None
                }
            }
        }
    }

    /// The value as `u64` if it is integral, non-negative and in range.
    pub fn as_u64(&self) -> Option<u64> {
        match self.0 {
            N::Int(i) => u64::try_from(i).ok(),
            N::Float(f) => {
                if f.fract() == 0.0 && (0.0..2.0 * TWO_63).contains(&f) {
                    Some(f as u64)
                } else {
                    None
                }
            }
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self.0 {
            N::Int(i) => i as f64,
            N::Float(f) => f,
        }
    }
}

impl PartialEq for Number {
    fn eq(&self, other: &Number) -> bool {
        match (self.0, other.0) {
            (N::Int(a), N::Int(b)) => a == b,
            _ => self.as_f64() == other.as_f64(),
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            N::Int(i) => write!(f, "{i}"),
            // `{:?}` is the shortest round-tripping form and uses exponents
            // for very large or small magnitudes (`1e21`, `1.5e-7`), all of
            // which are valid JSON.
            N::Float(v) if v.is_finite() => write!(f, "{v:?}"),
            N::Float(_) => f.write_str("null"),
        }
    }
}

/// An ordered JSON object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Object(Vec<(String, Value)>);

impl Object {
    pub fn new() -> Object {
        Object::default()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value for `key` (the last one if the key repeats).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
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
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.0.push((key, value)),
        }
    }

    /// Removes every entry for `key`, returning the last value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
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

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
}

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for Object {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Object {
        let mut o = Object::new();
        for (k, v) in iter {
            o.insert(k, v);
        }
        o
    }
}

impl Value {
    /// An empty object, for use with [`Value::with`].
    pub fn object() -> Value {
        Value::Object(Object::new())
    }

    /// An array from anything convertible.
    pub fn array<T: Into<Value>>(items: impl IntoIterator<Item = T>) -> Value {
        Value::Array(items.into_iter().map(Into::into).collect())
    }

    /// Builder: sets `key` on an object. A non-object is replaced by a new
    /// object holding just this entry.
    pub fn with(self, key: impl Into<String>, value: impl Into<Value>) -> Value {
        let mut o = match self {
            Value::Object(o) => o,
            _ => Object::new(),
        };
        o.insert(key, value);
        Value::Object(o)
    }

    /// Builder: like [`Value::with`] but skips `None`.
    pub fn with_opt<T: Into<Value>>(self, key: impl Into<String>, value: Option<T>) -> Value {
        match value {
            Some(v) => self.with(key, v),
            None => self,
        }
    }

    /// Builder: appends to an array. A non-array is replaced by a new array
    /// holding just this item.
    pub fn push(self, value: impl Into<Value>) -> Value {
        let mut a = match self {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        a.push(value.into());
        Value::Array(a)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The member `key` of an object; `None` for non-objects.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|o| o.get(key))
    }

    /// Element `idx` of an array; `None` for non-arrays.
    pub fn at(&self, idx: usize) -> Option<&Value> {
        self.as_array().and_then(|a| a.get(idx))
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(Number::as_u64)
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_u64().and_then(|v| u32::try_from(v).ok())
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().map(Number::as_f64)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The member `key`, or a protocol error naming it.
    pub fn require(&self, key: &str) -> Result<&Value> {
        self.get(key)
            .ok_or_else(|| Error::protocol(format!("json: missing field {key:?}")))
    }

    pub fn require_str(&self, key: &str) -> Result<&str> {
        self.require(key)?
            .as_str()
            .ok_or_else(|| wrong_type(key, "a string"))
    }

    pub fn require_u64(&self, key: &str) -> Result<u64> {
        self.require(key)?
            .as_u64()
            .ok_or_else(|| wrong_type(key, "a non-negative integer"))
    }

    pub fn require_u32(&self, key: &str) -> Result<u32> {
        self.require(key)?
            .as_u32()
            .ok_or_else(|| wrong_type(key, "a 32-bit unsigned integer"))
    }

    pub fn require_i64(&self, key: &str) -> Result<i64> {
        self.require(key)?
            .as_i64()
            .ok_or_else(|| wrong_type(key, "an integer"))
    }

    pub fn require_f64(&self, key: &str) -> Result<f64> {
        self.require(key)?
            .as_f64()
            .ok_or_else(|| wrong_type(key, "a number"))
    }

    pub fn require_bool(&self, key: &str) -> Result<bool> {
        self.require(key)?
            .as_bool()
            .ok_or_else(|| wrong_type(key, "a boolean"))
    }

    pub fn require_array(&self, key: &str) -> Result<&[Value]> {
        self.require(key)?
            .as_array()
            .ok_or_else(|| wrong_type(key, "an array"))
    }

    pub fn require_object(&self, key: &str) -> Result<&Object> {
        self.require(key)?
            .as_object()
            .ok_or_else(|| wrong_type(key, "an object"))
    }

    /// Parses a JSON text (shorthand for [`parse`]).
    pub fn parse(text: &str) -> Result<Value> {
        parse(text)
    }

    /// Compact serialization (same as `to_string()`).
    pub fn to_json(&self) -> String {
        self.to_string()
    }

    /// Indented serialization (two spaces), same as `format!("{:#}", v)`.
    pub fn to_json_pretty(&self) -> String {
        format!("{self:#}")
    }
}

fn wrong_type(key: &str, what: &str) -> Error {
    Error::protocol(format!("json: field {key:?} is not {what}"))
}

impl std::ops::Index<&str> for Value {
    type Output = Value;

    /// `v["key"]`: the member, or `Null` when missing or not an object.
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl std::ops::Index<usize> for Value {
    type Output = Value;

    /// `v[3]`: the element, or `Null` when out of range or not an array.
    fn index(&self, idx: usize) -> &Value {
        self.at(idx).unwrap_or(&NULL)
    }
}

macro_rules! from_int {
    ($($ty:ty),*) => {$(
        impl From<$ty> for Value {
            fn from(v: $ty) -> Value {
                Value::Number(Number::from_i64(v as i64))
            }
        }
    )*};
}

from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<u64> for Value {
    fn from(v: u64) -> Value {
        match i64::try_from(v) {
            Ok(i) => Value::Number(Number::from_i64(i)),
            Err(_) => Value::Number(Number::from_f64(v as f64)),
        }
    }
}

impl From<usize> for Value {
    fn from(v: usize) -> Value {
        Value::from(v as u64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Number(Number::from_f64(v))
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::Number(Number::from_f64(v as f64))
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::String(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::String(v)
    }
}

impl From<&String> for Value {
    fn from(v: &String) -> Value {
        Value::String(v.clone())
    }
}

impl From<Number> for Value {
    fn from(v: Number) -> Value {
        Value::Number(v)
    }
}

impl From<Object> for Value {
    fn from(v: Object) -> Value {
        Value::Object(v)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Value {
        Value::array(v)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Value {
        v.map_or(Value::Null, Into::into)
    }
}

// --- serializer ---

impl fmt::Display for Value {
    /// Compact JSON; with `{:#}`, indented by two spaces.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        write_value(&mut out, self, f.alternate().then_some(0));
        f.write_str(&out)
    }
}

fn newline(out: &mut String, indent: Option<usize>) {
    if let Some(n) = indent {
        out.push('\n');
        for _ in 0..n {
            out.push_str("  ");
        }
    }
}

fn write_value(out: &mut String, v: &Value, indent: Option<usize>) {
    let inner = indent.map(|n| n + 1);
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            let _ = write!(out, "{n}");
        }
        Value::String(s) => write_string(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, inner);
                write_value(out, item, inner);
            }
            if !a.is_empty() {
                newline(out, indent);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, item)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, inner);
                write_string(out, k);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, item, inner);
            }
            if !o.is_empty() {
                newline(out, indent);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let esc = match c {
            '"' => "\\\"",
            '\\' => "\\\\",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            '\u{08}' => "\\b",
            '\u{0c}' => "\\f",
            c if (c as u32) < 0x20 => "",
            _ => continue,
        };
        out.push_str(&s[start..i]);
        if esc.is_empty() {
            let _ = write!(out, "\\u{:04x}", c as u32);
        } else {
            out.push_str(esc);
        }
        start = i + c.len_utf8();
    }
    out.push_str(&s[start..]);
    out.push('"');
}

// --- parser ---

/// Parses a complete JSON text (surrounding whitespace allowed).
pub fn parse(text: &str) -> Result<Value> {
    let mut p = Parser {
        r: Reader::new(text.as_bytes(), "json"),
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if !p.r.is_at_end() {
        return Err(p.r.error("trailing characters after value"));
    }
    Ok(v)
}

/// Parses JSON from bytes, which must be UTF-8.
pub fn parse_bytes(data: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(data).map_err(|e| {
        Error::malformed(format!("json: invalid UTF-8 at offset {}", e.valid_up_to()))
    })?;
    parse(text)
}

struct Parser<'a> {
    r: Reader<'a>,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Ok(b' ' | b'\t' | b'\n' | b'\r') = self.r.peek_u8() {
            self.r.skip(1).ok();
        }
    }

    fn peek(&self) -> Option<u8> {
        self.r.peek_u8().ok()
    }

    fn expect_literal(&mut self, lit: &'static [u8]) -> Result<()> {
        match self.r.bytes(lit.len()) {
            Ok(b) if b == lit => Ok(()),
            _ => Err(self.r.error("invalid literal")),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        match self.peek() {
            None => Err(self.r.error("unexpected end of input")),
            Some(b'n') => self.expect_literal(b"null").map(|_| Value::Null),
            Some(b't') => self.expect_literal(b"true").map(|_| Value::Bool(true)),
            Some(b'f') => self.expect_literal(b"false").map(|_| Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(Value::Number),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(_) => Err(self.r.error("unexpected character")),
        }
    }

    fn check_depth(&self, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            Err(self.r.error("nesting too deep"))
        } else {
            Ok(())
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value> {
        self.check_depth(depth)?;
        self.r.skip(1)?; // '['
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.r.skip(1)?;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth)?);
            self.ws();
            match self.r.u8() {
                Ok(b',') => continue,
                Ok(b']') => return Ok(Value::Array(items)),
                _ => return Err(self.r.error("expected ',' or ']' in array")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value> {
        self.check_depth(depth)?;
        self.r.skip(1)?; // '{'
        let mut entries = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.r.skip(1)?;
            return Ok(Value::Object(Object(entries)));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.r.error("expected string key in object"));
            }
            let key = self.string()?;
            self.ws();
            if self.r.u8().ok() != Some(b':') {
                return Err(self.r.error("expected ':' after object key"));
            }
            self.ws();
            let value = self.value(depth)?;
            // Push without de-duplicating (linear time); `Object::get`
            // searches from the end, so the last duplicate wins.
            entries.push((key, value));
            self.ws();
            match self.r.u8() {
                Ok(b',') => continue,
                Ok(b'}') => return Ok(Value::Object(Object(entries))),
                _ => return Err(self.r.error("expected ',' or '}' in object")),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let mut n = 0;
        while let Some(b'0'..=b'9') = self.peek() {
            self.r.skip(1).ok();
            n += 1;
        }
        n
    }

    fn number(&mut self) -> Result<Number> {
        let start = self.r.position();
        let mut integral = true;
        if self.peek() == Some(b'-') {
            self.r.skip(1)?;
        }
        match self.peek() {
            Some(b'0') => {
                self.r.skip(1)?;
                if let Some(b'0'..=b'9') = self.peek() {
                    return Err(self.r.error("leading zero in number"));
                }
            }
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.r.error("expected digit")),
        }
        if self.peek() == Some(b'.') {
            self.r.skip(1)?;
            integral = false;
            if self.digits() == 0 {
                return Err(self.r.error("expected digit after decimal point"));
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.r.skip(1)?;
            integral = false;
            if let Some(b'+' | b'-') = self.peek() {
                self.r.skip(1)?;
            }
            if self.digits() == 0 {
                return Err(self.r.error("expected digit in exponent"));
            }
        }
        let bytes = &self.r.buffer()[start..self.r.position()];
        // The grammar above only admits ASCII.
        let text = std::str::from_utf8(bytes).map_err(|_| self.r.error("invalid number"))?;
        if integral {
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Number::from_i64(i));
            }
        }
        match text.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(Number::from_f64(f)),
            _ => Err(self.r.error("number out of range")),
        }
    }

    fn hex4(&mut self) -> Result<u16> {
        let mut v = 0u16;
        for _ in 0..4 {
            let d = match self.r.u8()? {
                c @ b'0'..=b'9' => c - b'0',
                c @ b'a'..=b'f' => c - b'a' + 10,
                c @ b'A'..=b'F' => c - b'A' + 10,
                _ => return Err(self.r.error("invalid \\u escape")),
            };
            v = (v << 4) | d as u16;
        }
        Ok(v)
    }

    fn string(&mut self) -> Result<String> {
        self.r.skip(1)?; // '"'
        let mut out: Vec<u8> = Vec::new();
        // A high surrogate waiting for its low half.
        let mut pending_high: Option<u16> = None;
        loop {
            let rest = self.r.rest();
            let run = rest
                .iter()
                .position(|&b| b == b'"' || b == b'\\' || b < 0x20)
                .ok_or_else(|| self.r.error("unterminated string"))?;
            if run > 0 {
                if pending_high.take().is_some() {
                    push_char(&mut out, char::REPLACEMENT_CHARACTER);
                }
                out.extend_from_slice(&rest[..run]);
                self.r.skip(run)?;
            }
            match self.r.u8()? {
                b'"' => {
                    if pending_high.take().is_some() {
                        push_char(&mut out, char::REPLACEMENT_CHARACTER);
                    }
                    break;
                }
                b'\\' => {}
                _ => return Err(self.r.error("control character in string")),
            }
            let esc = self.r.u8()?;
            if esc != b'u' && pending_high.take().is_some() {
                push_char(&mut out, char::REPLACEMENT_CHARACTER);
            }
            let c = match esc {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\u{08}',
                b'f' => '\u{0c}',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'u' => {
                    let unit = self.hex4()?;
                    match (pending_high.take(), unit) {
                        (Some(hi), 0xdc00..=0xdfff) => {
                            let cp =
                                0x10000 + (((hi as u32) - 0xd800) << 10) + ((unit as u32) - 0xdc00);
                            char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER)
                        }
                        (prev, 0xd800..=0xdbff) => {
                            if prev.is_some() {
                                push_char(&mut out, char::REPLACEMENT_CHARACTER);
                            }
                            pending_high = Some(unit);
                            continue;
                        }
                        (prev, _) => {
                            if prev.is_some() {
                                push_char(&mut out, char::REPLACEMENT_CHARACTER);
                            }
                            // Lone low surrogates are not chars: U+FFFD.
                            char::from_u32(unit as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
                        }
                    }
                }
                _ => return Err(self.r.error("invalid escape")),
            };
            push_char(&mut out, c);
        }
        // Runs are copied between ASCII delimiters of valid UTF-8 input and
        // escapes push whole chars, so this cannot fail; still, no panics.
        String::from_utf8(out).map_err(|_| self.r.error("invalid UTF-8 in string"))
    }
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::fuzz::Rng;
    use crate::util::vectors;

    fn input_of(b: &vectors::Block) -> Vec<u8> {
        match b.get("input") {
            Some(s) => s.as_bytes().to_vec(),
            None => b.hex("input_hex"),
        }
    }

    #[test]
    fn vectors() {
        let blocks = vectors::load("codec/json.txt");
        assert!(blocks.len() > 20);
        for b in &blocks {
            let name = b.name();
            let input = input_of(b);
            let res = parse_bytes(&input);
            let expected = match (b.get("output"), b.get("output_hex")) {
                (Some(s), _) => Some(s.to_owned()),
                (None, Some(_)) => Some(String::from_utf8(b.hex("output_hex")).unwrap()),
                (None, None) => None,
            };
            match expected.as_deref() {
                Some(expected) => {
                    let v = res.unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(v.to_string(), expected, "{name}");
                    // Canonical output re-parses to the same value.
                    assert_eq!(parse(expected).unwrap(), v, "{name}: reparse");
                }
                None => {
                    let err = res.expect_err(&name);
                    assert_eq!(err.kind(), crate::error::ErrorKind::Malformed, "{name}");
                    if let Some(msg) = b.get("error") {
                        assert!(err.to_string().contains(msg), "{name}: {err}");
                    }
                }
            }
        }
    }

    #[test]
    fn python_fixtures() {
        // Each line pair: json.dumps(v, ensure_ascii=True) and
        // json.dumps(v, ensure_ascii=False) of the same random value, from
        // scripts/vectors/json_fixtures.py. Both must parse to equal values,
        // and our serialization must parse back to the same value.
        let data = vectors::fixture("codec/json/python.jsonl");
        let text = std::str::from_utf8(&data).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 200);
        for pair in lines.chunks(2) {
            let a = parse(pair[0]).unwrap_or_else(|e| panic!("{}: {e}", pair[0]));
            let b = parse(pair[1]).unwrap_or_else(|e| panic!("{}: {e}", pair[1]));
            assert_eq!(a, b, "{}", pair[0]);
            assert_eq!(parse(&a.to_string()).unwrap(), a);
            assert_eq!(parse(&a.to_json_pretty()).unwrap(), a);
        }
    }

    #[test]
    fn builder_cast_messages() {
        let connect = Value::object()
            .with("type", "CONNECT")
            .with("userAgent", "streamlib");
        assert_eq!(
            connect.to_string(),
            r#"{"type":"CONNECT","userAgent":"streamlib"}"#
        );
        let launch = Value::object()
            .with("type", "LAUNCH")
            .with("requestId", 1)
            .with("appId", "0F5096E8");
        assert_eq!(
            launch.to_string(),
            r#"{"type":"LAUNCH","requestId":1,"appId":"0F5096E8"}"#
        );
        let stream = Value::object()
            .with("index", 0)
            .with("type", "video_source")
            .with("ssrc", 0xdead_beef_u32)
            .with("targetDelay", 100)
            .with("maxFrameRate", "30000/1000")
            .with(
                "resolutions",
                vec![Value::object().with("width", 1920).with("height", 1080)],
            )
            .with_opt("aesKey", None::<&str>)
            .with("aspectRatio", Some("16:9"));
        assert_eq!(
            stream.to_string(),
            concat!(
                r#"{"index":0,"type":"video_source","ssrc":3735928559,"targetDelay":100,"#,
                r#""maxFrameRate":"30000/1000","resolutions":[{"width":1920,"height":1080}],"#,
                r#""aspectRatio":"16:9"}"#
            )
        );
        let arr = Value::Null.push(1).push("two").push(Value::Null);
        assert_eq!(arr.to_string(), r#"[1,"two",null]"#);
        // insert replaces in place
        let v = Value::object().with("a", 1).with("b", 2).with("a", 3);
        assert_eq!(v.to_string(), r#"{"a":3,"b":2}"#);
    }

    #[test]
    fn receiver_status_getters() {
        let text = vectors::fixture("codec/json/receiver_status.json");
        let v = parse_bytes(&text).unwrap();
        assert_eq!(v.require_str("type").unwrap(), "RECEIVER_STATUS");
        assert_eq!(v.require_u64("requestId").unwrap(), 2);
        let app = &v["status"]["applications"][0];
        assert_eq!(app.require_str("appId").unwrap(), "0F5096E8");
        assert_eq!(app["transportId"].as_str(), Some("web-5"));
        assert_eq!(v["status"]["volume"]["level"].as_f64(), Some(0.5));
        assert_eq!(v["status"]["volume"]["muted"].as_bool(), Some(false));
        assert!(v["status"]["missing"][7]["x"].is_null());
        let ns: Vec<&str> = app
            .require_array("namespaces")
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(ns.contains(&"urn:x-cast:com.google.cast.webrtc"));
        let err = v.require_str("requestId").unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Protocol);
        assert_eq!(err.to_string(), "json: field \"requestId\" is not a string");
        assert!(v.require("nope").is_err());
    }

    #[test]
    fn numbers() {
        let v = parse("[0, -0, 9223372036854775807, -9223372036854775808, 9223372036854775808, 1.5, 3.0, 1e2, 1E-2, -2.5e+3]").unwrap();
        assert_eq!(v[0].as_i64(), Some(0));
        assert_eq!(v[2].as_i64(), Some(i64::MAX));
        assert_eq!(v[3].as_i64(), Some(i64::MIN));
        assert_eq!(v[3].as_u64(), None);
        assert!(!v[4].as_number().unwrap().is_integer());
        assert_eq!(v[4].as_i64(), None);
        assert_eq!(v[4].as_u64(), Some(1 << 63));
        assert_eq!(v[5].as_i64(), None);
        assert_eq!(v[6].as_i64(), Some(3));
        assert_eq!(v[7].as_u32(), Some(100));
        assert_eq!(v[8].as_f64(), Some(0.01));
        assert_eq!(v[9].as_f64(), Some(-2500.0));
        assert_eq!(Value::from(f64::NAN).to_string(), "null");
        assert_eq!(Value::from(u64::MAX).to_string(), "1.8446744073709552e19");
        assert_eq!(Value::from(0.1).to_string(), "0.1");
        assert_eq!(Value::from(1e21).to_string(), "1e21");
        assert_eq!(Value::from(2.0).to_string(), "2.0");
    }

    #[test]
    fn pretty() {
        let v = parse(r#"{"a":[1,{"b":null}],"c":{},"d":[]}"#).unwrap();
        assert_eq!(
            format!("{v:#}"),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": null\n    }\n  ],\n  \"c\": {},\n  \"d\": []\n}"
        );
    }

    #[test]
    fn depth_limit() {
        let ok = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_ok());
        let deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep)
            .unwrap_err()
            .to_string()
            .contains("nesting too deep"));
        let deep_obj = "{\"a\":".repeat(100_000);
        assert!(parse(&deep_obj).is_err());
    }

    #[test]
    fn duplicate_keys_last_wins() {
        let mut v = parse(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(v["a"].as_i64(), Some(3));
        let o = v.as_object_mut().unwrap();
        assert_eq!(o.remove("a").unwrap().as_i64(), Some(3));
        assert_eq!(o.len(), 1);
        assert_eq!(v.to_string(), r#"{"b":2}"#);
    }

    #[test]
    fn fuzz_smoke() {
        let mut seeds: Vec<Vec<u8>> = vectors::load("codec/json.txt")
            .iter()
            .map(input_of)
            .collect();
        seeds.push(vectors::fixture("codec/json/receiver_status.json"));
        let mut rng = Rng::new(0x6a73_6f6e);
        for _ in 0..10_000 {
            let input = rng.mutate_any(&seeds);
            if let Ok(v) = parse_bytes(&input) {
                let s = v.to_string();
                assert_eq!(parse(&s).unwrap(), v, "{s}");
            }
        }
    }
}
