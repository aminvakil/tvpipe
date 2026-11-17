//! DNS-SD browsing over legacy-unicast mDNS (RFC 6762 §6.7, RFC 6763).
//!
//! We send queries to `224.0.0.251:5353` from an ephemeral port. Because
//! the source port is not 5353, responders treat us as a "legacy unicast"
//! resolver and reply directly to that port. So we never bind 5353, never
//! fight the OS responder (mDNSResponder, Avahi, Windows), and need no
//! multicast group membership.
//!
//! The browse loop sends PTR queries for each service type, repeats them
//! after 1, 2, 4, … s until the deadline, and sends SRV/TXT/A follow-ups
//! for instances whose answers did not carry everything in the additional
//! section (RFC 6763 §12). All records are kept in a [`Cache`], which
//! resolves them into [`Instance`]s.
//!
//! One socket per local address. std cannot set `IP_MULTICAST_IF`, so a
//! socket bound to a non-default interface's address relies on the OS to
//! route the query out of that interface (Linux does; other systems may
//! use the default multicast route instead).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use super::dns::{self, Message, Name, Question, RData, Record};
use crate::error::{Error, Result};
use crate::util::hex;

pub const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;

/// Largest packet we accept (RFC 6762 §17: up to 9000 bytes).
const MAX_PACKET: usize = 9000;

/// One resolved DNS-SD service instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// Full instance name, e.g. `Living Room._airplay._tcp.local`.
    pub name: Name,
    /// The service type it was found under, e.g. `_airplay._tcp.local`.
    pub service: Name,
    /// SRV target and port, once known.
    pub host: Option<Name>,
    pub port: Option<u16>,
    /// TXT strings (empty until known).
    pub txt: Vec<Vec<u8>>,
    pub has_txt: bool,
    /// Addresses of the SRV target (IPv4 first).
    pub addrs: Vec<IpAddr>,
    /// Source address of the packet that announced the instance, as a
    /// fallback when no address record arrived.
    pub source: Option<IpAddr>,
}

impl Instance {
    /// The instance label in presentation form (the human-readable name).
    pub fn label(&self) -> String {
        self.name
            .first_label()
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .unwrap_or_default()
    }

    /// SRV, TXT and at least one address are known.
    pub fn is_complete(&self) -> bool {
        self.port.is_some() && self.has_txt && !self.addrs.is_empty()
    }

    /// The best address to connect to: an IPv4 address of the host, else
    /// any address, else the packet source.
    pub fn addr(&self) -> Option<IpAddr> {
        self.addrs.first().copied().or(self.source)
    }

    /// TXT key/value pairs (keys lowercased, first occurrence wins).
    pub fn txt_pairs(&self) -> Vec<(String, Option<Vec<u8>>)> {
        dns::txt_pairs(&self.txt)
    }
}

/// Everything we heard, deduplicated.
#[derive(Debug, Default, Clone)]
pub struct Cache {
    records: Vec<Record>,
    /// Who sent the PTR for each instance.
    sources: HashMap<Name, IpAddr>,
}

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }

    /// Adds the records of a response received from `from`.
    pub fn insert(&mut self, from: IpAddr, msg: &Message) {
        if !msg.is_response() || msg.rcode() != 0 {
            return;
        }
        for rec in msg.records() {
            if let RData::Ptr(target) = &rec.data {
                self.sources.entry(target.clone()).or_insert(from);
            }
            let pos = self
                .records
                .iter()
                .position(|r| r.name == rec.name && r.rtype == rec.rtype && r.data == rec.data);
            if rec.ttl == 0 {
                // Goodbye packet (RFC 6762 §10.1).
                if let Some(pos) = pos {
                    self.records.remove(pos);
                }
                continue;
            }
            match pos {
                Some(pos) => self.records[pos] = rec.clone(),
                None => {
                    // SRV and TXT are unique records: a new one replaces the
                    // old (RFC 6762 §10.2). Address records of a host may be
                    // split across packets, and PTRs are shared, so those
                    // accumulate.
                    if matches!(rec.data, RData::Srv { .. } | RData::Txt(_)) {
                        self.records
                            .retain(|r| !(r.name == rec.name && r.rtype == rec.rtype));
                    }
                    self.records.push(rec.clone());
                }
            }
        }
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    fn find<'a>(&'a self, name: &'a Name, rtype: u16) -> impl Iterator<Item = &'a Record> + 'a {
        self.records
            .iter()
            .filter(move |r| r.rtype == rtype && r.name == *name)
    }

    /// The instances of one service type, in the order first seen.
    pub fn instances(&self, service: &Name) -> Vec<Instance> {
        let mut out: Vec<Instance> = Vec::new();
        for rec in self.find(service, dns::TYPE_PTR) {
            let RData::Ptr(name) = &rec.data else {
                continue;
            };
            if out.iter().any(|i| i.name == *name) {
                continue;
            }
            out.push(self.resolve(name, service));
        }
        out
    }

    fn resolve(&self, name: &Name, service: &Name) -> Instance {
        let mut inst = Instance {
            name: name.clone(),
            service: service.clone(),
            host: None,
            port: None,
            txt: Vec::new(),
            has_txt: false,
            addrs: Vec::new(),
            source: self.sources.get(name).copied(),
        };
        if let Some(RData::Srv { port, target, .. }) =
            self.find(name, dns::TYPE_SRV).map(|r| &r.data).next()
        {
            inst.port = Some(*port);
            inst.host = Some(target.clone());
        }
        if let Some(RData::Txt(strings)) = self.find(name, dns::TYPE_TXT).map(|r| &r.data).next() {
            inst.txt = strings.clone();
            inst.has_txt = true;
        }
        if let Some(host) = &inst.host {
            let mut v4: Vec<IpAddr> = Vec::new();
            let mut v6: Vec<IpAddr> = Vec::new();
            for r in self.records.iter().filter(|r| r.name == *host) {
                match r.data {
                    RData::A(a) => v4.push(IpAddr::V4(a)),
                    RData::Aaaa(a) => v6.push(IpAddr::V6(a)),
                    _ => {}
                }
            }
            // Prefer the address the packet came from when the host has
            // several (e.g. Wi-Fi and Ethernet).
            if let Some(src) = inst.source {
                if let Some(i) = v4.iter().position(|a| *a == src) {
                    v4.swap(0, i);
                }
            }
            inst.addrs = v4.into_iter().chain(v6).collect();
        }
        inst
    }

    /// Follow-up questions needed to complete the instances of `services`.
    pub fn follow_ups(&self, services: &[Name]) -> Vec<Question> {
        let mut qs = Vec::new();
        for svc in services {
            for inst in self.instances(svc) {
                if inst.port.is_none() {
                    qs.push(Question::new(inst.name.clone(), dns::TYPE_SRV));
                }
                if !inst.has_txt {
                    qs.push(Question::new(inst.name.clone(), dns::TYPE_TXT));
                }
                if let Some(host) = inst.host {
                    if inst.addrs.is_empty() && !qs.iter().any(|q| q.name == host) {
                        qs.push(Question::new(host, dns::TYPE_A));
                    }
                }
            }
        }
        qs
    }
}

#[derive(Debug, Clone)]
pub struct BrowseOptions {
    /// Service types, e.g. `_airplay._tcp.local`.
    pub services: Vec<Name>,
    /// Local IPv4 addresses to send from (see `iface::browse_addrs`).
    pub interfaces: Vec<Ipv4Addr>,
    pub timeout: Duration,
    /// Where queries go; [`BrowseOptions::MDNS`] except in tests.
    pub target: SocketAddrV4,
}

impl BrowseOptions {
    pub const MDNS: SocketAddrV4 = SocketAddrV4::new(MDNS_GROUP, MDNS_PORT);
}

/// Browses until the timeout, or until `done` returns true for the cache
/// (checked after every received packet).
pub fn browse(opts: &BrowseOptions, mut done: impl FnMut(&Cache) -> bool) -> Result<Cache> {
    let deadline = Instant::now() + opts.timeout;
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(SocketAddr, Vec<u8>)>();
    let mut sockets = Vec::new();
    let mut readers = Vec::new();
    for &local in &opts.interfaces {
        let sock = match open_socket(local) {
            Ok(s) => s,
            Err(e) => {
                warn!("cannot browse from {local}: {e}");
                continue;
            }
        };
        let reader = sock
            .try_clone()
            .map_err(|e| Error::io("mdns: clone socket", e))?;
        let tx = tx.clone();
        let stop = stop.clone();
        readers.push(std::thread::spawn(move || recv_loop(reader, tx, stop)));
        debug!(
            "browsing from {}",
            sock.local_addr()
                .map_or(local.to_string(), |a| a.to_string())
        );
        sockets.push(sock);
    }
    drop(tx);
    if sockets.is_empty() {
        return Err(Error::not_found("mdns: no usable interface to browse from"));
    }

    let mut cache = Cache::new();
    let mut interval = Duration::from_secs(1);
    let mut next_query = Instant::now();
    let mut asked: HashMap<(Name, u16), Instant> = HashMap::new();
    let mut id: u16 = (std::process::id() as u16).wrapping_mul(0x9e37);
    let result = loop {
        let now = Instant::now();
        if now >= deadline {
            break Ok(());
        }
        if now >= next_query {
            for svc in &opts.services {
                id = id.wrapping_add(1);
                send(
                    &sockets,
                    opts.target,
                    &Message::query(id, vec![Question::new(svc.clone(), dns::TYPE_PTR)]),
                );
            }
            next_query = now + interval;
            interval *= 2;
        }
        // Follow-ups, at most once a second per question.
        for q in cache.follow_ups(&opts.services) {
            let key = (q.name.clone(), q.qtype);
            if asked
                .get(&key)
                .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(1))
            {
                continue;
            }
            asked.insert(key, now);
            trace!("follow-up {} type {}", q.name, q.qtype);
            id = id.wrapping_add(1);
            send(&sockets, opts.target, &Message::query(id, vec![q]));
        }
        let wait = next_query
            .min(deadline)
            .saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait.max(Duration::from_millis(1))) {
            Ok((from, bytes)) => {
                trace!(
                    "mdns: {} bytes from {from}: {}",
                    bytes.len(),
                    hex::encode(&bytes)
                );
                match Message::parse(&bytes) {
                    Ok(msg) => {
                        cache.insert(from.ip(), &msg);
                        if done(&cache) {
                            break Ok(());
                        }
                    }
                    Err(e) => debug!("ignoring packet from {from}: {e}"),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(Error::closed("mdns: all receive sockets failed"))
            }
        }
    };
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        let _ = r.join();
    }
    result.map(|()| cache)
}

fn open_socket(local: Ipv4Addr) -> Result<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddrV4::new(local, 0))
        .map_err(|e| Error::io(format!("mdns: bind {local}"), e))?;
    // RFC 6762 §11: send with IP TTL 255. Failure is not fatal.
    if let Err(e) = sock.set_multicast_ttl_v4(255) {
        debug!("set multicast TTL: {e}");
    }
    sock.set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| Error::io("mdns: set read timeout", e))?;
    Ok(sock)
}

fn send(sockets: &[UdpSocket], target: SocketAddrV4, msg: &Message) {
    let bytes = match msg.to_bytes(true) {
        Ok(b) => b,
        Err(e) => {
            warn!("cannot encode query: {e}");
            return;
        }
    };
    for s in sockets {
        if let Err(e) = s.send_to(&bytes, target) {
            debug!("send from {:?}: {e}", s.local_addr().ok());
        }
    }
}

fn recv_loop(sock: UdpSocket, tx: mpsc::Sender<(SocketAddr, Vec<u8>)>, stop: Arc<AtomicBool>) {
    let mut buf = vec![0u8; MAX_PACKET];
    while !stop.load(Ordering::Relaxed) {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if tx.send((from, buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                        // Windows reports ICMP port-unreachable on UDP reads.
                        | std::io::ErrorKind::ConnectionReset
                ) => {}
            Err(e) => {
                debug!("receive failed: {e}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::dns::{Record, FLAG_AA, FLAG_QR};

    fn n(s: &str) -> Name {
        Name::parse(s).unwrap()
    }

    fn response(records: Vec<Record>) -> Message {
        Message {
            flags: FLAG_QR | FLAG_AA,
            answers: records,
            ..Message::default()
        }
    }

    const SRC: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 34));

    #[test]
    fn resolves_ptr_srv_txt_a_across_packets() {
        let svc = n("_airplay._tcp.local");
        let inst = n("TV._airplay._tcp.local");
        let host = n("tv.local");
        let mut cache = Cache::new();
        cache.insert(
            SRC,
            &response(vec![Record::new(
                svc.clone(),
                4500,
                RData::Ptr(inst.clone()),
            )]),
        );
        let i = &cache.instances(&svc)[0];
        assert!(!i.is_complete());
        assert_eq!(i.addr(), Some(SRC));
        assert_eq!(
            cache.follow_ups(std::slice::from_ref(&svc)),
            vec![
                Question::new(inst.clone(), dns::TYPE_SRV),
                Question::new(inst.clone(), dns::TYPE_TXT)
            ]
        );
        cache.insert(
            SRC,
            &response(vec![
                Record::new(
                    inst.clone(),
                    120,
                    RData::Srv {
                        priority: 0,
                        weight: 0,
                        port: 7000,
                        target: host.clone(),
                    },
                ),
                Record::new(inst.clone(), 4500, RData::Txt(vec![b"a=1".to_vec()])),
            ]),
        );
        assert_eq!(
            cache.follow_ups(std::slice::from_ref(&svc)),
            vec![Question::new(host.clone(), dns::TYPE_A)]
        );
        cache.insert(
            SRC,
            &response(vec![
                Record::new(host.clone(), 120, RData::Aaaa("fe80::1".parse().unwrap())),
                Record::new(host.clone(), 120, RData::A(Ipv4Addr::new(10, 0, 0, 9))),
                Record::new(host.clone(), 120, RData::A(Ipv4Addr::new(192, 168, 1, 34))),
            ]),
        );
        let i = &cache.instances(&svc)[0];
        assert!(i.is_complete());
        assert_eq!(i.label(), "TV");
        assert_eq!(i.port, Some(7000));
        assert_eq!(i.addr(), Some(SRC), "packet source preferred");
        assert_eq!(i.addrs.len(), 3);
        assert!(i.addrs[2].is_ipv6());
        assert_eq!(i.txt_pairs()[0].0, "a");
        assert!(cache.follow_ups(&[svc]).is_empty());
    }

    #[test]
    fn txt_update_and_goodbye() {
        let svc = n("_x._tcp.local");
        let inst = n("A._x._tcp.local");
        let mut cache = Cache::new();
        let ptr = Record::new(svc.clone(), 4500, RData::Ptr(inst.clone()));
        cache.insert(SRC, &response(vec![ptr.clone()]));
        cache.insert(
            SRC,
            &response(vec![Record::new(
                inst.clone(),
                1,
                RData::Txt(vec![b"v=1".to_vec()]),
            )]),
        );
        cache.insert(
            SRC,
            &response(vec![Record::new(
                inst.clone(),
                1,
                RData::Txt(vec![b"v=2".to_vec()]),
            )]),
        );
        assert_eq!(cache.instances(&svc)[0].txt, vec![b"v=2".to_vec()]);
        let mut bye = ptr;
        bye.ttl = 0;
        cache.insert(SRC, &response(vec![bye]));
        assert!(cache.instances(&svc).is_empty());
    }

    #[test]
    fn queries_and_errors_are_ignored() {
        let svc = n("_x._tcp.local");
        let mut msg = response(vec![Record::new(
            svc.clone(),
            1,
            RData::Ptr(n("A._x._tcp.local")),
        )]);
        msg.flags = 0;
        let mut cache = Cache::new();
        cache.insert(SRC, &msg);
        msg.flags = FLAG_QR | 3;
        cache.insert(SRC, &msg);
        assert!(cache.records().is_empty());
    }

    /// End to end over loopback: a fake responder answers the PTR query
    /// with the PTR only, then answers the follow-ups.
    #[test]
    fn browse_against_fake_responder() {
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        responder
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let target = match responder.local_addr().unwrap() {
            SocketAddr::V4(a) => a,
            SocketAddr::V6(_) => unreachable!(),
        };
        let svc = n("_streamlib-test._tcp.local");
        let inst = n("Fake TV._streamlib-test._tcp.local");
        let host = n("fake-tv.local");
        let thread = {
            let (inst, host) = (inst.clone(), host.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 9000];
                let mut seen = Vec::new();
                while let Ok((len, from)) = responder.recv_from(&mut buf) {
                    let q = Message::parse(&buf[..len]).unwrap();
                    let question = q.questions[0].clone();
                    seen.push(question.qtype);
                    let data = match question.qtype {
                        dns::TYPE_PTR => RData::Ptr(inst.clone()),
                        dns::TYPE_SRV => RData::Srv {
                            priority: 0,
                            weight: 0,
                            port: 8009,
                            target: host.clone(),
                        },
                        dns::TYPE_TXT => RData::Txt(vec![b"fn=Fake TV".to_vec()]),
                        dns::TYPE_A => RData::A(Ipv4Addr::new(127, 0, 0, 1)),
                        _ => continue,
                    };
                    let mut reply = response(vec![Record::new(question.name.clone(), 10, data)]);
                    reply.id = q.id;
                    reply.questions = vec![question];
                    responder
                        .send_to(&reply.to_bytes(true).unwrap(), from)
                        .unwrap();
                    if seen.contains(&dns::TYPE_A) {
                        break;
                    }
                }
                seen
            })
        };
        let opts = BrowseOptions {
            services: vec![svc.clone()],
            interfaces: vec![Ipv4Addr::LOCALHOST],
            timeout: Duration::from_secs(5),
            target,
        };
        let start = Instant::now();
        let complete = |c: &Cache| {
            let found = c.instances(&svc);
            !found.is_empty() && found.iter().all(Instance::is_complete)
        };
        let cache = browse(&opts, complete).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "done callback stops early"
        );
        let seen = thread.join().unwrap();
        assert_eq!(
            seen,
            [dns::TYPE_PTR, dns::TYPE_SRV, dns::TYPE_TXT, dns::TYPE_A]
        );
        let i = &cache.instances(&svc)[0];
        assert_eq!(i.label(), "Fake TV");
        assert_eq!(i.port, Some(8009));
        assert_eq!(i.addr(), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }
}
