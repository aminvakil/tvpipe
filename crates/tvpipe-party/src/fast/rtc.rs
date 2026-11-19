//! [`FastPath`] on the sans-IO `rtc` crate: one ICE-lite answerer
//! ([`RTCPeerConnection`]) per phone, host candidates only, one DTLS
//! certificate for all of them.
//!
//! Every phone talks to the same sockets. A datagram finds its peer by
//! the host ufrag in a STUN binding request's USERNAME (each peer gets
//! its own), and after that by its address pair, learned from those
//! requests (the phone's address is peer-reflexive: browsers hide theirs
//! behind mDNS names, which the host never resolves).

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use ::rtc::data_channel::{RTCDataChannelId, RTCDataChannelInit};
use ::rtc::peer_connection::certificate::{CertificateParams, RTCCertificate};
use ::rtc::peer_connection::configuration::setting_engine::{
    SctpMaxMessageSize, SettingEngineBuilder,
};
use ::rtc::peer_connection::configuration::RTCConfigurationBuilder;
use ::rtc::peer_connection::event::{RTCDataChannelEvent, RTCPeerConnectionEvent};
use ::rtc::peer_connection::message::RTCMessage;
use ::rtc::peer_connection::sdp::RTCSessionDescription;
use ::rtc::peer_connection::state::RTCPeerConnectionState;
use ::rtc::peer_connection::transport::{CandidateConfig, CandidateHostConfig, RTCIceCandidate};
use ::rtc::peer_connection::{RTCPeerConnection, RTCPeerConnectionBuilder};
use ::rtc::sansio::Protocol;
use ::rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};

use super::{stun, Channel, FastEvent, FastPath, PeerId, Transmit, RELIABLE_ID, UNRELIABLE_ID};

/// Offers past this size are refused (a browser's is ~1 KiB).
const MAX_OFFER: usize = 16 * 1024;
/// Address pairs remembered per peer (a phone switching networks).
const MAX_ROUTES: usize = 8;
/// A datagram that panics in tests.
#[cfg(test)]
pub(crate) const POISON: &[u8] = b"tvpipe-party: poison";
#[cfg(not(test))]
const POISON: &[u8] = b"";
/// [`FastEvent::Down`]'s reason when `rtc` panicked.
pub(crate) const CRASHED: &str = "internal error";

/// Limits and addresses.
#[derive(Clone, Debug)]
pub(crate) struct RtcOptions {
    /// The local sockets' addresses: the host candidates.
    pub locals: Vec<SocketAddr>,
    /// Peers at most (more offers are refused).
    pub max_peers: usize,
    /// A peer whose channels are not open by then is dropped.
    pub connect_timeout: Duration,
    /// The largest message either way (SCTP max-message-size).
    pub max_message: u32,
    /// Bytes queued on `r` past which the peer is dropped as too slow
    /// (the WebSocket, with its latest-wins views, takes over).
    pub max_buffered: usize,
}

impl RtcOptions {
    pub(crate) fn new(locals: Vec<SocketAddr>) -> RtcOptions {
        RtcOptions {
            locals,
            max_peers: 64,
            connect_timeout: Duration::from_secs(10),
            max_message: 64 * 1024,
            max_buffered: 256 * 1024,
        }
    }
}

struct Peer {
    pc: RTCPeerConnection,
    ufrag: String,
    r: RTCDataChannelId,
    u: RTCDataChannelId,
    r_open: bool,
    u_open: bool,
    open: bool,
    /// Not open by then: dropped.
    deadline: Instant,
    routes: VecDeque<(SocketAddr, SocketAddr)>,
    /// Why it is going down (removed after the drain that found it).
    down: Option<String>,
}

/// The `rtc` stack for many phones.
pub(crate) struct RtcPath {
    options: RtcOptions,
    cert: RTCCertificate,
    peers: HashMap<PeerId, Peer>,
    ufrags: HashMap<String, PeerId>,
    routes: HashMap<(SocketAddr, SocketAddr), PeerId>,
    transmits: VecDeque<Transmit>,
    events: VecDeque<FastEvent>,
}

impl RtcPath {
    /// Makes the certificate (ECDSA P-256, as browsers do).
    pub(crate) fn new(options: RtcOptions) -> Result<RtcPath, String> {
        let provider = ::rtc::crypto::default_provider().map_err(|e| e.to_string())?;
        let params =
            CertificateParams::new(vec!["tvpipe-party".to_owned()]).map_err(|e| e.to_string())?;
        let cert = RTCCertificate::generate(
            provider.crypto(),
            ::rtc::crypto::SignatureScheme::EcdsaP256Sha256,
            params,
        )
        .map_err(|e| e.to_string())?;
        Ok(RtcPath {
            options,
            cert,
            peers: HashMap::new(),
            ufrags: HashMap::new(),
            routes: HashMap::new(),
            transmits: VecDeque::new(),
            events: VecDeque::new(),
        })
    }

    fn new_peer(&mut self, offer: &str, now: Instant) -> Result<(Peer, String), String> {
        let err = |e: ::rtc::shared::error::Error| e.to_string();
        let ufrag = loop {
            let u = random_ice_chars(8);
            if !self.ufrags.contains_key(&u) {
                break u;
            }
        };
        let settings = SettingEngineBuilder::new()
            .with_lite(true)
            .with_ice_credentials(ufrag.clone(), random_ice_chars(24))
            .with_sctp_max_message_size(SctpMaxMessageSize::Bounded(self.options.max_message))
            .build();
        let config = RTCConfigurationBuilder::new()
            .with_certificates(vec![self.cert.clone()])
            .build();
        let mut pc = RTCPeerConnectionBuilder::new()
            .with_configuration(config)
            .with_setting_engine(settings)
            .build(now)
            .map_err(err)?;
        let r = pc
            .create_data_channel(
                "r",
                Some(RTCDataChannelInit {
                    ordered: true,
                    negotiated: Some(RELIABLE_ID),
                    ..Default::default()
                }),
            )
            .map_err(err)?
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
            .map_err(err)?
            .id();
        pc.set_remote_description(
            now,
            RTCSessionDescription::offer(offer.to_owned()).map_err(err)?,
        )
        .map_err(err)?;
        for local in &self.options.locals {
            let c = CandidateHostConfig {
                base_config: CandidateConfig {
                    network: "udp".into(),
                    address: local.ip().to_string(),
                    port: local.port(),
                    component: 1,
                    ..Default::default()
                },
                ..Default::default()
            }
            .new_candidate_host()
            .map_err(|e| e.to_string())?;
            let init = RTCIceCandidate::from(&c).to_json().map_err(err)?;
            pc.add_local_candidate(init).map_err(err)?;
        }
        let answer = pc.create_answer(None).map_err(err)?;
        pc.set_local_description(now, answer.clone()).map_err(err)?;
        let peer = Peer {
            pc,
            ufrag,
            r,
            u,
            r_open: false,
            u_open: false,
            open: false,
            deadline: now + self.options.connect_timeout,
            routes: VecDeque::new(),
            down: None,
        };
        Ok((peer, answer.sdp))
    }

    /// Moves what a peer produced into the queues; forgets it if it went
    /// down.
    fn drain(&mut self, id: PeerId) {
        let Some(p) = self.peers.get_mut(&id) else {
            return;
        };
        let (transmits, events) = (&mut self.transmits, &mut self.events);
        if guard(|| drain_peer(id, p, transmits, events)).is_none() {
            p.down = Some(CRASHED.into());
        }
        if let Some(why) = p.down.take() {
            self.close(id, Instant::now());
            self.events.push_back(FastEvent::Down { peer: id, why });
        }
    }

    fn forget(&mut self, id: PeerId) -> Option<Peer> {
        let p = self.peers.remove(&id)?;
        self.ufrags.remove(&p.ufrag);
        for r in &p.routes {
            if self.routes.get(r) == Some(&id) {
                self.routes.remove(r);
            }
        }
        Some(p)
    }

    fn route(&mut self, local: SocketAddr, from: SocketAddr, data: &[u8]) -> Option<PeerId> {
        if let Some(ufrag) = stun::request_ufrag(data) {
            let id = *self.ufrags.get(ufrag)?;
            let key = (local, from);
            if self.routes.get(&key) != Some(&id) {
                if let Some(old) = self.routes.insert(key, id) {
                    if let Some(p) = self.peers.get_mut(&old) {
                        p.routes.retain(|r| *r != key);
                    }
                }
                let p = self.peers.get_mut(&id)?;
                p.routes.push_back(key);
                if p.routes.len() > MAX_ROUTES {
                    if let Some(r) = p.routes.pop_front() {
                        self.routes.remove(&r);
                    }
                }
            }
            return Some(id);
        }
        self.routes.get(&(local, from)).copied()
    }
}

impl FastPath for RtcPath {
    fn answer(&mut self, peer: PeerId, offer: &str, now: Instant) -> Result<String, String> {
        if offer.len() > MAX_OFFER {
            return Err("offer too big".into());
        }
        if let Some(mut old) = self.forget(peer) {
            guard(|| old.pc.close());
        }
        if self.peers.len() >= self.options.max_peers {
            return Err("too many peers".into());
        }
        let (p, sdp) = guard(|| self.new_peer(offer, now)).ok_or(CRASHED)??;
        self.ufrags.insert(p.ufrag.clone(), peer);
        self.peers.insert(peer, p);
        self.drain(peer);
        Ok(sdp)
    }

    fn datagram(&mut self, local: SocketAddr, from: SocketAddr, data: &[u8], now: Instant) {
        let Some(id) = self.route(local, from, data) else {
            return;
        };
        let Some(p) = self.peers.get_mut(&id) else {
            return;
        };
        let read = TaggedBytesMut {
            now,
            transport: TransportContext {
                local_addr: local,
                peer_addr: from,
                ecn: None,
                transport_protocol: TransportProtocol::UDP,
            },
            message: data.into(),
        };
        // Tests poison a peer to check the containment.
        let poison = cfg!(test) && data == POISON;
        match guard(|| {
            assert!(!poison, "poisoned");
            p.pc.handle_read(read)
        }) {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                p.down.get_or_insert_with(|| e.to_string());
            }
            None => p.down = Some(CRASHED.into()),
        }
        self.drain(id);
    }

    fn send(&mut self, peer: PeerId, channel: Channel, text: &str, now: Instant) -> bool {
        let max_buffered = self.options.max_buffered;
        let Some(p) = self.peers.get_mut(&peer) else {
            return false;
        };
        if !p.open {
            return false;
        }
        let id = match channel {
            Channel::Reliable => p.r,
            Channel::Unreliable => p.u,
        };
        let pc = &mut p.pc;
        let sent = guard(|| {
            let mut dc = pc.data_channel(id)?;
            if channel == Channel::Reliable && dc.outstanding_bytes() > max_buffered {
                return None;
            }
            Some(dc.send_text(now, text).is_ok())
        });
        let sent = match sent {
            Some(Some(sent)) => sent,
            Some(None) => {
                p.down = Some("too slow".into());
                false
            }
            None => {
                p.down = Some(CRASHED.into());
                false
            }
        };
        self.drain(peer);
        sent
    }

    fn close(&mut self, peer: PeerId, _now: Instant) {
        let Some(mut p) = self.forget(peer) else {
            return;
        };
        let transmits = &mut self.transmits;
        guard(|| {
            let _ = p.pc.close();
            while let Some(w) = p.pc.poll_write() {
                transmits.push_back(Transmit {
                    local: w.transport.local_addr,
                    to: w.transport.peer_addr,
                    data: w.message.to_vec(),
                });
            }
        });
    }

    fn next_timeout(&mut self) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        for p in self.peers.values_mut() {
            let pc = &mut p.pc;
            let polled = match guard(|| pc.poll_timeout()) {
                Some(t) => t,
                None => {
                    // `timeout` forgets it.
                    p.down = Some(CRASHED.into());
                    p.deadline = Instant::now();
                    p.open = false;
                    None
                }
            };
            let t = match (polled, p.open) {
                (Some(t), true) => t,
                (Some(t), false) => t.min(p.deadline),
                (None, true) => continue,
                (None, false) => p.deadline,
            };
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        next
    }

    fn timeout(&mut self, now: Instant) {
        let ids: Vec<PeerId> = self.peers.keys().copied().collect();
        for id in ids {
            let Some(p) = self.peers.get_mut(&id) else {
                continue;
            };
            if !p.open && now >= p.deadline {
                p.down.get_or_insert_with(|| "timed out".into());
            } else {
                let pc = &mut p.pc;
                let ran = guard(|| {
                    if pc.poll_timeout().is_some_and(|t| t <= now) {
                        pc.handle_timeout(now).map_err(|e| e.to_string())
                    } else {
                        Ok(())
                    }
                });
                match ran {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        p.down.get_or_insert(e);
                    }
                    None => p.down = Some(CRASHED.into()),
                }
            }
            self.drain(id);
        }
    }

    fn poll_transmit(&mut self) -> Option<Transmit> {
        self.transmits.pop_front()
    }

    fn poll_event(&mut self) -> Option<FastEvent> {
        self.events.pop_front()
    }

    #[cfg(test)]
    fn peers(&self) -> usize {
        self.peers.len()
    }
}

/// What `p` produced, into the queues (see [`RtcPath::drain`]).
fn drain_peer(
    id: PeerId,
    p: &mut Peer,
    transmits: &mut VecDeque<Transmit>,
    events: &mut VecDeque<FastEvent>,
) {
    loop {
        let mut any = false;
        while let Some(w) = p.pc.poll_write() {
            any = true;
            transmits.push_back(Transmit {
                local: w.transport.local_addr,
                to: w.transport.peer_addr,
                data: w.message.to_vec(),
            });
        }
        while let Some(e) = p.pc.poll_event() {
            any = true;
            match e {
                RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnOpen(c)) => {
                    p.r_open |= c == p.r;
                    p.u_open |= c == p.u;
                    if p.r_open && p.u_open && !p.open {
                        p.open = true;
                        events.push_back(FastEvent::Open(id));
                    }
                }
                RTCPeerConnectionEvent::OnDataChannel(
                    RTCDataChannelEvent::OnClose(c) | RTCDataChannelEvent::OnClosing(c),
                ) if c == p.r || c == p.u => {
                    p.down.get_or_insert_with(|| "channel closed".into());
                }
                RTCPeerConnectionEvent::OnConnectionStateChangeEvent(
                    s @ (RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed),
                ) => {
                    p.down.get_or_insert_with(|| format!("connection {s}"));
                }
                _ => {}
            }
        }
        while let Some(m) = p.pc.poll_read() {
            any = true;
            if let RTCMessage::DataChannelMessage(c, m) = m.message {
                let channel = if c == p.r {
                    Channel::Reliable
                } else if c == p.u {
                    Channel::Unreliable
                } else {
                    continue;
                };
                if !m.is_string {
                    continue;
                }
                if let Ok(text) = String::from_utf8(m.data.to_vec()) {
                    events.push_back(FastEvent::Message {
                        peer: id,
                        channel,
                        text,
                    });
                }
            }
        }
        if !any {
            return;
        }
    }
}

/// Runs a call into `rtc`, turning a panic into `None`.
///
/// rtc 0.21.1 panics on some malformed SCTP and DCEP input from a peer
/// that completed DTLS (docs/multiscreen-design.md §2.5): contained
/// here, it costs that peer its fast path and nothing else. Only with
/// `panic = "unwind"`; with `abort` the fast path stays off
/// ([`super::available`]).
fn guard<T>(f: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

/// `n` random ICE characters (`[A-Za-z0-9+/]`, RFC 8839 ice-char).
fn random_ice_chars(n: usize) -> String {
    use ring::rand::SecureRandom;
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b = vec![0u8; n];
    // As for the party's keys: refusing to run beats predictable ones.
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .expect("the system random generator failed");
    b.iter()
        .map(|x| char::from(CHARS[usize::from(x % 64)]))
        .collect()
}
