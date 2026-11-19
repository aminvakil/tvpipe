//! What one session (or one command) runs with, instead of process-wide
//! state: its stop request. A program running several sessions gives
//! each its own.

use crate::util::stop::Stop;

/// The per-session context, passed down from the API (or the CLI).
#[derive(Clone, Default, Debug)]
pub struct Ctx {
    /// Requested by the program (or Ctrl-C in the CLI); every blocking
    /// wait of the session polls it.
    pub stop: Stop,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx::default()
    }
}
