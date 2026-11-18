//! Mirroring time bases (spec §7): 32.32 fixed-point seconds, the PTP
//! mapping anchored on the receiver's response headers, the NTP-mode
//! monotonic clock, and the NTP timing responder.
//!
//! PTP needs no PTP stack: the receiver's
//! `X-Apple-RequestReceivedTimestamp` (+ `X-Apple-ProcessingTime`) is on
//! the same base as its PTP timeline, so a response read at local time
//! `t0` anchors `R(t) = R0 + (t − t0)`. Every `/feedback` response
//! re-anchors it, never backwards.
//!
//! Sources: docs/spec/airplay-mirroring.md §3, §7.1–§7.4; vectors
//! tests/vectors/airplay-mirror/audio.txt (sync packet inputs),
//! appletv-session.txt.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::net::http::Headers;

/// Seconds between 1900 (NTP) and 1970.
pub const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;
/// The same offset in 32.32.
pub const NTP_EPOCH_OFFSET_FP: u64 = NTP_EPOCH_OFFSET << 32;

/// A duration as unsigned 32.32 seconds (fraction floored).
pub fn fp_from_duration(d: Duration) -> u64 {
    (d.as_secs() << 32) | (((d.subsec_nanos() as u64) << 32) / 1_000_000_000)
}

/// Milliseconds as 32.32 seconds.
pub fn fp_from_ms(ms: u64) -> u64 {
    fp_from_duration(Duration::from_millis(ms))
}

/// 32.32 seconds as nanoseconds (fraction floored).
pub fn fp_to_ns(fp: u64) -> u64 {
    (fp >> 32) * 1_000_000_000 + (((fp & 0xffff_ffff) * 1_000_000_000) >> 32)
}

/// 32.32 seconds as a duration.
pub fn fp_to_duration(fp: u64) -> Duration {
    Duration::from_nanos(fp_to_ns(fp))
}

/// `X-Apple-RequestReceivedTimestamp + X-Apple-ProcessingTime` (ms) as
/// 32.32 seconds; `None` if either header is missing or unparsable.
pub fn anchor_from_headers(headers: &Headers) -> Option<u64> {
    let ms = |name: &str| headers.get(name)?.trim().parse::<u64>().ok();
    let received = ms("X-Apple-RequestReceivedTimestamp")?;
    let processing = ms("X-Apple-ProcessingTime")?;
    Some(fp_from_ms(received.checked_add(processing)?))
}

#[derive(Debug, Clone, Copy)]
struct Anchor {
    local: Instant,
    remote: u64,
}

/// The receiver's PTP timeline seen through response headers (§7.2).
#[derive(Debug)]
pub struct PtpClock {
    anchor: Mutex<Anchor>,
    timeline: AtomicU64,
}

impl PtpClock {
    /// `remote` (32.32) was the receiver's time at local `local`.
    pub fn new(local: Instant, remote: u64, timeline: u64) -> PtpClock {
        PtpClock {
            anchor: Mutex::new(Anchor { local, remote }),
            timeline: AtomicU64::new(timeline),
        }
    }

    fn get(&self) -> Anchor {
        *self.anchor.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `R(t)` in 32.32 seconds.
    pub fn at(&self, t: Instant) -> u64 {
        project(self.get(), t)
    }

    /// Re-anchors on a later response: `(t1, max(R1, R(t1)))`.
    pub fn reanchor(&self, t1: Instant, r1: u64) {
        let mut a = self.anchor.lock().unwrap_or_else(|e| e.into_inner());
        let projected = project(*a, t1);
        let remote = r1.max(projected);
        trace!(
            "ptp: re-anchor, receiver {} ms ahead of the projection",
            ((r1 as i128 - projected as i128) * 1000) >> 32
        );
        *a = Anchor { local: t1, remote };
    }

    /// The timeline id (`ClockID` as u64).
    pub fn timeline(&self) -> u64 {
        self.timeline.load(Ordering::Relaxed)
    }

    pub fn set_timeline(&self, id: u64) {
        self.timeline.store(id, Ordering::Relaxed);
    }
}

fn project(a: Anchor, t: Instant) -> u64 {
    if t >= a.local {
        a.remote.wrapping_add(fp_from_duration(t - a.local))
    } else {
        a.remote.saturating_sub(fp_from_duration(a.local - t))
    }
}

/// The NTP-mode sender clock `C` (§7.3): monotonic, origin at the first
/// use in this process.
pub fn ntp_clock(t: Instant) -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    fp_from_duration(t.saturating_duration_since(origin))
}

/// Which clock a session runs on.
#[derive(Debug, Clone)]
pub enum Timeline {
    Ptp(Arc<PtpClock>),
    Ntp,
}

impl Timeline {
    /// The session timeline at `t` (32.32): `R(t)` or `C(t)` (no epoch).
    pub fn at(&self, t: Instant) -> u64 {
        match self {
            Timeline::Ptp(c) => c.at(t),
            Timeline::Ntp => ntp_clock(t),
        }
    }

    /// The video header's timeline field: the clock id, 0 for NTP.
    pub fn timeline_id(&self) -> u64 {
        match self {
            Timeline::Ptp(c) => c.timeline(),
            Timeline::Ntp => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// NTP timing

pub const NTP_PACKET_LEN: usize = 32;
const NTP_REQUEST: u8 = 0xd2;
const NTP_REPLY: u8 = 0xd3;

/// The reply to a receiver's timing request, or `None` for anything that
/// is not a request (§7.3). `recv` and `xmit` are `C + epoch`.
pub fn ntp_reply(req: &[u8], recv: u64, xmit: u64) -> Option<[u8; NTP_PACKET_LEN]> {
    if req.len() < NTP_PACKET_LEN || req[0] != 0x80 || req[1] != NTP_REQUEST {
        return None;
    }
    let mut out = [0u8; NTP_PACKET_LEN];
    out.copy_from_slice(&req[..NTP_PACKET_LEN]);
    out[1] = NTP_REPLY;
    out.copy_within(24..32, 8);
    out[16..24].copy_from_slice(&recv.to_be_bytes());
    out[24..32].copy_from_slice(&xmit.to_be_bytes());
    Some(out)
}

/// A sender-initiated timing request (§7.3, AirServer-style receivers).
pub fn ntp_request(seq: u16, xmit: u64) -> [u8; NTP_PACKET_LEN] {
    let mut out = [0u8; NTP_PACKET_LEN];
    out[0] = 0x80;
    out[1] = NTP_REQUEST;
    out[2..4].copy_from_slice(&seq.to_be_bytes());
    out[24..32].copy_from_slice(&xmit.to_be_bytes());
    out
}

/// `C(now) + epoch`, what NTP packets carry.
fn ntp_now() -> u64 {
    ntp_clock(Instant::now()).wrapping_add(NTP_EPOCH_OFFSET_FP)
}

/// Answers timing requests on the timing socket until stopped.
pub struct NtpResponder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NtpResponder {
    /// Serves `sock` (already bound and advertised); only answers `peer_ip`.
    pub fn start(sock: UdpSocket, peer_ip: std::net::IpAddr) -> Result<NtpResponder> {
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| Error::io("airplay: timing socket", e))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("airplay-ntp".into())
            .spawn(move || {
                let mut buf = [0u8; 128];
                while !flag.load(Ordering::Acquire) {
                    let (n, from) = match sock.recv_from(&mut buf) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let recv = ntp_now();
                    if from.ip() != peer_ip {
                        continue;
                    }
                    if let Some(reply) = ntp_reply(&buf[..n], recv, ntp_now()) {
                        trace!("ntp: timing request from {from}");
                        let _ = sock.send_to(&reply, from);
                    }
                }
            })
            .map_err(|e| Error::io("airplay: spawn NTP responder", e))?;
        Ok(NtpResponder {
            stop,
            thread: Some(thread),
        })
    }

    /// Sends three timing requests to `to`, 100 ms apart (§7.3).
    pub fn initiate(sock: &UdpSocket, to: SocketAddr) {
        for seq in 1..=3u16 {
            let _ = sock.send_to(&ntp_request(seq, ntp_now()), to);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for NtpResponder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point() {
        // §7.2 worked example: 1201707321 + 127 ms.
        assert_eq!(fp_from_ms(1_201_707_448), 0x0012_562b_72b0_20c4);
        assert_eq!(fp_to_ns(0x0012_562b_72b0_20c4), 1_201_707_447_999_999);
        assert_eq!(fp_to_ns(0x0012_562c_72b0_20c4), 1_201_708_447_999_999);
        assert_eq!(fp_from_duration(Duration::from_millis(1500)), 0x1_8000_0000);
        let mut h = Headers::new();
        h.add("X-Apple-RequestReceivedTimestamp", "1201707321");
        assert_eq!(anchor_from_headers(&h), None);
        h.add("X-Apple-ProcessingTime", "127");
        assert_eq!(anchor_from_headers(&h), Some(0x0012_562b_72b0_20c4));
        h.set("X-Apple-ProcessingTime", "x");
        assert_eq!(anchor_from_headers(&h), None);
    }

    #[test]
    fn ptp_mapping_never_moves_back() {
        let t0 = Instant::now();
        let r0 = fp_from_ms(1_201_707_448);
        let c = PtpClock::new(t0, r0, 7);
        let ms = |fp: u64| (fp_to_ns(fp) + 500_000) / 1_000_000;
        let t = t0 + Duration::from_millis(1500);
        assert_eq!(ms(c.at(t)), 1_201_708_948);
        // A receiver timestamp behind the projection keeps the projection.
        c.reanchor(t, fp_from_ms(1_201_708_000));
        assert_eq!(ms(c.at(t)), 1_201_708_948);
        // One ahead moves the mapping forward.
        c.reanchor(t, fp_from_ms(1_201_709_000));
        assert_eq!(ms(c.at(t)), 1_201_709_000);
        assert!(c.at(t0) < c.at(t));
        assert_eq!(c.timeline(), 7);
        c.set_timeline(9);
        assert_eq!(Timeline::Ptp(Arc::new(c)).timeline_id(), 9);
        assert_eq!(Timeline::Ntp.timeline_id(), 0);
    }

    #[test]
    fn ntp_responder_packets() {
        let mut req = [0u8; 32];
        req[0] = 0x80;
        req[1] = 0xd2;
        req[2..4].copy_from_slice(&7u16.to_be_bytes());
        req[24..32].copy_from_slice(&0x83aa_0000_1234_5678u64.to_be_bytes());
        let r = ntp_reply(&req, 0x1111, 0x2222).unwrap();
        assert_eq!(&r[..8], &[0x80, 0xd3, 0, 7, 0, 0, 0, 0]);
        assert_eq!(&r[8..16], &0x83aa_0000_1234_5678u64.to_be_bytes());
        assert_eq!(&r[16..24], &0x1111u64.to_be_bytes());
        assert_eq!(&r[24..32], &0x2222u64.to_be_bytes());
        // Replies, short packets and other types are ignored.
        assert!(ntp_reply(&r, 1, 2).is_none());
        assert!(ntp_reply(&req[..31], 1, 2).is_none());
        let q = ntp_request(2, 0xabcd);
        assert_eq!(&q[..4], &[0x80, 0xd2, 0, 2]);
        assert_eq!(&q[24..], &0xabcdu64.to_be_bytes());
    }

    #[test]
    fn ntp_responder_answers_over_udp() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap();
        let _r = NtpResponder::start(sock, addr.ip()).unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.send_to(&ntp_request(5, 42), addr).unwrap();
        let mut buf = [0u8; 64];
        let (n, _) = client.recv_from(&mut buf).unwrap();
        assert_eq!(n, 32);
        assert_eq!(buf[1], 0xd3);
        assert_eq!(&buf[8..16], &42u64.to_be_bytes());
        let recv = u64::from_be_bytes(buf[16..24].try_into().unwrap());
        assert!(recv >> 32 >= NTP_EPOCH_OFFSET);
    }
}
