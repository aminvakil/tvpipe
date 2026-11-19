//! Stop requests from the person at the terminal, so `play` can send
//! STOP/TEARDOWN before it exits (PLAN §3 "Teardown").
//!
//! Ctrl-C and SIGTERM come through the library's `tvpipe::signals` (the
//! `signals` feature; the platform FFI lives there, see ARCHITECTURE.md,
//! "Platform-Specific Code"). [`watch_stdin`] adds a std-only way to stop:
//! `q` + Enter on a terminal. Other lines answer [`ask`] (the
//! `share --tunnel` confirmation).
//!
//! The library never reads stdin; this module is the CLI's.

use std::io::{BufRead, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use tvpipe::__cli::Stop;

/// The stdin watcher runs (stdin is a terminal).
static WATCHING: AtomicBool = AtomicBool::new(false);
/// Where the watcher sends the next line that is not `q`, if [`ask`]
/// waits for one.
static ANSWER: Mutex<Option<mpsc::Sender<String>>> = Mutex::new(None);

/// Installs the Ctrl-C/SIGTERM handler and forwards it to `stop`.
/// Returns false if the platform refused; Ctrl-C then kills the process
/// as usual.
pub fn link(stop: Stop) -> bool {
    tvpipe::signals::on_signal(move || stop.request())
}

/// Whether the stop came from Ctrl-C/SIGTERM (exit status 130; `q` +
/// Enter exits with 0).
pub fn signalled() -> bool {
    tvpipe::signals::requested()
}

/// Prints `question` and waits for a line on the terminal (after
/// [`watch_stdin`]). `None`: stdin is not a terminal, or `cancel` said so
/// first (polled every 100 ms).
pub fn ask(question: &str, cancel: &dyn Fn() -> bool) -> Option<String> {
    if !WATCHING.load(Ordering::SeqCst) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    *ANSWER.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    crate::ui::prompt(question);
    let answer = loop {
        if cancel() || signalled() {
            break None;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(l) => break Some(l),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        }
    };
    ANSWER.lock().unwrap_or_else(|e| e.into_inner()).take();
    crate::ui::answered(answer.is_none());
    answer
}

/// If stdin is a terminal, reads it on a background thread and requests
/// `stop` on a line `q` (like ffmpeg). Does nothing otherwise, so piped or
/// closed stdin never stops a session.
///
/// Never with `--machine`: the router owns stdin then, and on Windows
/// `is_terminal` must not run while its read is pending. Windows
/// serializes synchronous I/O on a file object, and `is_terminal` asks a
/// pipe for its name (`GetFileInformationByHandleEx`, to spot MSYS
/// terminals), which waits until the router's `ReadFile` returns: until
/// the parent writes again, possibly never (the SDK's sessions hung at
/// start on Windows runners).
pub fn watch_stdin(stop: Stop) {
    if crate::machine::router().is_some() {
        return;
    }
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("stdin-quit".into())
        .spawn(move || {
            for line in stdin.lock().lines() {
                // The terminal echoed the line: the status line moved up.
                crate::ui::typed();
                match line {
                    Ok(l) if l.trim().eq_ignore_ascii_case("q") => {
                        stop.request();
                        return;
                    }
                    Ok(l) => {
                        let tx = ANSWER.lock().unwrap_or_else(|e| e.into_inner()).take();
                        if let Some(tx) = tx {
                            let _ = tx.send(l);
                        }
                    }
                    Err(_) => return,
                }
            }
            WATCHING.store(false, Ordering::SeqCst);
        });
    if spawned.is_ok() {
        WATCHING.store(true, Ordering::SeqCst);
    }
    if let Err(e) = spawned {
        debug!("cannot watch stdin: {e}");
    }
}
