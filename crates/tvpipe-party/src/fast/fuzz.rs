//! The fuzz gate of the fast path (docs/multiscreen-design.md §2.3): the
//! host must not panic on any datagram or offer, in any state, since a
//! game built with `panic = "abort"` would die from one malformed packet.
//!
//! Deterministic smoke runs here (`TVPIPE_FUZZ_ITERS` raises the count).
//! Seeds are the datagrams and offer of a real session between the host
//! and an `rtc` offerer; mutations reach the STUN and DTLS parsers before
//! any authentication, and garbage after the handshake reaches the
//! record layer. Every panic is collected (with `catch_unwind`) so one
//! run lists them all.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::tests::{Dir, Net, Phone};
use super::{Channel, FastEvent, FastPath};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }

    /// 1..=6 edits: flips, boundary bytes, inserts, deletes, truncation,
    /// lengths set to extremes.
    fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut m = seed.to_vec();
        for _ in 0..1 + self.below(6) {
            if m.is_empty() {
                m.push(self.next() as u8);
            }
            let i = self.below(m.len());
            match self.below(7) {
                0 => m[i] ^= 1 << self.below(8),
                1 => m[i] = [0, 1, 0x7f, 0x80, 0xff][self.below(5)],
                2 => m[i] = self.next() as u8,
                3 => m.insert(i, self.next() as u8),
                4 => {
                    m.remove(i);
                }
                5 => m.truncate(i),
                _ => {
                    // A 16-bit length field gone wild.
                    if i + 1 < m.len() {
                        let v = [0u16, 1, 0x7fff, 0xffff][self.below(4)].to_be_bytes();
                        m[i] = v[0];
                        m[i + 1] = v[1];
                    }
                }
            }
        }
        m
    }
}

/// A clean session: what each side sent, and the phone's offer.
fn capture() -> (Vec<Vec<u8>>, Vec<Vec<u8>>, String) {
    let mut net = Net::new();
    let (_, offer) = Phone::new("192.168.1.200:40200".parse().unwrap(), net.now);
    net.join(1);
    assert!(net.run_until(Duration::from_secs(10), |n| n
        .events
        .contains(&FastEvent::Open(1))));
    let now = net.now;
    net.phones[0].1.send(
        Channel::Reliable,
        r#"{"t":"act","id":1,"type":"x","data":{}}"#,
        now,
    );
    net.phones[0].1.send(
        Channel::Unreliable,
        r#"{"t":"in","s":1,"data":[0.5,-1]}"#,
        now,
    );
    net.host
        .send(1, Channel::Reliable, r#"{"t":"state","rev":2}"#, now);
    net.run_until(Duration::from_secs(2), |n| n.events.len() >= 3);
    let to_host = net
        .log
        .iter()
        .filter(|l| l.0 == Dir::ToHost)
        .map(|l| l.2.clone())
        .collect();
    let to_phone = net
        .log
        .iter()
        .filter(|l| l.0 == Dir::ToPhone)
        .map(|l| l.2.clone())
        .collect();
    (to_host, to_phone, offer)
}

fn iterations(default: usize) -> usize {
    std::env::var("TVPIPE_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Runs `f`, recording a panic's message and location instead of
/// unwinding further.
fn guarded(panics: &Arc<Mutex<BTreeSet<String>>>, f: impl FnOnce()) -> bool {
    let caught = catch_unwind(AssertUnwindSafe(f));
    if let Err(e) = &caught {
        let msg = e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        let at = LAST_PANIC.lock().unwrap().take().unwrap_or_default();
        panics.lock().unwrap().insert(format!("{at}: {msg}"));
    }
    caught.is_ok()
}

static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

fn quiet_panics() -> std::sync::MutexGuard<'static, ()> {
    let guard = super::tests::PANIC_HOOK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map(|l| l.to_string()).unwrap_or_default();
        *LAST_PANIC.lock().unwrap() = Some(at);
    }));
    guard
}

fn report(panics: &Arc<Mutex<BTreeSet<String>>>) {
    let _ = std::panic::take_hook();
    let p = panics.lock().unwrap();
    assert!(
        p.is_empty(),
        "{} distinct panics:\n{}",
        p.len(),
        p.iter().cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Garbage and mutated datagrams into a host with one phone, before
/// any packet, and after the channels opened.
#[test]
fn fuzz_datagrams() {
    let (to_host, to_phone, _) = capture();
    let seeds: Vec<Vec<u8>> = to_host.iter().chain(&to_phone).cloned().collect();
    let panics = Arc::new(Mutex::new(BTreeSet::new()));
    let _hook = quiet_panics();
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let n = iterations(3000);
    for open in [false, true] {
        let mut net: Option<Net> = None;
        for i in 0..n {
            let fresh = net.as_ref().is_none_or(|n| n.host.peers() == 0);
            if fresh {
                let ok = guarded(&panics, || {
                    let mut n = Net::new();
                    n.join(1);
                    if open {
                        assert!(n.run_until(Duration::from_secs(10), |n| n
                            .events
                            .contains(&FastEvent::Open(1))));
                    }
                    net = Some(n);
                });
                if !ok {
                    net = None;
                    continue;
                }
            }
            let Some(n) = net.as_mut() else { continue };
            let phone = n.phones[0].1.addr;
            let host = n.host_addr;
            let data = match rng.below(4) {
                0 => {
                    let len = rng.below(1500);
                    rng.bytes(len)
                }
                1 => {
                    // Typed garbage: STUN, DTLS, then random.
                    let len = 4 + rng.below(200);
                    let mut d = rng.bytes(len);
                    d[0] = [0, 1, 20, 21, 22, 23, 128][rng.below(7)];
                    if d.len() > 8 && rng.below(2) == 0 {
                        d[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
                    }
                    d
                }
                _ => {
                    let k = rng.below(seeds.len());
                    rng.mutate(&seeds[k])
                }
            };
            let from: SocketAddr = if rng.below(8) == 0 {
                format!("10.0.0.{}:{}", rng.below(255), 1 + rng.below(65000))
                    .parse()
                    .unwrap()
            } else {
                phone
            };
            let ok = guarded(&panics, || {
                n.host.datagram(host, from, &data, n.now);
                if i % 16 == 0 {
                    n.step(Duration::from_millis(rng.below(3000) as u64));
                }
                while n.host.poll_transmit().is_some() {}
                while n.host.poll_event().is_some() {}
            });
            if !ok {
                net = None;
            }
        }
    }
    report(&panics);
}

/// A handshake where the phone's datagrams are sometimes mutated or
/// repeated: the host's parsers see corrupt input in every state.
#[test]
fn fuzz_handshakes() {
    let panics = Arc::new(Mutex::new(BTreeSet::new()));
    let _hook = quiet_panics();
    let n = iterations(3000) / 30;
    let rng = Arc::new(Mutex::new(Rng(0xfeed_0042_0042_0042)));
    for _ in 0..n {
        guarded(&panics, || {
            let mut net = Net::new();
            net.join(1);
            let r = rng.clone();
            net.tamper = Box::new(move |dir, d| {
                let mut r = r.lock().unwrap();
                if dir == Dir::ToHost && r.below(4) == 0 {
                    *d = r.mutate(d);
                }
            });
            net.run_until(Duration::from_secs(20), |n| {
                n.host.peers() == 0 || n.events.contains(&FastEvent::Open(1))
            });
            let now = net.now;
            if net.host.peers() > 0 {
                net.phones[0].1.send(Channel::Reliable, "{}", now);
                net.run_until(Duration::from_secs(2), |_| false);
            }
        });
    }
    report(&panics);
}

/// Mutated and random offers.
#[test]
fn fuzz_offers() {
    let (_, _, offer) = capture();
    let panics = Arc::new(Mutex::new(BTreeSet::new()));
    let _hook = quiet_panics();
    let mut rng = Rng(0x0ffe_0ffe_0ffe_0ffe);
    let mut host = super::rtc::RtcPath::new(super::rtc::RtcOptions::new(vec![super::tests::HOST
        .parse()
        .unwrap()]))
    .unwrap();
    let now = std::time::Instant::now();
    let lines: Vec<&str> = offer.split("\r\n").collect();
    for i in 0..iterations(3000) / 3 {
        let text = match rng.below(3) {
            // Line-level: drop, repeat or garble a line.
            0 => {
                let mut l: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
                let at = rng.below(l.len());
                match rng.below(3) {
                    0 => {
                        l.remove(at);
                    }
                    1 => {
                        let dup = l[at].clone();
                        l.insert(at, dup);
                    }
                    _ => {
                        let m = rng.mutate(l[at].as_bytes());
                        l[at] = String::from_utf8_lossy(&m).into_owned();
                    }
                }
                l.join("\r\n")
            }
            1 => String::from_utf8_lossy(&rng.mutate(offer.as_bytes())).into_owned(),
            _ => {
                let len = rng.below(400);
                String::from_utf8_lossy(&rng.bytes(len)).into_owned()
            }
        };
        let ok = guarded(&panics, || {
            let _ = host.answer(i as u64 % 4, &text, now);
            while host.poll_transmit().is_some() {}
            while host.poll_event().is_some() {}
        });
        if !ok {
            host = super::rtc::RtcPath::new(super::rtc::RtcOptions::new(vec![super::tests::HOST
                .parse()
                .unwrap()]))
            .unwrap();
        }
    }
    report(&panics);
}
