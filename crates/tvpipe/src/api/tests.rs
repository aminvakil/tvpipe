//! The public API against the browser receiver (no network, no TV).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::events::{EndReason, ReceiverState, Route};
use crate::Storage;

fn browser() -> Options {
    Options::new()
        .to("browser")
        .storage(Storage::memory())
        .player(PlayerOptions::new().port(0))
}

fn bars(w: usize, h: usize, i: usize) -> Vec<u8> {
    let mut f = vec![0u8; w * h * 4];
    for (n, px) in f.chunks_mut(4).enumerate() {
        let x = n % w;
        px.copy_from_slice(&[(x * 255 / w) as u8, (i * 8) as u8, 128, 255]);
    }
    f
}

#[test]
fn stream_raw_frames_and_audio_to_a_browser_receiver() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let mut s = stream(
        browser()
            .video(160, 90, 30, PixelFormat::Rgba)
            .audio(48_000, 2)
            .on_event(move |e| s2.lock().unwrap().push(e.type_name())),
    )
    .unwrap();
    assert!(s.player_url().unwrap().starts_with("http://127.0.0.1:"));
    let t0 = Instant::now();
    let mut queued = 0;
    let mut i = 0;
    while t0.elapsed() < Duration::from_millis(1500) {
        let pts = s.elapsed();
        if s.write_video(&bars(160, 90, i), pts).unwrap() == Written::Queued {
            queued += 1;
        }
        s.write_audio(&vec![0i16; 1600 * 2], pts).unwrap();
        i += 1;
        std::thread::sleep(Duration::from_millis(33));
    }
    assert!(queued > 10, "{queued}");
    let started = s.started().expect("started");
    assert_eq!(started.route, Route::Builtin);
    assert_eq!((started.video.width, started.video.height), (160, 90));
    let r = s.receivers();
    assert_eq!(r[0].name, "browser");
    assert_eq!(r[0].state, ReceiverState::Connected);
    let events = s.poll_events();
    assert!(events.iter().any(|e| matches!(e, Event::Connected(_))));
    // The bytes behind a bitrate (not on the machine interface's wire).
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Stats(st) if st.video_bytes > 0)),
        "{events:?}"
    );
    // A wrong size is the caller's error, not a panic.
    assert!(s.write_video(&[0; 10], s.elapsed()).is_err());
    let end = s.finish().unwrap();
    assert_eq!(end.reason, EndReason::Eof);
    let seen = seen.lock().unwrap();
    assert!(seen.contains(&"started"), "{seen:?}");
    assert!(seen.contains(&"end"), "{seen:?}");
}

#[test]
fn padded_bottom_up_frames_are_packed() {
    let mut s = stream(browser().video(4, 2, 30, PixelFormat::Rgba)).unwrap();
    // Two rows of 16 bytes, 20 bytes apart.
    let mut buf = vec![0u8; 36];
    buf[..16].fill(1);
    buf[20..36].fill(2);
    let f = Frame::new(&buf, 20).bottom_up(true);
    assert_eq!(
        s.write_video_frame(f, Duration::ZERO).unwrap(),
        Written::Queued
    );
    let rec = s.last_record().unwrap();
    assert_eq!(&rec[13..29], &[2u8; 16]);
    assert_eq!(&rec[29..45], &[1u8; 16]);
    let short = Frame::new(&buf[..30], 20);
    assert!(s
        .write_video_frame(short, Duration::from_millis(100))
        .is_err());
    s.stop().unwrap();
}

#[test]
fn frame_buffers_are_filled_in_place_and_reused() {
    let mut s = stream(browser().video(4, 2, 30, PixelFormat::Rgba)).unwrap();
    let mut b = s.frame_buffer();
    assert_eq!(b.len(), 32);
    b.fill(7);
    assert_eq!(s.submit(b, Duration::ZERO).unwrap(), Written::Queued);
    let rec = s.last_record().unwrap();
    assert_eq!((rec[0], &rec[13..]), (1, &[7u8; 32][..]));
    // Too soon for the frame rate: dropped, the buffer goes back.
    let b = s.frame_buffer();
    assert_eq!(
        s.submit(b, Duration::from_millis(1)).unwrap(),
        Written::Dropped
    );
    let mut other = stream(browser().video(2, 2, 30, PixelFormat::Rgba)).unwrap();
    let b = other.frame_buffer();
    assert!(s.submit(b, Duration::from_secs(1)).is_err());
    other.stop().unwrap();
    s.stop().unwrap();
}

#[test]
fn test_pattern_session_stops() {
    let mut s = test_pattern(browser().size(160, 90)).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while s.started().is_none() && Instant::now() < until {
        let _ = s.next_event(Duration::from_millis(100));
    }
    assert_eq!(s.state(), SessionState::Streaming);
    let stopper = s.stopper();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stopper.stop();
    });
    let end = s.wait().unwrap();
    assert_eq!(end.reason, EndReason::Stopped);
}

#[test]
fn h264_stream_needs_h264_writes() {
    let mut s = Stream::start(browser().h264(160, 90, 30)).unwrap();
    assert!(s.write_video(&[], Duration::ZERO).is_err());
    assert!(!s.wants_frame());
    drop(s);
    assert!(Stream::start(browser()).is_err());
}

#[test]
fn feature_off_is_a_coded_error() {
    let e = crate::error::Error::feature("cast", "cast:TV");
    assert_eq!(e.code(), Some("feature-disabled"));
}

/// SLRW records as the writers make them (`tests/vectors/slrw/records.txt`),
/// for the Node SDK's encoder; `TVPIPE_BLESS=1` rewrites the file.
#[test]
fn slrw_record_vectors() {
    use crate::media::slrw::{self, Kind};
    use crate::util::hex;
    let mut blocks: Vec<(String, Vec<(&str, String)>)> = Vec::new();
    let mut rec = |name: &str, fields: Vec<(&'static str, String)>, bytes: Vec<u8>| {
        let mut f = fields;
        f.push(("bytes", hex::encode(&bytes)));
        blocks.push((name.to_owned(), f));
    };
    let mut magic = Vec::new();
    slrw::write_magic(&mut magic).unwrap();
    rec("magic", vec![], magic);
    for (name, kind, pts, payload) in [
        ("video", Kind::Video, 0x0102u64, vec![0x61u8, 0x62]),
        ("audio", Kind::Audio, 21_333, vec![1, 0, 0xff, 0x7f]),
        ("end", Kind::End, 0, vec![]),
    ] {
        let mut b = Vec::new();
        slrw::write_packet(&mut b, kind, pts, &payload).unwrap();
        rec(
            name,
            vec![
                ("kind", kind.code().to_string()),
                ("pts_us", pts.to_string()),
                ("payload", hex::encode(&payload)),
            ],
            b,
        );
    }
    let au = [0u8, 0, 0, 1, 0x65, 0x88];
    let mut b = Vec::new();
    slrw::write_h264(&mut b, 0x10, true, true, &au).unwrap();
    rec(
        "h264 keyframe annex b",
        vec![
            ("kind", "3".into()),
            ("pts_us", "16".into()),
            (
                "prefix",
                format!("{}", slrw::H264_KEYFRAME | slrw::H264_ANNEX_B),
            ),
            ("payload", hex::encode(&au)),
        ],
        b,
    );
    let avcc = [0u8, 0, 0, 2, 0x41, 0x9a];
    let mut b = Vec::new();
    slrw::write_h264(&mut b, 33_333, false, false, &avcc).unwrap();
    rec(
        "h264 delta avcc",
        vec![
            ("kind", "3".into()),
            ("pts_us", "33333".into()),
            ("prefix", "0".into()),
            ("payload", hex::encode(&avcc)),
        ],
        b,
    );
    let cfg = [1u8, 0x64, 0, 0x28, 0xff];
    let mut b = Vec::new();
    slrw::write_h264_config(&mut b, slrw::CONFIG_AVCC, &cfg).unwrap();
    rec(
        "h264-config avcc",
        vec![
            ("kind", "4".into()),
            ("pts_us", "0".into()),
            ("prefix", "0".into()),
            ("payload", hex::encode(&cfg)),
        ],
        b,
    );
    let mut b = Vec::new();
    slrw::write_control(&mut b, slrw::CONTROL_KEYFRAME_REQUESTS).unwrap();
    rec("control keyframe requests", vec![("flags", "1".into())], b);
    // Kind 6 exists only with --machine (docs/machine.md).
    let json = r#"{"type":"stop"}"#;
    let mut b = super::stream::record(Kind::End, Duration::ZERO, json.len());
    b[0] = 6;
    b.extend_from_slice(json.as_bytes());
    rec("command stop", vec![("command", json.into())], b);
    let floats = [0.0f32, 1.0, -1.0, 0.5, 2.0, -2.0, -0.5];
    let pcm: Vec<u8> = floats
        .iter()
        .flat_map(|&f| super::stream::f32_to_i16(f).to_le_bytes())
        .collect();
    let mut b = Vec::new();
    slrw::write_packet(&mut b, Kind::Audio, 0, &pcm).unwrap();
    rec(
        "float audio",
        vec![
            ("kind", "2".into()),
            ("pts_us", "0".into()),
            (
                "f32",
                floats
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        ],
        b,
    );
    let mut text = String::from(
        "# SLRW records (docs/slrw.md) as tvpipe's writers make them, for the Node\n\
         # SDK's encoder. Generated by crates/tvpipe/src/api/tests.rs\n\
         # (TVPIPE_BLESS=1 cargo test -p tvpipe slrw_record_vectors).\n",
    );
    for (name, fields) in &blocks {
        text.push_str(&format!("\nname = {name}\n"));
        for (k, v) in fields {
            text.push_str(&format!("{k} = {v}\n"));
        }
    }
    let path = crate::util::vectors::path("tests/vectors/slrw/records.txt");
    if std::env::var_os("TVPIPE_BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_default(),
        text,
        "{} is out of date (TVPIPE_BLESS=1)",
        path.display()
    );
}
