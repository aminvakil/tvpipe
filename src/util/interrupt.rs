//! Ctrl-C as a stop request, so `play` can send STOP/TEARDOWN before it
//! exits (PLAN §3 "Teardown").
//!
//! std cannot change how a process reacts to Ctrl-C: the default is to
//! die at once. This module and `util::timer` are the only places with
//! platform FFI outside the config-directory paths (see
//! ARCHITECTURE.md, "Platform-Specific Code"), against the C runtime std links
//! anyway; no crate.
//!
//! - unix: `signal(SIGINT/SIGTERM)` installs a handler that only stores
//!   into an atomic (async-signal-safe); a second signal calls `_exit(130)`.
//! - windows: `SetConsoleCtrlHandler`; the handler runs on its own thread,
//!   stores the flag and returns TRUE; on a second Ctrl-C it returns FALSE
//!   so the default handler ends the process.
//!
//! [`watch_stdin`] adds a std-only way to stop: `q` + Enter on a terminal.

use std::io::{BufRead, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set only by the signal/console handler, so a Ctrl-C stop exits with 130
/// while `q` + Enter exits with 0.
static SIGNALLED: AtomicBool = AtomicBool::new(false);

/// Installs the Ctrl-C handler. Returns false if the platform refused (or
/// has no handler here); Ctrl-C then kills the process as usual.
pub fn install() -> bool {
    sys::install()
}

/// Whether a stop was requested (Ctrl-C, SIGTERM, or `q` on stdin).
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

/// Whether the stop came from Ctrl-C/SIGTERM (exit status 130).
pub fn signalled() -> bool {
    SIGNALLED.load(Ordering::SeqCst)
}

/// Requests a stop from code (e.g. the stdin watcher).
pub fn request() {
    REQUESTED.store(true, Ordering::SeqCst);
}

/// If stdin is a terminal, reads it on a background thread and requests a
/// stop on a line `q` (like ffmpeg). Does nothing otherwise, so piped or
/// closed stdin never stops a session.
pub fn watch_stdin() {
    let stdin = std::io::stdin();
    // With --machine, stdin belongs to the machine interface's router.
    if !stdin.is_terminal() || crate::machine::enabled() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("stdin-quit".into())
        .spawn(move || {
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) if l.trim().eq_ignore_ascii_case("q") => {
                        request();
                        return;
                    }
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        });
    if let Err(e) = spawned {
        debug!("cannot watch stdin: {e}");
    }
}

#[cfg(unix)]
mod sys {
    use super::{REQUESTED, SIGNALLED};
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
        SIGNALLED.store(true, Ordering::SeqCst);
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
    use super::{REQUESTED, SIGNALLED};
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
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                SIGNALLED.store(true, Ordering::SeqCst);
                !REQUESTED.swap(true, Ordering::SeqCst) as i32
            }
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
        // other test; the handler itself is checked by hand (see 0004).
        assert!(install());
        assert!(install(), "installing twice is fine");
    }
}
