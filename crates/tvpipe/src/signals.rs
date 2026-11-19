//! Ctrl-C and SIGTERM as a stop request, so a program can end its
//! sessions cleanly: the receivers get their TEARDOWN/STOP and leave
//! mirroring mode (PLAN §3 "Teardown"). Opt-in, with the `signals`
//! feature; the library never installs a handler by itself.
//!
//! ```no_run
//! # fn main() -> tvpipe::Result<()> {
//! let stream = tvpipe::stream(tvpipe::Options::new().to("Living Room"))?;
//! // One line: Ctrl-C stops the stream (a second Ctrl-C exits at once).
//! tvpipe::signals::stop_on_signal(stream.stopper());
//! # Ok(()) }
//! ```
//!
//! std cannot change how a process reacts to Ctrl-C: the default is to
//! die at once. This module and `util::timer` are the only places with
//! platform FFI outside the config-directory paths (see ARCHITECTURE.md,
//! "Platform-Specific Code"), against the C runtime std links anyway; no
//! crate.
//!
//! - unix: `signal(SIGINT/SIGTERM)` installs a handler that only stores
//!   into an atomic (async-signal-safe); a second signal calls `_exit(130)`.
//! - windows: `SetConsoleCtrlHandler`; the handler runs on its own thread,
//!   stores the flag and returns TRUE; on a second Ctrl-C it returns FALSE
//!   so the default handler ends the process.
//!
//! A handler may only touch an atomic, so [`on_signal`] and
//! [`stop_on_signal`] poll it from a thread of their own (every 20 ms).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::Stopper;

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Installs the Ctrl-C/SIGTERM handler (again is fine). Returns false if
/// the platform refused or has none here; Ctrl-C then ends the process as
/// usual.
pub fn install() -> bool {
    sys::install()
}

/// Whether Ctrl-C or SIGTERM came (after [`install`]). A program that
/// stops for this reason usually exits with status 130.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

/// Installs the handler and runs `f` once, on a thread of its own, when
/// Ctrl-C or SIGTERM comes. Returns false (and never runs `f`) if no
/// handler could be installed or the thread could not start.
pub fn on_signal(f: impl FnOnce() + Send + 'static) -> bool {
    if !install() {
        return false;
    }
    std::thread::Builder::new()
        .name("tvpipe-signals".into())
        .spawn(move || {
            while !requested() {
                std::thread::sleep(Duration::from_millis(20));
            }
            f();
        })
        .is_ok()
}

/// Stops a session or stream on Ctrl-C/SIGTERM ([`on_signal`] with
/// [`Stopper::stop`]).
pub fn stop_on_signal(stopper: Stopper) -> bool {
    on_signal(move || stopper.stop())
}

#[cfg(unix)]
mod sys {
    use super::REQUESTED;
    use std::sync::atomic::Ordering;

    // POSIX signal numbers; identical on every unix we build for.
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    /// `SIG_ERR` is `(sighandler_t)-1`.
    const SIG_ERR: usize = usize::MAX;

    extern "C" {
        // `sighandler_t signal(int, sighandler_t)`; a handler is a
        // pointer-sized function address.
        fn signal(signum: i32, handler: usize) -> usize;
        fn _exit(status: i32) -> !;
    }

    extern "C" fn on_signal(_signum: i32) {
        // Only async-signal-safe operations: a lock-free atomic swap, and
        // `_exit` (which runs no atexit handlers or destructors).
        if REQUESTED.swap(true, Ordering::SeqCst) {
            // SAFETY: `_exit` is async-signal-safe and takes any status.
            unsafe { _exit(130) }
        }
    }

    pub fn install() -> bool {
        let handler = on_signal as extern "C" fn(i32) as usize;
        // SAFETY: `signal` is the C library's; `on_signal` has the
        // `void (*)(int)` ABI it expects and only touches an atomic.
        unsafe { signal(SIGINT, handler) != SIG_ERR && signal(SIGTERM, handler) != SIG_ERR }
    }
}

#[cfg(windows)]
mod sys {
    use super::REQUESTED;
    use std::sync::atomic::Ordering;

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;

    type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: i32) -> i32;
    }

    unsafe extern "system" fn on_ctrl(event: u32) -> i32 {
        match event {
            // First press: handled (TRUE); second: not handled (FALSE), so
            // the default handler ends the process.
            CTRL_C_EVENT | CTRL_BREAK_EVENT => !REQUESTED.swap(true, Ordering::SeqCst) as i32,
            _ => 0,
        }
    }

    pub fn install() -> bool {
        // SAFETY: `on_ctrl` has the PHANDLER_ROUTINE ABI and only touches
        // an atomic; adding a handler has no other preconditions.
        unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) != 0 }
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub fn install() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs() {
        // Raising a signal here would set the process-wide flag for every
        // other test; the handler itself is checked by hand (see 0004) and
        // by drawbluff's Ctrl-C test.
        assert!(install());
        assert!(install(), "installing twice is fine");
        assert!(!requested());
    }
}
