//! A 1 ms system timer on Windows, so timed waits keep their time.
//!
//! The Cast sender paces packets in 10 ms bursts and every receive loop
//! polls with short socket timeouts. On Windows those waits
//! (`Condvar::wait_timeout`, `SO_RCVTIMEO`, `thread::sleep`) end on the
//! system timer tick, 15.6 ms by default: measured on Windows 11 ARM64 a
//! 1 ms condvar or socket timeout took ~25 ms on average (up to ~75 ms),
//! a 10 ms `sleep` ~16-20 ms. With `timeBeginPeriod(1)` the same waits
//! take ~1-2 ms and ~10.7 ms. std has no API for it, so this is the second
//! platform FFI exception (ARCHITECTURE.md, docs/measurements.md),
//! against winmm, a system DLL; no crate. Elsewhere it does nothing:
//! Linux and macOS timers are already fine-grained.
//!
//! Since Windows 10 2004 the setting applies to the calling process only.
//! It is never undone: it ends with the process.

use std::sync::Once;

static RAISED: Once = Once::new();

/// Asks for a 1 ms timer resolution, once per process. Safe to call from
/// anywhere, any number of times.
pub fn raise_resolution() {
    RAISED.call_once(|| {
        if !sys::raise() {
            debug!("cannot raise the timer resolution; timed waits stay coarse");
        }
    });
}

#[cfg(windows)]
mod sys {
    /// `TIMERR_NOERROR`.
    const TIMERR_NOERROR: u32 = 0;

    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period_ms: u32) -> u32;
    }

    pub fn raise() -> bool {
        // SAFETY: `timeBeginPeriod` takes a plain integer, touches no
        // memory of ours and only fails with TIMERR_NOCANDO for a period
        // out of range; 1 ms is the documented minimum.
        unsafe { timeBeginPeriod(1) == TIMERR_NOERROR }
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn raise() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn short_waits_stay_short() {
        raise_resolution();
        raise_resolution();
        // A 1 ms condvar timeout, the kind of wait the Cast pacer does.
        // Without the 1 ms timer this took ~25 ms on Windows; the bound is
        // loose so a loaded machine passes.
        let m = std::sync::Mutex::new(());
        let cv = std::sync::Condvar::new();
        let mut waits: Vec<Duration> = (0..20)
            .map(|_| {
                let g = m.lock().unwrap();
                let t = Instant::now();
                let _ = cv.wait_timeout(g, Duration::from_millis(1)).unwrap();
                t.elapsed()
            })
            .collect();
        waits.sort();
        assert!(waits[10] < Duration::from_millis(12), "{waits:?}");
    }
}
