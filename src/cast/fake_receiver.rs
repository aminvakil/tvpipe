//! A fake Cast Streaming receiver for tests: TLS control channel
//! (CONNECT, LAUNCH → RECEIVER_STATUS, OFFER → ANSWER, STOP), and a UDP
//! side that reassembles and decrypts frames and answers with Cast RTCP
//! (receiver report, reference time, checkpoint ACKs, NACKs, CST2 ACKs,
//! optional PLI). Incoming RTP can be dropped at a configurable rate to
//! exercise retransmission.
//!
//! Behaviour modelled on openscreen's receiver (`receiver_impl.cc`,
//! `compound_rtcp_builder.cc`, `application_agent.cc`).

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::messages;
use super::rtcp::{PacketNack, ReceiverPacket, ReportBlock, SenderReport};
use super::rtp::{expand_near, FrameCrypto, FrameId, RtpPacket, ALL_PACKETS_LOST, LEADER};
use super::testing::ServerConn;
use crate::codec::json::Value;
use crate::codec::protobuf::CastMessage;
use crate::util::hex;
use crate::util::time::NtpTime;

pub const TRANSPORT_ID: &str = "fake-transport-1";
pub const SESSION_ID: &str = "fake-session-1";

/// A completely received frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub frame_id: FrameId,
    pub keyframe: bool,
    pub referenced: FrameId,
    pub rtp_timestamp: u32,
    pub data: Vec<u8>,
    pub completed_at: Instant,
}

#[derive(Debug, Default)]
struct Partial {
    keyframe: bool,
    referenced: FrameId,
    rtp_timestamp: u32,
    packets: Vec<Option<Vec<u8>>>,
}

/// One accepted stream.
pub struct Stream {
    pub codec: String,
    pub sender_ssrc: u32,
    pub receiver_ssrc: u32,
    crypto: FrameCrypto,
    partial: BTreeMap<FrameId, Partial>,
    pub frames: BTreeMap<FrameId, Frame>,
    pub checkpoint: FrameId,
    highest: FrameId,
    pub packets: u64,
    pub dropped: u64,
    pub duplicates: u64,
    pub sender_reports: Vec<SenderReport>,
    last_sr_at: Option<Instant>,
    feedback_count: u8,
    pub pli: bool,
}

impl Stream {
    fn on_rtp(&mut self, p: &RtpPacket<'_>, now: Instant) -> bool {
        let id = expand_near(self.highest.max(0), p.frame_id);
        if id <= self.checkpoint || self.frames.contains_key(&id) {
            self.duplicates += 1;
            return false;
        }
        self.highest = self.highest.max(id);
        let n = p.max_packet_id as usize + 1;
        let part = self.partial.entry(id).or_insert_with(|| Partial {
            keyframe: p.keyframe,
            referenced: p.referenced(id),
            rtp_timestamp: p.rtp_timestamp,
            packets: vec![None; n],
        });
        if part.packets.len() != n || p.packet_id as usize >= n {
            return false;
        }
        let slot = &mut part.packets[p.packet_id as usize];
        if slot.is_some() {
            self.duplicates += 1;
            return false;
        }
        *slot = Some(p.payload.to_vec());
        if part.packets.iter().all(Option::is_some) {
            let part = self.partial.remove(&id).unwrap_or_default();
            let mut data: Vec<u8> = part.packets.into_iter().flatten().flatten().collect();
            self.crypto.apply(id, &mut data);
            self.frames.insert(
                id,
                Frame {
                    frame_id: id,
                    keyframe: part.keyframe,
                    referenced: part.referenced,
                    rtp_timestamp: part.rtp_timestamp,
                    data,
                    completed_at: now,
                },
            );
            while self.frames.contains_key(&(self.checkpoint + 1)) {
                self.checkpoint += 1;
            }
            return true;
        }
        false
    }

    fn feedback(&mut self, now: Instant, clock: (Instant, NtpTime)) -> Vec<u8> {
        let mut nacks = Vec::new();
        for id in self.checkpoint + 1..=self.highest {
            if self.frames.contains_key(&id) {
                continue;
            }
            match self.partial.get(&id) {
                None => nacks.push(PacketNack {
                    frame_id: id,
                    packet_id: ALL_PACKETS_LOST,
                }),
                Some(p) => {
                    for (i, pkt) in p.packets.iter().enumerate() {
                        if pkt.is_none() {
                            nacks.push(PacketNack {
                                frame_id: id,
                                packet_id: i as u16,
                            });
                        }
                    }
                }
            }
        }
        let acks: Vec<FrameId> = self
            .frames
            .range(self.checkpoint + 2..)
            .map(|(&id, _)| id)
            .collect();
        let report = self.sender_reports.last().map(|sr| ReportBlock {
            ssrc: self.sender_ssrc,
            last_sr: sr.report_id(),
            delay_since_last_sr: self
                .last_sr_at
                .map(|t| ((now - t).as_nanos() * 65536 / 1_000_000_000) as u32)
                .unwrap_or(0),
            ..Default::default()
        });
        let ntp = NtpTime(clock.1 .0 + (((now - clock.0).as_nanos() << 32) / 1_000_000_000) as u64);
        self.feedback_count = self.feedback_count.wrapping_add(1);
        let pkt = ReceiverPacket {
            receiver_ssrc: self.receiver_ssrc,
            sender_ssrc: self.sender_ssrc,
            report,
            reference_ntp: ntp.0,
            picture_loss: std::mem::take(&mut self.pli),
            checkpoint: self.checkpoint,
            playout_delay_ms: 400,
            nacks,
            acks,
            feedback_count: self.feedback_count,
        };
        pkt.build()
    }
}

/// Shared receiver state, inspected by tests.
#[derive(Default)]
pub struct State {
    pub streams: Vec<Stream>,
    pub connects: Vec<(String, String)>,
    pub launched: bool,
    pub stopped: bool,
    pub offer: Option<Value>,
    pub answer_port: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Fraction of incoming RTP packets to drop (0.0..1.0).
    pub loss: f64,
    pub seed: u32,
    /// Refuse the LAUNCH.
    pub fail_launch: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            loss: 0.0,
            seed: 0x1234_5678,
            fail_launch: false,
        }
    }
}

pub struct FakeReceiver {
    pub addr: SocketAddr,
    pub state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl FakeReceiver {
    pub fn spawn(opts: Options) -> FakeReceiver {
        // Its 10 ms receive polls and 20 ms feedback need a fine timer on
        // Windows, like the sender's pacing (`util::timer`).
        crate::util::timer::raise_resolution();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let state = Arc::new(Mutex::new(State {
            answer_port: udp.local_addr().unwrap().port(),
            ..State::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        {
            let state = state.clone();
            let stop = stop.clone();
            threads.push(std::thread::spawn(move || {
                let (sock, _) = listener.accept().unwrap();
                let mut conn = ServerConn::accept(sock);
                conn.set_read_timeout(Duration::from_millis(100));
                control(&mut conn, &state, &stop, opts);
            }));
        }
        {
            let state = state.clone();
            let stop = stop.clone();
            threads.push(std::thread::spawn(move || media(&udp, &state, &stop, opts)));
        }
        FakeReceiver {
            addr,
            state,
            stop,
            threads,
        }
    }

    /// Asks the receiver to send a PLI on the video stream.
    pub fn request_keyframe(&self) {
        let mut st = self.state.lock().unwrap();
        for s in st.streams.iter_mut().filter(|s| s.codec == "h264") {
            s.pli = true;
        }
    }

    pub fn shutdown(mut self) -> Arc<Mutex<State>> {
        self.stop.store(true, Ordering::Release);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        self.state.clone()
    }
}

fn reply(conn: &mut ServerConn, to: &CastMessage, ns: &str, payload: &Value) {
    let _ = conn.send(&CastMessage::json(
        to.destination_id.clone(),
        to.source_id.clone(),
        ns,
        payload,
    ));
}

fn status(request_id: u32, running: bool) -> Value {
    let apps = if running {
        Value::array([Value::object()
            .with("appId", messages::MIRRORING_APP_ID)
            .with("sessionId", SESSION_ID)
            .with("transportId", TRANSPORT_ID)
            .with("displayName", "Fake Mirroring")
            .with(
                "namespaces",
                Value::array([Value::object().with("name", messages::NS_WEBRTC)]),
            )])
    } else {
        Value::array(Vec::<Value>::new())
    };
    Value::object()
        .with("type", "RECEIVER_STATUS")
        .with("requestId", request_id)
        .with("status", Value::object().with("applications", apps))
}

fn control(conn: &mut ServerConn, state: &Mutex<State>, stop: &AtomicBool, opts: Options) {
    // Above the sender's ranges (audio 1..=50000, video 50001..=100000,
    // picked at random): an answer reusing the sender's SSRC is rejected,
    // which with 1001.. made about one audio stream in 25000 fail.
    let mut next_ssrc = 200_000;
    while !stop.load(Ordering::Acquire) {
        let msg = match conn.recv() {
            Ok(m) => m,
            Err(e) if e.kind() == crate::error::ErrorKind::Timeout => continue,
            Err(e) if e.kind() == crate::error::ErrorKind::Io => continue,
            Err(_) => return,
        };
        let Ok(v) = msg.payload_json() else { continue };
        let ty = v["type"].as_str().unwrap_or("");
        let rid = v["requestId"].as_u32().unwrap_or(0);
        match (msg.namespace.as_str(), ty) {
            (messages::NS_CONNECTION, "CONNECT") => {
                state
                    .lock()
                    .unwrap()
                    .connects
                    .push((msg.source_id.clone(), msg.destination_id.clone()));
            }
            (messages::NS_RECEIVER, "LAUNCH") => {
                if opts.fail_launch {
                    reply(
                        conn,
                        &msg,
                        messages::NS_RECEIVER,
                        &Value::object()
                            .with("type", "LAUNCH_ERROR")
                            .with("requestId", rid)
                            .with("reason", "NOT_FOUND"),
                    );
                    continue;
                }
                state.lock().unwrap().launched = true;
                // An unrelated status first, like real receivers send.
                reply(conn, &msg, messages::NS_RECEIVER, &status(0, false));
                reply(conn, &msg, messages::NS_RECEIVER, &status(rid, true));
            }
            (messages::NS_RECEIVER, "STOP") => {
                state.lock().unwrap().stopped = true;
                reply(conn, &msg, messages::NS_RECEIVER, &status(rid, false));
            }
            (messages::NS_WEBRTC, "OFFER") => {
                let seq = v["seqNum"].as_u32().unwrap_or(0);
                let mut st = state.lock().unwrap();
                st.offer = Some(v.clone());
                let mut idx = Vec::new();
                let mut ssrcs = Vec::new();
                let mut have_audio = false;
                let mut have_video = false;
                for s in v["offer"]["supportedStreams"]
                    .as_array()
                    .unwrap_or_default()
                {
                    let codec = s["codecName"].as_str().unwrap_or("").to_owned();
                    let take = match codec.as_str() {
                        "opus" | "aac" if !have_audio => {
                            have_audio = true;
                            true
                        }
                        "h264" if !have_video => {
                            have_video = true;
                            true
                        }
                        _ => false,
                    };
                    if !take {
                        continue;
                    }
                    let key: [u8; 16] = hex::decode(s["aesKey"].as_str().unwrap())
                        .unwrap()
                        .try_into()
                        .unwrap();
                    let mask: [u8; 16] = hex::decode(s["aesIvMask"].as_str().unwrap())
                        .unwrap()
                        .try_into()
                        .unwrap();
                    next_ssrc += 1;
                    idx.push(s["index"].as_u32().unwrap());
                    ssrcs.push(next_ssrc);
                    st.streams.push(Stream {
                        codec,
                        sender_ssrc: s["ssrc"].as_u32().unwrap(),
                        receiver_ssrc: next_ssrc,
                        crypto: FrameCrypto::new(&key, &mask).unwrap(),
                        partial: BTreeMap::new(),
                        frames: BTreeMap::new(),
                        checkpoint: LEADER,
                        highest: LEADER,
                        packets: 0,
                        dropped: 0,
                        duplicates: 0,
                        sender_reports: Vec::new(),
                        last_sr_at: None,
                        feedback_count: 0,
                        pli: false,
                    });
                }
                let answer = Value::object()
                    .with("type", "ANSWER")
                    .with("seqNum", seq)
                    .with("result", "ok")
                    .with(
                        "answer",
                        Value::object()
                            .with("udpPort", st.answer_port)
                            .with("sendIndexes", Value::array(idx))
                            .with("ssrcs", Value::array(ssrcs))
                            .with(
                                "constraints",
                                Value::object()
                                    .with(
                                        "audio",
                                        Value::object()
                                            .with("maxSampleRate", 48000)
                                            .with("maxChannels", 2)
                                            .with("maxBitRate", 256000),
                                    )
                                    .with(
                                        "video",
                                        Value::object()
                                            .with(
                                                "maxDimensions",
                                                Value::object()
                                                    .with("width", 1280)
                                                    .with("height", 720)
                                                    .with("frameRate", "30"),
                                            )
                                            .with("maxBitRate", 5_000_000),
                                    ),
                            ),
                    );
                drop(st);
                reply(conn, &msg, messages::NS_WEBRTC, &answer);
            }
            _ => {}
        }
    }
}

fn media(udp: &UdpSocket, state: &Mutex<State>, stop: &AtomicBool, opts: Options) {
    let clock = (Instant::now(), NtpTime::now());
    let mut buf = vec![0u8; 2048];
    let mut rng = opts.seed.max(1);
    let mut peer: Option<SocketAddr> = None;
    let mut last_feedback = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let mut send_now = false;
        if let Ok((n, from)) = udp.recv_from(&mut buf) {
            peer = Some(from);
            let now = Instant::now();
            let pkt = &buf[..n];
            let mut st = state.lock().unwrap();
            if super::rtcp::is_rtcp(pkt) {
                let Some(author) = super::rtcp::author_ssrc(pkt) else {
                    continue;
                };
                if let Some(s) = st.streams.iter_mut().find(|s| s.sender_ssrc == author) {
                    if let Ok(Some(sr)) = SenderReport::parse(pkt, author) {
                        s.sender_reports.push(sr);
                        s.last_sr_at = Some(now);
                    }
                }
            } else if let Ok(p) = RtpPacket::parse(pkt) {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let drop_it = (rng as f64 / u32::MAX as f64) < opts.loss;
                if let Some(s) = st.streams.iter_mut().find(|s| s.sender_ssrc == p.ssrc) {
                    if drop_it {
                        s.dropped += 1;
                    } else {
                        s.packets += 1;
                        send_now |= s.on_rtp(&p, now);
                    }
                }
            }
        }
        let now = Instant::now();
        if (send_now || now - last_feedback >= Duration::from_millis(20)) && peer.is_some() {
            last_feedback = now;
            let mut st = state.lock().unwrap();
            for s in st.streams.iter_mut() {
                if s.sender_reports.is_empty() && s.highest == LEADER {
                    continue;
                }
                let fb = s.feedback(now, clock);
                if let Some(p) = peer {
                    let _ = udp.send_to(&fb, p);
                }
            }
        }
    }
}
