//! The test vector files (tvpipe's format: blocks of `key = value` lines
//! separated by blank lines, `#` comments, continuation lines indented).

use std::path::Path;

pub(crate) struct Block {
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

    pub fn str(&self, key: &str) -> &str {
        self.get(key)
            .unwrap_or_else(|| panic!("vector block at line {}: missing {key:?}", self.line))
    }
}

pub(crate) fn parse(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            open = false;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with(char::is_whitespace) && open {
            if let Some((_, v)) = blocks.last_mut().and_then(|b| b.entries.last_mut()) {
                v.push_str(line.trim());
            }
            continue;
        }
        let (k, v) = line
            .split_once(" = ")
            .unwrap_or_else(|| panic!("vectors line {}: expected `key = value`", i + 1));
        if !open {
            blocks.push(Block {
                line: i + 1,
                entries: Vec::new(),
            });
            open = true;
        }
        if let Some(b) = blocks.last_mut() {
            b.entries.push((k.trim().to_owned(), v.to_owned()));
        }
    }
    blocks
}

/// `tests/vectors/NAME`.
pub(crate) fn vectors(name: &str) -> Vec<Block> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(name);
    parse(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}
