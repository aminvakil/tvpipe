//! Test vector files.
//!
//! Format (shared by every module's known-answer tests):
//!
//! ```text
//! # comment lines start with '#'
//! name = AES-128 FIPS-197 C.1
//! key = 000102030405060708090a0b0c0d0e0f
//! pt  = 00112233445566778899aabbccddeeff
//! ct  = 69c4e0d86a7b0430d8cdb78070b4c55a
//!
//! name = next block ...
//! ```
//!
//! Blocks are separated by blank lines. Each line is `key = value`. A value
//! may continue on following lines that start with whitespace (useful for
//! long hex); continuation lines are appended with the leading whitespace
//! removed. Values are interpreted by the caller: [`Block::hex`] decodes hex
//! (whitespace ignored), [`Block::str`] returns the raw text.
//!
//! Files live under `tests/vectors/<module>/*.txt`.

#![cfg(test)]

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Block {
    /// 1-based line number where the block starts, for error messages.
    pub line: usize,
    pub entries: Vec<(String, String)>,
}

impl Block {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The raw value; panics with the block's line number if missing.
    pub fn str(&self, key: &str) -> &str {
        self.get(key)
            .unwrap_or_else(|| panic!("vector block at line {}: missing key {key:?}", self.line))
    }

    /// The value decoded as hex; panics if missing or invalid.
    pub fn hex(&self, key: &str) -> Vec<u8> {
        crate::util::hex::decode(self.str(key))
            .unwrap_or_else(|e| panic!("vector block at line {}: key {key:?}: {e}", self.line))
    }

    /// The value parsed with `FromStr`; panics if missing or invalid.
    pub fn parse<T: std::str::FromStr>(&self, key: &str) -> T {
        self.str(key)
            .parse()
            .unwrap_or_else(|_| panic!("vector block at line {}: bad value for {key:?}", self.line))
    }

    /// The `name` entry, or `"line N"`.
    pub fn name(&self) -> String {
        self.get("name")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line {}", self.line))
    }
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut cur = Block::default();
    for (idx, raw) in text.lines().enumerate() {
        let lineno = idx + 1;
        if raw.trim().is_empty() {
            if !cur.entries.is_empty() {
                blocks.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if raw.trim_start().starts_with('#') {
            continue;
        }
        if raw.starts_with(char::is_whitespace) {
            let (_, v) = cur
                .entries
                .last_mut()
                .unwrap_or_else(|| panic!("vectors line {lineno}: continuation without key"));
            v.push_str(raw.trim());
            continue;
        }
        let (k, v) = raw
            .split_once('=')
            .unwrap_or_else(|| panic!("vectors line {lineno}: expected 'key = value'"));
        if cur.entries.is_empty() {
            cur.line = lineno;
        }
        cur.entries.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    if !cur.entries.is_empty() {
        blocks.push(cur);
    }
    blocks
}

/// Path relative to the crate root, e.g. `path("tests/vectors/aes/fips197.txt")`.
pub fn path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Loads `tests/vectors/<rel>`.
pub fn load(rel: &str) -> Vec<Block> {
    let p = path("tests/vectors").join(rel);
    let text = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("cannot read vectors {}: {e}", p.display()));
    parse(&text)
}

/// Reads a binary fixture from `tests/fixtures/<rel>`.
pub fn fixture(rel: &str) -> Vec<u8> {
    let p = path("tests/fixtures").join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let blocks = parse("# header\nname = one\nkey = 00 01\n  02\n\n\nname = two\nv = 5\n");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].hex("key"), [0, 1, 2]);
        assert_eq!(blocks[0].line, 2);
        assert_eq!(blocks[1].name(), "two");
        assert_eq!(blocks[1].parse::<u32>("v"), 5);
    }

    #[test]
    fn sample_file() {
        let blocks = load("sample.txt");
        assert_eq!(blocks[0].hex("data"), b"tvpipe");
    }
}
