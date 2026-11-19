//! The fast path against an `rtc` offerer playing the phone, over a
//! network simulated in memory (virtual time, no sockets).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use ::rtc::data_channel::{RTCDataChannelId, RTCDataChannelInit};
use ::rtc::peer_connection::configuration::RTCConfigurationBuilder;
use ::rtc::peer_connection::event::{RTCDataChannelEvent, RTCPeerConnectionEvent};
use ::rtc::peer_connection::message::RTCMessage;
use ::rtc::peer_connection::sdp::RTCSessionDescription;
use ::rtc::peer_connection::transport::{CandidateConfig, CandidateHostConfig, RTCIceCandidate};
use ::rtc::peer_connection::{RTCPeerConnection, RTCPeerConnectionBuilder};
use ::rtc::sansio::Protocol;
use ::rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};

use super::rtc::{RtcOptions, RtcPath};
use super::{Channel, FastEvent, FastPath, PeerId, RELIABLE_ID, UNRELIABLE_ID};

pub(crate) const HOST: &str = "192.168.1.10:50000";

/// Tests that replace the panic hook take this first (it is global).
pub(crate) static PANIC_HOOK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A phone: a full-ICE offerer with the two negotiated channels, as the
/// browser client makes them.
pub(crate) struct Phone {
    pub pc: RTCPeerConnection,
    pub addr: SocketAddr,
    r: RTCDataChannelId,
    u: RTCDataChannelId,
    opened: usize,
    pub got: Vec<(Channel, String)>,
}

impl Phone {
    pub(crate) fn new(addr: SocketAddr, now: Instant) -> (Phone, String) {
        let mut pc = RTCPeerConnectionBuilder::new()
            .with_configuration(RTCConfigurationBuilder::new().build())
            .build(now)
            .unwrap();
        let r = pc
            .create_data_channel(
                "r",
                Some(RTCDataChannelInit {
                    ordered: true,
                    negotiated: Some(RELIABLE_ID),
                    ..Default::default()
                }),
            )
            .unwrap()
            .id();
        let u = pc
            .create_data_channel(
                "u",
                Some(RTCDataChannelInit {
                    ordered: false,
                    max_retransmits: Some(0),
                    negotiated: Some(UNRELIABLE_ID),
                    ..Default::default()
                }),
            )
            .unwrap()
            .id();
        let c = CandidateHostConfig {
            base_config: CandidateConfig {
                network: "udp".into(),
                address: addr.ip().to_string(),
                port: addr.port(),
                component: 1,
                ..Default::default()
            },
            ..Default::default()
        }
        .new_candidate_host()
        .unwrap();
        pc.add_local_candidate(RTCIceCandidate::from(&c).to_json().unwrap())
            .unwrap();
        let offer = pc.create_offer(None).unwrap();
        pc.set_local_description(now, offer.clone()).unwrap();
        let phone = Phone {
            pc,
            addr,
            r,
            u,
            opened: 0,
            got: Vec::new(),
        };
        (phone, offer.sdp)
    }

    pub(crate) fn answer(&mut self, sdp: String, now: Instant) {
        self.pc
            .set_remote_description(now, RTCSessionDescription::answer(sdp).unwrap())
            .unwrap();
    }

    pub(crate) fn open(&self) -> bool {
        self.opened == 2
    }

    pub(crate) fn send(&mut self, channel: Channel, text: &str, now: Instant) {
        let id = match channel {
            Channel::Reliable => self.r,
            Channel::Unreliable => self.u,
        };
        self.pc
            .data_channel(id)
            .unwrap()
            .send_text(now, text)
            .unwrap();
    }

    fn read(&mut self, from: SocketAddr, data: &[u8], now: Instant) {
        let _ = self.pc.handle_read(TaggedBytesMut {
            now,
            transport: TransportContext {
                local_addr: self.addr,
                peer_addr: from,
                ecn: None,
                transport_protocol: TransportProtocol::UDP,
            },
            message: data.into(),
        });
    }

    /// Datagrams to send: (to, data).
    fn drain(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        while let Some(w) = self.pc.poll_write() {
            out.push((w.transport.peer_addr, w.message.to_vec()));
        }
        while let Some(e) = self.pc.poll_event() {
            if let RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnOpen(_)) = e {
                self.opened += 1;
            }
        }
        while let Some(m) = self.pc.poll_read() {
            if let RTCMessage::DataChannelMessage(c, m) = m.message {
                let ch = if c == self.r {
                    Channel::Reliable
                } else {
                    Channel::Unreliable
                };
                self.got
                    .push((ch, String::from_utf8_lossy(&m.data).into_owned()));
            }
        }
        while let Some(w) = self.pc.poll_write() {
            out.push((w.transport.peer_addr, w.message.to_vec()));
        }
        out
    }
}

/// Which way a datagram went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dir {
    ToHost,
    ToPhone,
}

/// Changes a datagram in flight.
pub(crate) type Tamper = Box<dyn FnMut(Dir, &mut Vec<u8>)>;

/// The host, phones, the wire and a virtual clock.
pub(crate) struct Net {
    pub host: RtcPath,
    pub host_addr: SocketAddr,
    pub phones: Vec<(PeerId, Phone)>,
    pub now: Instant,
    pub events: Vec<FastEvent>,
    /// Everything that crossed the wire, in order.
    pub log: Vec<(Dir, usize, Vec<u8>)>,
    /// Drops every datagram whose index in the log this returns true for.
    pub lose: Box<dyn FnMut(Dir, usize) -> bool>,
    /// May change a datagram before it is delivered (the fuzz test).
    pub tamper: Tamper,
    wire: VecDeque<(Dir, usize, SocketAddr, Vec<u8>)>,
}

impl Net {
    pub(crate) fn new() -> Net {
        let host_addr: SocketAddr = HOST.parse().unwrap();
        Net {
            host: RtcPath::new(RtcOptions::new(vec![host_addr])).unwrap(),
            host_addr,
            phones: Vec::new(),
            now: Instant::now(),
            events: Vec::new(),
            log: Vec::new(),
            lose: Box::new(|_, _| false),
            tamper: Box::new(|_, _| {}),
            wire: VecDeque::new(),
        }
    }

    /// A phone offers; the host answers.
    pub(crate) fn join(&mut self, id: PeerId) -> usize {
        let addr: SocketAddr = format!("192.168.1.{}:{}", 100 + id, 40000 + id)
            .parse()
            .unwrap();
        let (mut phone, offer) = Phone::new(addr, self.now);
        let answer = self.host.answer(id, &offer, self.now).unwrap();
        phone.answer(answer, self.now);
        self.phones.push((id, phone));
        self.phones.len() - 1
    }

    /// Moves datagrams until nothing is in flight, then advances the clock
    /// to the next timeout (at most `step`).
    pub(crate) fn step(&mut self, step: Duration) {
        loop {
            while let Some(t) = self.host.poll_transmit() {
                assert_eq!(t.local, self.host_addr);
                if let Some(i) = self.phones.iter().position(|(_, p)| p.addr == t.to) {
                    self.wire.push_back((Dir::ToPhone, i, t.local, t.data));
                }
            }
            for i in 0..self.phones.len() {
                for (to, data) in self.phones[i].1.drain() {
                    assert_eq!(to, self.host_addr);
                    self.wire
                        .push_back((Dir::ToHost, i, self.phones[i].1.addr, data));
                }
            }
            while let Some(e) = self.host.poll_event() {
                self.events.push(e);
            }
            let Some((dir, i, from, mut data)) = self.wire.pop_front() else {
                break;
            };
            (self.tamper)(dir, &mut data);
            let n = self.log.len();
            self.log.push((dir, i, data.clone()));
            if (self.lose)(dir, n) {
                continue;
            }
            match dir {
                Dir::ToHost => self.host.datagram(self.host_addr, from, &data, self.now),
                Dir::ToPhone => self.phones[i].1.read(from, &data, self.now),
            }
        }
        let mut next = self.now + step;
        if let Some(t) = self.host.next_timeout() {
            next = next.min(t);
        }
        for (_, p) in &mut self.phones {
            if let Some(t) = p.pc.poll_timeout() {
                next = next.min(t);
            }
        }
        self.now = next.max(self.now + Duration::from_micros(100));
        self.host.timeout(self.now);
        for (_, p) in &mut self.phones {
            let _ = p.pc.handle_timeout(self.now);
        }
    }

    /// Steps until `done` or `limit` of virtual time passed.
    pub(crate) fn run_until(
        &mut self,
        limit: Duration,
        mut done: impl FnMut(&Net) -> bool,
    ) -> bool {
        let end = self.now + limit;
        while self.now < end {
            if done(self) {
                return true;
            }
            self.step(Duration::from_millis(50));
        }
        done(self)
    }
}

#[test]
fn a_phone_connects_and_talks_both_ways() {
    let mut net = Net::new();
    net.join(7);
    assert!(
        net.run_until(Duration::from_secs(10), |n| n.phones[0].1.open()
            && n.events.contains(&FastEvent::Open(7)))
    );
    let now = net.now;
    net.phones[0]
        .1
        .send(Channel::Reliable, r#"{"t":"act","id":1}"#, now);
    net.phones[0]
        .1
        .send(Channel::Unreliable, r#"{"t":"in","s":1}"#, now);
    assert!(net
        .host
        .send(7, Channel::Reliable, r#"{"t":"ack","id":1}"#, now));
    assert!(
        net.run_until(Duration::from_secs(5), |n| n.events.len() >= 3
            && !n.phones[0].1.got.is_empty())
    );
    // Two channels: no order between them.
    net.events[1..].sort_by_key(|e| format!("{e:?}"));
    assert_eq!(
        net.events[1..],
        [
            FastEvent::Message {
                peer: 7,
                channel: Channel::Reliable,
                text: r#"{"t":"act","id":1}"#.into()
            },
            FastEvent::Message {
                peer: 7,
                channel: Channel::Unreliable,
                text: r#"{"t":"in","s":1}"#.into()
            },
        ]
    );
    assert_eq!(
        net.phones[0].1.got,
        [(Channel::Reliable, r#"{"t":"ack","id":1}"#.into())]
    );
}

#[test]
fn many_phones_share_the_host_socket() {
    let mut net = Net::new();
    for id in 1..=4 {
        net.join(id);
    }
    assert!(net.run_until(Duration::from_secs(10), |n| n
        .events
        .iter()
        .filter(|e| matches!(e, FastEvent::Open(_)))
        .count()
        == 4));
    let now = net.now;
    for id in 1..=4u64 {
        assert!(net
            .host
            .send(id, Channel::Reliable, &format!("to {id}"), now));
    }
    assert!(net.run_until(Duration::from_secs(5), |n| n
        .phones
        .iter()
        .all(|(_, p)| !p.got.is_empty())));
    for (id, p) in &net.phones {
        assert_eq!(p.got, [(Channel::Reliable, format!("to {id}"))]);
    }
}

#[test]
fn a_silent_phone_times_out() {
    let mut net = Net::new();
    net.join(1);
    net.lose = Box::new(|_, _| true);
    assert!(net.run_until(Duration::from_secs(15), |n| n
        .events
        .iter()
        .any(|e| matches!(e, FastEvent::Down { peer: 1, .. }))));
    assert_eq!(net.host.peers(), 0);
}

#[test]
fn a_phone_that_goes_away_is_reported_down() {
    let mut net = Net::new();
    net.join(1);
    assert!(net.run_until(Duration::from_secs(10), |n| n
        .events
        .contains(&FastEvent::Open(1))));
    let since = net.log.len();
    net.lose = Box::new(move |_, n| n >= since);
    assert!(net.run_until(Duration::from_secs(60), |n| n
        .events
        .iter()
        .any(|e| matches!(e, FastEvent::Down { peer: 1, .. }))));
    assert_eq!(net.host.peers(), 0);
}

#[test]
fn closing_tells_the_phone_and_forgets() {
    let mut net = Net::new();
    net.join(1);
    assert!(net.run_until(Duration::from_secs(10), |n| n
        .events
        .contains(&FastEvent::Open(1))));
    let now = net.now;
    net.host.close(1, now);
    assert_eq!(net.host.peers(), 0);
    assert!(!net.host.send(1, Channel::Reliable, "x", now));
    net.step(Duration::from_millis(10));
    assert!(!net
        .events
        .iter()
        .any(|e| matches!(e, FastEvent::Down { .. })));
}

#[test]
fn bad_offers_are_errors() {
    let mut host = RtcPath::new(RtcOptions::new(vec![HOST.parse().unwrap()])).unwrap();
    let now = Instant::now();
    for offer in ["", "v=0", "garbage\r\n", &"a".repeat(20_000)] {
        assert!(host.answer(1, offer, now).is_err(), "{offer:?}");
    }
    assert_eq!(host.peers(), 0);
}

#[test]
fn a_panic_in_rtc_costs_only_that_peer() {
    let mut net = Net::new();
    net.join(1);
    net.join(2);
    assert!(net.run_until(Duration::from_secs(10), |n| n
        .events
        .iter()
        .filter(|e| matches!(e, FastEvent::Open(_)))
        .count()
        == 2));
    let (host, from, now) = (net.host_addr, net.phones[0].1.addr, net.now);
    let _hook = PANIC_HOOK.lock().unwrap_or_else(|e| e.into_inner());
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    net.host.datagram(host, from, super::rtc::POISON, now);
    std::panic::set_hook(hook);
    while let Some(e) = net.host.poll_event() {
        net.events.push(e);
    }
    assert!(net.events.contains(&FastEvent::Down {
        peer: 1,
        why: super::rtc::CRASHED.into()
    }));
    assert_eq!(net.host.peers(), 1);
    assert!(net.host.send(2, Channel::Reliable, "still here", now));
    assert!(net.run_until(Duration::from_secs(5), |n| !n.phones[1].1.got.is_empty()));
}
