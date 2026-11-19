//! `q` and the question before stopping: while something is watched or
//! played, the first `q` asks ("Stop streaming? Press q again…"), a
//! second `q` stops, any other key or 5 s keep going. Otherwise `q` stops
//! at once. Ctrl-C never asks. No I/O and no real clock.

use std::time::Duration;

/// How long the question stays before the status line comes back.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// A key from the terminal (a line in line mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `q` (or `Q`).
    Quit,
    /// Anything else.
    Other,
}

impl Key {
    pub fn from_byte(b: u8) -> Key {
        if b.eq_ignore_ascii_case(&b'q') {
            Key::Quit
        } else {
            Key::Other
        }
    }

    pub fn from_line(l: &str) -> Key {
        if l.trim().eq_ignore_ascii_case("q") {
            Key::Quit
        } else {
            Key::Other
        }
    }
}

/// The question's state.
#[derive(Debug, Default)]
pub struct Confirm {
    /// When the question was asked.
    asked: Option<Duration>,
}

impl Confirm {
    /// A key at `now`; `watched`: something is watched or played. True:
    /// stop now.
    pub fn key(&mut self, key: Key, watched: bool, now: Duration) -> bool {
        self.expire(now);
        match key {
            Key::Quit if self.asked.is_some() || !watched => {
                self.asked = None;
                true
            }
            Key::Quit => {
                self.asked = Some(now);
                false
            }
            Key::Other => {
                self.asked = None;
                false
            }
        }
    }

    /// Drops the question after [`TIMEOUT`].
    pub fn expire(&mut self, now: Duration) {
        if self.asked.is_some_and(|t| now.saturating_sub(t) >= TIMEOUT) {
            self.asked = None;
        }
    }

    /// The question is shown.
    pub fn asking(&self) -> bool {
        self.asked.is_some()
    }

    /// Stopping anyway (Ctrl-C, the session ended): no question.
    pub fn stopped(&mut self) {
        self.asked = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: f64) -> Duration {
        Duration::from_secs_f64(secs)
    }

    #[test]
    fn q_stops_at_once_when_nobody_watches() {
        let mut c = Confirm::default();
        assert!(c.key(Key::Quit, false, s(1.0)));
        assert!(!c.asking());
    }

    #[test]
    fn q_asks_and_q_again_stops() {
        let mut c = Confirm::default();
        assert!(!c.key(Key::Quit, true, s(1.0)));
        assert!(c.asking());
        assert!(c.key(Key::Quit, true, s(2.0)));
        assert!(!c.asking());
    }

    #[test]
    fn another_key_keeps_going() {
        let mut c = Confirm::default();
        assert!(!c.key(Key::Quit, true, s(1.0)));
        assert!(!c.key(Key::Other, true, s(1.5)));
        assert!(!c.asking());
        // The next q asks again.
        assert!(!c.key(Key::Quit, true, s(2.0)));
        assert!(c.asking());
        // Keys without a question do nothing.
        let mut c = Confirm::default();
        assert!(!c.key(Key::Other, true, s(1.0)));
        assert!(!c.asking());
    }

    #[test]
    fn the_question_times_out() {
        let mut c = Confirm::default();
        assert!(!c.key(Key::Quit, true, s(1.0)));
        c.expire(s(5.9));
        assert!(c.asking());
        c.expire(s(6.0));
        assert!(!c.asking());
        // A q after the timeout asks again instead of stopping.
        let mut c = Confirm::default();
        assert!(!c.key(Key::Quit, true, s(1.0)));
        assert!(!c.key(Key::Quit, true, s(7.0)));
        assert!(c.asking());
    }

    #[test]
    fn ctrl_c_ends_the_question() {
        let mut c = Confirm::default();
        assert!(!c.key(Key::Quit, true, s(1.0)));
        c.stopped();
        assert!(!c.asking());
    }

    #[test]
    fn keys_and_lines() {
        assert_eq!(Key::from_byte(b'q'), Key::Quit);
        assert_eq!(Key::from_byte(b'Q'), Key::Quit);
        assert_eq!(Key::from_byte(b'x'), Key::Other);
        assert_eq!(Key::from_byte(0x1b), Key::Other);
        assert_eq!(Key::from_line(" q "), Key::Quit);
        assert_eq!(Key::from_line("quit"), Key::Other);
        assert_eq!(Key::from_line(""), Key::Other);
    }
}
