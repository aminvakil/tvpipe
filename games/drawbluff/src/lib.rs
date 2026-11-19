//! drawbluff, a Drawful-style party game, hosted in Rust: the reference
//! game of tvpipe's multi-screen platform (docs/multiscreen-design.md §4).
//! The same rules, prompts and phone UI as the Node version in this
//! directory (`game/`, `main.ts`), which is the protocol reference.

pub mod bots;
pub mod game;
pub mod rules;
pub mod screen;
pub mod sound;
pub mod tv;

/// The prompts to draw, by structure (`data/prompts.txt`, shared with the
/// Node version; also the source of decoy lies).
pub fn prompts() -> Vec<rules::Prompt> {
    rules::parse_prompts(include_str!("../data/prompts.txt"))
}
