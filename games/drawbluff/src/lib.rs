//! drawbluff, a Drawful-style party game, hosted in Rust: the reference
//! game of tvpipe's multi-screen platform (docs/multiscreen-design.md §4).
//! The same rules, prompts and phone UI as the Node version in this
//! directory (`game/`, `main.ts`), which is the protocol reference.

pub mod rules;

/// The prompts to draw (`data/prompts.txt`, shared with the Node version;
/// also the source of decoy lies).
pub fn prompts() -> Vec<String> {
    include_str!("../data/prompts.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}
