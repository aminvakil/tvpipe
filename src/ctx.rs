//! What one session (or one command) runs with, instead of process-wide
//! state: its stop request, where its events go and where pairings are
//! kept. A program running
//! several sessions gives each its own.

use crate::events::Events;
use crate::storage::Storage;
use crate::util::stop::Stop;

/// The per-session context, passed down from the API (or the CLI).
#[derive(Clone, Debug)]
pub struct Ctx {
    /// Requested by the program (or Ctrl-C in the CLI); every blocking
    /// wait of the session polls it.
    pub stop: Stop,
    /// Where the session's events go (nowhere by default).
    pub events: Events,
    /// Pairings and the player token.
    pub storage: Storage,
}

impl Ctx {
    /// The CLI's storage, no events, no stop yet.
    pub fn new() -> Ctx {
        Ctx {
            stop: Stop::new(),
            events: Events::none(),
            storage: Storage::cli(),
        }
    }
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx::new()
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
