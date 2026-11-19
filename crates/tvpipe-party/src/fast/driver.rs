//! Runs a [`FastPath`] on threads: one blocking UDP socket per local
//! address, each with a reader thread, and the driver thread that owns
//! the stack. Readers and the party send into one channel, so a command
//! wakes the driver as a datagram does; the driver sleeps until the next
//! of those or the stack's next timeout.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Channel, FastEvent, FastPath, PeerId};
use crate::conn::Inbound;

/// Datagrams and commands waiting for the driver at most; datagrams past
/// it are dropped (as the network would).
const QUEUE: usize = 4096;
/// How often idle readers look whether to stop.
const READ_POLL: Duration = Duration::from_millis(250);
/// The longest datagram read (more than any MTU).
const MAX_DATAGRAM: usize = 2048;

/// What the party asks the driver.
pub(crate) enum Cmd {
    /// A phone's offer: answer it ([`Up::Answer`]).
    Offer {
        peer: PeerId,
        sdp: String,
    },
    Send {
        peer: PeerId,
        channel: Channel,
        text: String,
    },
    Close {
        peer: PeerId,
    },
    Datagram {
        local: SocketAddr,
        from: SocketAddr,
        data: Vec<u8>,
        at: Instant,
    },
}

/// What the driver tells the party (inside [`Inbound::Fast`]).
#[derive(Debug)]
pub(crate) enum Up {
    Answer {
        peer: PeerId,
        sdp: Result<String, String>,
    },
    Open(PeerId),
    Message {
        peer: PeerId,
        channel: Channel,
        text: String,
        at: Instant,
    },
    Down {
        peer: PeerId,
        why: String,
    },
}

/// The fast path's threads, until dropped.
pub(crate) struct Driver {
    tx: SyncSender<Cmd>,
    stop: Arc<AtomicBool>,
    locals: Vec<SocketAddr>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver")
            .field("locals", &self.locals)
            .finish_non_exhaustive()
    }
}

/// Sends commands to the driver (cheap to clone).
#[derive(Clone, Debug)]
pub(crate) struct Link(SyncSender<Cmd>);

impl Link {
    /// Queues a command; false if the driver is gone.
    pub(crate) fn send(&self, cmd: Cmd) -> bool {
        self.0.send(cmd).is_ok()
    }
}

impl Driver {
    /// Binds `addrs` (port 0: any) and starts the threads; `make` builds
    /// the stack for the bound addresses. Addresses that cannot be bound
    /// are skipped; none bound is an error.
    pub(crate) fn start<F: FastPath + 'static>(
        addrs: &[SocketAddr],
        make: impl FnOnce(Vec<SocketAddr>) -> Result<F, String>,
        up: mpsc::Sender<Inbound>,
    ) -> Result<Driver, String> {
        let mut sockets = Vec::new();
        let mut last = None;
        for a in addrs {
            match UdpSocket::bind(a) {
                Ok(s) => sockets.push(s),
                Err(e) => last = Some(format!("{a}: {e}")),
            }
        }
        if sockets.is_empty() {
            return Err(last.unwrap_or_else(|| "no address".into()));
        }
        let locals = sockets
            .iter()
            .map(|s| s.local_addr().map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let fast = make(locals.clone())?;
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut senders = Vec::new();
        for (s, &local) in sockets.into_iter().zip(&locals) {
            let send = s.try_clone().map_err(|e| e.to_string())?;
            senders.push((local, send));
            s.set_read_timeout(Some(READ_POLL))
                .map_err(|e| e.to_string())?;
            let (tx, stop) = (tx.clone(), stop.clone());
            let t = std::thread::Builder::new()
                .name("party-udp".into())
                .spawn(move || read_loop(s, local, tx, &stop))
                .map_err(|e| e.to_string())?;
            threads.push(t);
        }
        let stop2 = stop.clone();
        let t = std::thread::Builder::new()
            .name("party-fast".into())
            .spawn(move || drive(fast, rx, senders, up, &stop2))
            .map_err(|e| e.to_string())?;
        threads.push(t);
        Ok(Driver {
            tx,
            stop,
            locals,
            threads,
        })
    }

    pub(crate) fn link(&self) -> Link {
        Link(self.tx.clone())
    }

    /// The bound addresses (the host candidates).
    pub(crate) fn locals(&self) -> &[SocketAddr] {
        &self.locals
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the driver; readers notice within READ_POLL.
        let _ = self.tx.try_send(Cmd::Close { peer: PeerId::MAX });
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn read_loop(s: UdpSocket, local: SocketAddr, tx: SyncSender<Cmd>, stop: &AtomicBool) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    while !stop.load(Ordering::Relaxed) {
        // Errors are timeouts, and on Windows ICMP "port unreachable"
        // from a phone that left: keep reading.
        if let Ok((n, from)) = s.recv_from(&mut buf) {
            let cmd = Cmd::Datagram {
                local,
                from,
                data: buf[..n].to_vec(),
                at: Instant::now(),
            };
            if let Err(TrySendError::Disconnected(_)) = tx.try_send(cmd) {
                return;
            }
        }
    }
}

fn drive(
    mut fast: impl FastPath,
    rx: mpsc::Receiver<Cmd>,
    sockets: Vec<(SocketAddr, UdpSocket)>,
    up: mpsc::Sender<Inbound>,
    stop: &AtomicBool,
) {
    let tell = |u: Up| up.send(Inbound::Fast(u)).is_ok();
    while !stop.load(Ordering::Relaxed) {
        let wait = fast.next_timeout().map_or(Duration::from_secs(1), |t| {
            t.saturating_duration_since(Instant::now())
        });
        let cmd = match rx.recv_timeout(wait) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let now = Instant::now();
        match cmd {
            Some(Cmd::Offer { peer, sdp }) => {
                let sdp = fast.answer(peer, &sdp, now);
                if !tell(Up::Answer { peer, sdp }) {
                    return;
                }
            }
            Some(Cmd::Send {
                peer,
                channel,
                text,
            }) => {
                fast.send(peer, channel, &text, now);
            }
            Some(Cmd::Close { peer }) => fast.close(peer, now),
            Some(Cmd::Datagram {
                local,
                from,
                data,
                at,
            }) => fast.datagram(local, from, &data, at),
            None => {}
        }
        fast.timeout(Instant::now());
        while let Some(t) = fast.poll_transmit() {
            if let Some((_, s)) = sockets.iter().find(|(l, _)| *l == t.local) {
                // Lost like any datagram if the network refuses it.
                let _ = s.send_to(&t.data, t.to);
            }
        }
        while let Some(e) = fast.poll_event() {
            let u = match e {
                FastEvent::Open(peer) => Up::Open(peer),
                FastEvent::Message {
                    peer,
                    channel,
                    text,
                } => Up::Message {
                    peer,
                    channel,
                    text,
                    at: now,
                },
                FastEvent::Down { peer, why } => Up::Down { peer, why },
            };
            if !tell(u) {
                return;
            }
        }
    }
}
