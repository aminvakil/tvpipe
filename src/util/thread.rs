//! Joining threads that may be stuck on a socket.
//!
//! Closing a connection used to end a thread blocked reading it:
//! `shutdown(Both)` from another thread wakes a blocked `recv` on Linux
//! and macOS. On Windows it does not (checked on Windows 11: the read
//! returns only at its timeout or when the peer closes), so a plain
//! `join` after the shutdown waits for a peer that may never answer.
//! [`join_until`] joins with a deadline instead and leaves a stuck thread
//! behind; it ends on its own read timeout, or with the process.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long [`join_until`] sleeps between checks.
const POLL: Duration = Duration::from_millis(5);

/// Joins `handle` if the thread ends by `deadline`; otherwise detaches it
/// and returns false.
pub fn join_until<T>(handle: JoinHandle<T>, deadline: Instant) -> bool {
    while !handle.is_finished() {
        let now = Instant::now();
        if now >= deadline {
            debug!(
                "thread {} still running at its join deadline; leaving it",
                handle.thread().name().unwrap_or("?")
            );
            return false;
        }
        std::thread::sleep(POLL.min(deadline - now));
    }
    let _ = handle.join();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn joins_a_finished_thread() {
        let h = std::thread::spawn(|| {});
        assert!(join_until(h, Instant::now() + Duration::from_secs(5)));
    }

    #[test]
    fn leaves_a_stuck_thread_at_the_deadline() {
        let (tx, rx) = mpsc::channel::<()>();
        let h = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        let t = Instant::now();
        assert!(!join_until(h, t + Duration::from_millis(50)));
        assert!(t.elapsed() < Duration::from_secs(2));
        drop(tx);
    }

    /// The reason for this module: a blocked read ends at once after a
    /// local shutdown on Linux and macOS, not on Windows; with a deadline
    /// the join is bounded either way.
    #[test]
    fn a_reader_of_a_silent_peer_is_bounded() {
        use std::io::Read;
        use std::net::{Shutdown, TcpListener, TcpStream};
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (_peer, _) = l.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut r = c.try_clone().unwrap();
        let h = std::thread::spawn(move || {
            let _ = r.read(&mut [0; 8]);
        });
        std::thread::sleep(Duration::from_millis(50));
        let _ = c.shutdown(Shutdown::Both);
        let t = Instant::now();
        let _ = join_until(h, t + Duration::from_millis(500));
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
