//! A stop request shared by the threads of one session.
//!
//! Every blocking wait in a session polls (or waits on) its [`Stop`]: the
//! inputs, the fanout, the browser pages, the headless browser's keeper.
//! A program running several sessions gives each its own token; the CLI
//! requests its session's stop on Ctrl-C, `q` or the machine interface's
//! `stop` command. Cloning shares the token.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}

/// A cloneable stop request: [`request`](Stop::request) once, every clone
/// sees [`requested`](Stop::requested).
#[derive(Clone, Default)]
pub struct Stop(Arc<Inner>);

impl Stop {
    pub fn new() -> Stop {
        Stop::default()
    }

    /// Requests the stop and wakes every [`wait`](Stop::wait).
    pub fn request(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        let _guard = self.0.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.0.wake.notify_all();
    }

    /// Whether a stop was requested.
    pub fn requested(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Takes the request back (tests that share one token).
    #[cfg(test)]
    pub fn clear(&self) {
        self.0.flag.store(false, Ordering::SeqCst);
    }

    /// Sleeps up to `timeout`, returning early (with `true`) on a stop
    /// request.
    pub fn wait(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        let mut guard = self.0.lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if self.requested() {
                return true;
            }
            let now = Instant::now();
            if now >= until {
                return false;
            }
            guard = match self.0.wake.wait_timeout(guard, until - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
    }
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Stop").field(&self.requested()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_request() {
        let a = Stop::new();
        let b = a.clone();
        assert!(!b.requested());
        assert!(!b.wait(Duration::from_millis(1)));
        a.request();
        assert!(b.requested());
        assert!(b.wait(Duration::from_secs(60)));
    }

    #[test]
    fn wait_wakes_on_request() {
        let a = Stop::new();
        let b = a.clone();
        let t = std::thread::spawn(move || b.wait(Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(20));
        a.request();
        assert!(t.join().unwrap());
    }
}
