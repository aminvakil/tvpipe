//! What one session (or one command) runs with, instead of process-wide
//! state: its stop request and where its events go. A program running
//! several sessions gives each its own.

use crate::events::Events;
use crate::util::stop::Stop;

/// The per-session context, passed down from the API (or the CLI).
#[derive(Clone, Default, Debug)]
pub struct Ctx {
    /// Requested by the program (or Ctrl-C in the CLI); every blocking
    /// wait of the session polls it.
    pub stop: Stop,
    /// Where the session's events go (nowhere by default).
    pub events: Events,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx::default()
    }
}

impl From<Stop> for Ctx {
    fn from(stop: Stop) -> Ctx {
        Ctx {
            stop,
            ..Ctx::default()
        }
    }
}
