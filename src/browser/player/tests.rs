//! The browser receiver against a Rust WebSocket client playing the page
//! (no browser): page server security, config, record framing, timing on
//! the session clock, late joiners and keyframes, slow tabs.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::browser::protocol;
use crate::media::clock::MediaClock;
use crate::net::websocket::{self, Message};

fn never() -> bool {
    false
}

/// An avcC (Constrained Baseline, level 3.1) from the built-in encoder.
fn avcc() -> Vec<u8> {
    let enc = crate::media::h264enc::Encoder::new(crate::media::h264enc::Config {
        width: 320,
        height: 240,
        ..crate::media::h264enc::Config::default()
    })
    .unwrap();
    enc.avcc()
}

fn video_info() -> VideoInfo {
    VideoInfo {
        codec: VideoCodec::H264,
        width: 320,
        height: 240,
        fps: Ratio::integer(30),
        avcc: avcc(),
    }
}

fn audio_info() -> AudioInfo {
    AudioInfo {
        codec: AudioCodec::PcmS16le,
        sample_rate: 48_000,
        channels: 2,
        frame_samples: 480,
        config: Vec::new(),
    }
}

fn video(pts: u64, key: bool, fill: u8) -> EncodedFrame {
    // One fake NAL unit (the sink does not parse AVCC unless it has to).
    let data = vec![0, 0, 0, 3, if key { 0x65 } else { 0x41 }, fill, fill];
    EncodedFrame {
        track: TrackKind::Video,
        pts,
        keyframe: key,
        data: data.into(),
    }
}

fn audio(pts: u64) -> EncodedFrame {
    EncodedFrame {
        track: TrackKind::Audio(AudioFamily::AirPlay),
        pts,
        keyframe: true,
        data: vec![1u8; 480 * 4].into(),
    }
}

/// A fake page: a WebSocket client with a read timeout.
struct Page {
    rx: websocket::Receiver<TcpStream, TcpStream>,
    tx: websocket::Sender<TcpStream>,
}

impl Page {
    fn connect(sink: &BrowserSink) -> Page {
        let (mut rx, tx) = websocket::connect(
            &sink.ws_url(),
            Duration::from_secs(10),
            websocket::Limits::default(),
        )
        .unwrap();
        // The receiver's own handle: on Windows timeouts are per handle.
        rx.with_stream(|s| s.set_read_timeout(Some(Duration::from_secs(10))))
            .unwrap();
        Page { rx, tx }
    }

    fn next(&mut self) -> Message {
        self.rx.read_message().expect("a message from tvpipe")
    }

    /// The next text message of type `ty` (skipping others).
    fn text(&mut self, ty: &str) -> Value {
        loop {
            if let Message::Text(t) = self.next() {
                let v = Value::parse(&t).unwrap();
                if v.get("type").and_then(Value::as_str) == Some(ty) {
                    return v;
                }
            }
        }
    }

    /// The next binary message as a record.
    fn record(&mut self) -> crate::media::slrw::Packet {
        loop {
            match self.next() {
                Message::Binary(b) => return protocol::parse_record(&b).unwrap(),
                Message::Text(_) => {}
                Message::Close(c) => panic!("closed: {c:?}"),
            }
        }
    }

    /// Nothing arrives within `d`.
    fn quiet(&mut self, d: Duration) -> bool {
        self.rx
            .with_stream(|s| s.set_read_timeout(Some(d)))
            .unwrap();
        let r = match self.rx.read_message() {
            Err(e) if e.kind() == ErrorKind::Timeout => true,
            Ok(m) => {
                eprintln!("unexpected {m:?}");
                false
            }
            Err(e) => panic!("{e}"),
        };
        self.rx
            .with_stream(|s| s.set_read_timeout(Some(Duration::from_secs(10))))
            .unwrap();
        r
    }
}

/// Waits until the sink has `n` tabs.
fn wait_viewers(sink: &BrowserSink, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while sink.viewers() != n {
        assert!(Instant::now() < deadline, "{} viewers", sink.viewers());
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn started(clock: &Arc<MediaClock>) -> BrowserSink {
    let mut sink = BrowserSink::bind(&PlayerOptions::default(), never).unwrap();
    sink.set_clock(SessionClock::new(clock.clone()));
    sink
}

#[test]
fn codec_strings() {
    assert_eq!(codec_string(&avcc()).as_deref(), Some("avc1.42C01E"));
    assert_eq!(codec_string(&[1, 2]), None);
}

#[test]
fn config_message_fields() {
    let v =
        Value::parse(&config_message(&video_info(), Some(&audio_info()), 120, "browser").unwrap())
            .unwrap();
    assert_eq!(v.get("type").unwrap().as_str(), Some("config"));
    let vid = v.get("video").unwrap();
    assert_eq!(vid.get("codec").unwrap().as_str(), Some("avc1.42C01E"));
    assert_eq!(vid.get("width").unwrap().as_u32(), Some(320));
    let desc = vid.get("description").unwrap().as_str().unwrap();
    assert_eq!(crate::net::base64::decode(desc).unwrap(), avcc());
    let a = v.get("audio").unwrap();
    assert_eq!(a.get("sampleRate").unwrap().as_u32(), Some(48_000));
    assert_eq!(a.get("channels").unwrap().as_u32(), Some(2));
    assert_eq!(v.get("latency").unwrap().as_u32(), Some(120));
    // No avcC: Annex B in band, no description.
    let mut vi = video_info();
    vi.avcc.clear();
    let v = Value::parse(&config_message(&vi, None, 100, "b").unwrap()).unwrap();
    assert!(v.get("video").unwrap().get("description").is_none());
    assert!(v.get("audio").unwrap().is_null());
}

#[test]
fn audio_plans_with_airplay_receivers() {
    use crate::media::passthrough::sink_takes;
    let caps = capabilities(&PlayerOptions::default());
    assert_eq!(caps.audio_family, AudioFamily::AirPlay);
    // What the AirPlay family plans as PCM, at the TV's rate or ours.
    for rate in [44_100, 48_000] {
        let f = AudioFormat {
            codec: AudioCodec::PcmS16le,
            sample_rate: rate,
            channels: 2,
        };
        assert!(sink_takes(&caps, &f), "{rate}");
    }
    let no_audio = capabilities(&PlayerOptions {
        audio: false,
        ..PlayerOptions::default()
    });
    assert!(no_audio.audio_formats.is_empty());
}

fn http(addr: u16, raw: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", addr)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn page_server_checks_token_host_and_origin() {
    let sink = BrowserSink::bind(&PlayerOptions::default(), never).unwrap();
    let port = sink.shared.access.port();
    let token = sink.shared.access.token().to_owned();
    let get = |target: &str, host: &str, extra: &str| {
        http(
            port,
            &format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n"),
        )
    };
    let here = format!("127.0.0.1:{port}");
    let page = get(&format!("/?t={token}"), &here, "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("/player.js"));
    assert!(get("/player.js", &format!("localhost:{port}"), "").starts_with("HTTP/1.1 200"));
    assert!(get("/", &here, "").starts_with("HTTP/1.1 403"));
    assert!(get("/?t=00", &here, "").starts_with("HTTP/1.1 403"));
    assert!(get(&format!("/?t={token}"), "evil.example:80", "").starts_with("HTTP/1.1 421"));
    assert!(get(&format!("/?t={token}"), "127.0.0.1", "").starts_with("HTTP/1.1 421"));
    let upgrade = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
                   Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
    let ws = format!("/ws?t={token}");
    let r = get(
        &ws,
        &here,
        &format!("{upgrade}Origin: http://evil.example\r\n"),
    );
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");
    // Our own origin upgrades (only the head is read: the socket stays).
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {ws} HTTP/1.1\r\nHost: {here}\r\n{upgrade}Origin: http://{here}\r\n\r\n"
    )
    .unwrap();
    let mut head = [0u8; 12];
    s.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 101");
    assert!(get("/ws?t=1", &here, upgrade).starts_with("HTTP/1.1 403"));

    // --tunnel: the public host with its own token and origin.
    let public = "abc.opentunnel.xyz";
    assert!(get(&format!("/?t={token}"), public, "").starts_with("HTTP/1.1 421"));
    let ptoken = sink.shared.access.publish(public, None, None).unwrap();
    assert!(get(&format!("/?t={ptoken}"), public, "").starts_with("HTTP/1.1 200"));
    assert!(get(&format!("/?t={token}"), public, "").starts_with("HTTP/1.1 403"));
    assert!(get(&format!("/?t={ptoken}"), &here, "").starts_with("HTTP/1.1 403"));
    let pws = format!("/ws?t={ptoken}");
    for origin in [
        format!("http://{here}"),
        format!("http://{public}"),
        "https://evil.example".into(),
    ] {
        let r = get(&pws, public, &format!("{upgrade}Origin: {origin}\r\n"));
        assert!(r.starts_with("HTTP/1.1 403"), "{origin}: {r}");
    }
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {pws} HTTP/1.1\r\nHost: {public}\r\n{upgrade}Origin: https://{public}\r\n\r\n"
    )
    .unwrap();
    s.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 101");
}

#[test]
fn streams_config_and_records_on_the_session_clock() {
    let clock = Arc::new(MediaClock::new());
    let mut sink = started(&clock);
    let mut page = Page::connect(&sink);
    let st = page.text("status");
    assert_eq!(st.get("state").unwrap().as_str(), Some("waiting"));
    wait_viewers(&sink, 1);
    // Nothing configured yet: the tab waits, the sink wants a keyframe
    // for it.
    assert!(sink.wants_keyframe());
    sink.start(&video_info(), Some(&audio_info())).unwrap();
    let cfg = page.text("config");
    assert_eq!(cfg.get("version").unwrap().as_u32(), Some(VERSION));
    assert!(cfg.get("audio").is_some_and(|a| !a.is_null()));

    // The fanout schedules frames on the shared clock and pins them; the
    // records carry those instants in tvpipe's µs.
    let t0 = Instant::now() + Duration::from_millis(50);
    clock.reanchor_at(0, VIDEO_CLOCK_RATE, t0);
    assert_eq!(sink.push(&video(0, false, 1)).unwrap(), SinkStatus::Dropped);
    assert_eq!(
        sink.push(&video(3000, true, 2)).unwrap(),
        SinkStatus::Accepted
    );
    assert!(!sink.wants_keyframe());
    assert_eq!(sink.push(&audio(1600)).unwrap(), SinkStatus::Accepted);
    assert_eq!(
        sink.push(&video(6000, false, 3)).unwrap(),
        SinkStatus::Accepted
    );

    let key = page.record();
    assert_eq!(key.kind, Kind::H264);
    assert_eq!(key.payload[0], slrw::H264_KEYFRAME);
    assert_eq!(&key.payload[1..], &video(3000, true, 2).data[..]);
    let at = |pts| {
        sink.shared
            .us_of(t0 + crate::util::time::ticks_to_duration(pts, 90_000))
    };
    assert_eq!(key.pts_us, at(3000));
    let a = page.record();
    assert_eq!(a.kind, Kind::Audio);
    assert_eq!(a.payload.len(), 480 * 4);
    assert_eq!(a.pts_us, at(3000));
    let delta = page.record();
    assert_eq!(delta.payload[0], 0);
    assert_eq!(delta.pts_us, at(6000));

    // Clock sync: the page's number comes back with tvpipe's clock.
    page.tx.send_text(r#"{"type":"ping","t":1234.5}"#).unwrap();
    let pong = page.text("pong");
    assert_eq!(pong.get("t").unwrap().as_f64(), Some(1234.5));
    let now = pong.get("now").unwrap().as_u64().unwrap();
    assert!(now > 0 && now <= sink.shared.now_us());

    sink.stop().unwrap();
    let end = page.text("end");
    assert_eq!(
        end.get("reason").unwrap().as_str(),
        Some("the stream ended")
    );
    assert!(matches!(page.next(), Message::Close(_)));
    sink.stop().unwrap();
}

#[test]
fn late_joiners_wait_for_a_keyframe_without_freezing_others() {
    let clock = Arc::new(MediaClock::new());
    let mut sink = started(&clock);
    // No tab: frames are dropped, nobody wants a keyframe.
    sink.start(&video_info(), None).unwrap();
    assert!(!sink.wants_keyframe());
    assert_eq!(sink.push(&video(0, true, 0)).unwrap(), SinkStatus::Dropped);
    // Audio without an audio track is not sent.
    assert_eq!(sink.push(&audio(0)).unwrap(), SinkStatus::Dropped);

    let mut a = Page::connect(&sink);
    a.text("config");
    wait_viewers(&sink, 1);
    assert!(sink.wants_keyframe(), "the only tab waits");
    assert_eq!(
        sink.push(&video(3000, false, 1)).unwrap(),
        SinkStatus::Dropped
    );
    sink.push(&video(6000, true, 2)).unwrap();
    assert_eq!(a.record().payload[0], slrw::H264_KEYFRAME);

    // B joins while A plays: config at once, no keyframe request (that
    // would freeze A), deltas only after the next keyframe.
    let mut b = Page::connect(&sink);
    let st = b.text("status");
    assert_eq!(st.get("state").unwrap().as_str(), Some("streaming"));
    b.text("config");
    wait_viewers(&sink, 2);
    assert!(!sink.wants_keyframe());
    sink.push(&video(9000, false, 3)).unwrap();
    assert_eq!(a.record().payload[1..], video(9000, false, 3).data[..]);
    assert!(b.quiet(Duration::from_millis(200)));
    sink.push(&video(12000, true, 4)).unwrap();
    sink.push(&video(15000, false, 5)).unwrap();
    for p in [&mut a, &mut b] {
        let r = p.record();
        assert_eq!(r.payload[0], slrw::H264_KEYFRAME);
        assert_eq!(r.payload[1..], video(12000, true, 4).data[..]);
        assert_eq!(p.record().payload[1..], video(15000, false, 5).data[..]);
    }

    // A decoder that fails asks for a keyframe: that tab skips deltas;
    // once every tab waits, the sink asks the input.
    a.tx.send_text(r#"{"type":"keyframe","reason":"test"}"#)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while sink
        .shared
        .lock()
        .tabs
        .iter()
        .filter(|t| t.waiting_key)
        .count()
        != 1
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!sink.wants_keyframe());
    b.tx.send_text(r#"{"type":"keyframe","reason":"test"}"#)
        .unwrap();
    while !sink.wants_keyframe() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }

    // A tab leaving is just gone; the session goes on.
    drop(b);
    wait_viewers(&sink, 1);
    sink.stop().unwrap();
}

#[test]
fn a_stalled_tab_never_blocks_push() {
    let clock = Arc::new(MediaClock::new());
    let mut sink = started(&clock);
    // A client that never reads.
    let page = Page::connect(&sink);
    wait_viewers(&sink, 1);
    sink.start(&video_info(), None).unwrap();
    let big = |pts: u64, key: bool| {
        let mut data = vec![0u8; 1 << 20];
        data[..5].copy_from_slice(&[0, 0x0f, 0xff, 0xfc, if key { 0x65 } else { 0x41 }]);
        EncodedFrame {
            track: TrackKind::Video,
            pts,
            keyframe: key,
            data: data.into(),
        }
    };
    let t0 = Instant::now();
    for i in 0..64u64 {
        sink.push(&big(i * 3000, i % 8 == 0)).unwrap();
    }
    // Far more than a socket buffer and the tab's queue: some were
    // dropped, and pushing did not wait for the socket.
    let dropped = sink.shared.lock().tabs[0].dropped;
    assert!(dropped > 0, "nothing dropped");
    assert!(t0.elapsed() < Duration::from_secs(30), "{:?}", t0.elapsed());
    sink.stop().unwrap();
    drop(page);
}

/// A page that acknowledges what it got (`ack`) has at most
/// MAX_IN_FLIGHT unacknowledged bytes: a viewer whose acks stop (a slow
/// link behind a tunnel) skips frames, the others get every one.
#[test]
fn acknowledging_tabs_have_a_limited_amount_in_flight() {
    let clock = Arc::new(MediaClock::new());
    let mut sink = started(&clock);
    let mut a = Page::connect(&sink);
    let mut b = Page::connect(&sink);
    wait_viewers(&sink, 2);
    sink.start(&video_info(), None).unwrap();
    a.text("config");
    b.text("config");
    let sized = |pts: u64, key: bool, n: usize| {
        let mut data = vec![0u8; n];
        let len = (n - 4) as u32;
        data[..4].copy_from_slice(&len.to_be_bytes());
        data[4] = if key { 0x65 } else { 0x41 };
        EncodedFrame {
            track: TrackKind::Video,
            pts,
            keyframe: key,
            data: data.into(),
        }
    };
    let shared = sink.shared.clone();
    let acked = |i: usize| shared.lock().tabs[i].queue.acked.load(Ordering::Relaxed);
    let wait_acked = |i: usize, want: u64| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while acked(i) != want || !shared.lock().tabs[i].queue.acking.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "tab {i}: acked {}", acked(i));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let ack = |p: &Page, bytes: u64| {
        p.tx.send_text(&format!(r#"{{"type":"ack","bytes":{bytes}}}"#))
            .unwrap();
    };
    ack(&a, 0);
    ack(&b, 0);
    wait_acked(0, 0);
    wait_acked(1, 0);
    // Record bytes: 13 of header, 1 of flags, the access unit.
    let record = |n: usize| (n + 14) as u64;
    let (mut a_got, mut b_got) = (0u64, 0u64);
    sink.push(&sized(0, true, 100_000)).unwrap();
    assert!(a.record().payload[0] & slrw::H264_KEYFRAME != 0);
    assert!(b.record().payload[0] & slrw::H264_KEYFRAME != 0);
    a_got += record(100_000);
    b_got += record(100_000);
    ack(&a, a_got);
    wait_acked(0, a_got);
    let mut b_frames = 0;
    for i in 1..=8u64 {
        sink.push(&sized(i * 3000, false, 64 << 10)).unwrap();
        a.record();
        a_got += record(64 << 10);
        ack(&a, a_got);
        wait_acked(0, a_got);
        if b_got <= MAX_IN_FLIGHT as u64 {
            b.record();
            b_got += record(64 << 10);
            b_frames += 1;
        }
    }
    // B got the keyframe and deltas up to the limit, then nothing.
    assert_eq!(b_frames, 3);
    assert!(b.quiet(Duration::from_millis(200)));
    assert!(sink.shared.lock().tabs[1].waiting_key);
    assert!(sink.shared.lock().tabs[1].dropped > 0);
    // Once B catches up, it resumes at the next keyframe.
    ack(&b, b_got);
    wait_acked(1, b_got);
    sink.push(&sized(30_000, false, 1000)).unwrap();
    sink.push(&sized(33_000, true, 1000)).unwrap();
    assert!(b.record().payload[0] & slrw::H264_KEYFRAME != 0);
    assert_eq!(a.record().payload[0] & slrw::H264_KEYFRAME, 0);
    sink.stop().unwrap();
}

#[test]
fn waits_for_a_viewer_or_a_stop() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STOP: AtomicBool = AtomicBool::new(false);
    fn stop() -> bool {
        STOP.load(Ordering::SeqCst)
    }
    let opts = PlayerOptions {
        wait_for_viewer: true,
        ..PlayerOptions::default()
    };
    let h = std::thread::spawn(move || BrowserSink::bind(&opts, stop).map(|s| s.url().to_owned()));
    std::thread::sleep(Duration::from_millis(100));
    STOP.store(true, Ordering::SeqCst);
    assert!(h.join().unwrap().is_err());
}

#[test]
fn annexb_when_the_input_has_no_avcc() {
    let clock = Arc::new(MediaClock::new());
    let mut sink = started(&clock);
    let mut page = Page::connect(&sink);
    wait_viewers(&sink, 1);
    let mut vi = video_info();
    vi.avcc.clear();
    sink.start(&vi, None).unwrap();
    page.text("config");
    sink.push(&video(0, true, 7)).unwrap();
    let r = page.record();
    assert_eq!(r.payload[0], slrw::H264_KEYFRAME | slrw::H264_ANNEX_B);
    assert_eq!(&r.payload[1..], &[0, 0, 0, 1, 0x65, 7, 7]);
    sink.stop().unwrap();
}

#[test]
fn the_token_is_kept_and_rotated() {
    let dir = crate::airplay::credentials::tests::temp_dir("player-token");
    let path = dir.join(TOKEN_FILE);
    let t1 = token_in(&path, false).unwrap();
    assert!(valid_token(&t1), "{t1}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("{t1}\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(token_in(&path, false).unwrap(), t1);
    let t2 = token_in(&path, true).unwrap();
    assert_ne!(t2, t1);
    assert_eq!(token_in(&path, false).unwrap(), t2);
    // Anything else in the file is replaced.
    std::fs::write(&path, "../../etc\n").unwrap();
    let t3 = token_in(&path, false).unwrap();
    assert!(valid_token(&t3) && t3 != t2);
    assert!(!valid_token("ABCDEF0123456789abcdef0123456789"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_url_uses_the_given_token_and_the_fixed_port_or_another() {
    let token = "0123456789abcdef0123456789abcdef".to_owned();
    // Taken (by us, or by whoever has it): the sink binds another port.
    let held = std::net::TcpListener::bind(("127.0.0.1", DEFAULT_PORT));
    let opts = PlayerOptions {
        port: None,
        token: Some(token.clone()),
        ..PlayerOptions::default()
    };
    let sink = BrowserSink::bind(&opts, never).unwrap();
    assert_ne!(sink.shared.access.port(), DEFAULT_PORT);
    assert!(
        sink.url().ends_with(&format!("/?t={token}")),
        "{}",
        sink.url()
    );
    drop(held);
    // An explicit port that is taken is an error.
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    let opts = PlayerOptions {
        port: Some(port),
        ..PlayerOptions::default()
    };
    assert!(BrowserSink::bind(&opts, never).is_err());
}
