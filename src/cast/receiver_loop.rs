//! Opt-in loop against a real receiver (openscreen's `cast_receiver`, or
//! hardware): streams ffmpeg-generated H.264 (and Opus) through
//! [`StreamingSink`]. See `scripts/receivers/cast-receiver.sh`.
//!
//! ```sh
//! STREAMLIB_CAST_ADDR=127.0.0.1:8010 STREAMLIB_CAST_SECS=10 \
//!     cargo test cast::receiver_loop -- --ignored --nocapture
//! ```
//!
//! The ffmpeg output is parsed here only because the W4 demuxer is not
//! merged yet: Annex B H.264 with access unit delimiters, and Ogg Opus.

use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::sink::{connect, CastOptions};
use crate::types::{
    AudioCodec, AudioFamily, AudioInfo, EncodedFrame, Sink, SinkStatus, TrackKind, VideoCodec,
    VideoInfo,
};
use crate::util::ratio::Ratio;

fn ffmpeg() -> String {
    std::env::var("STREAMLIB_FFMPEG").unwrap_or_else(|_| {
        if std::path::Path::new("/opt/homebrew/bin/ffmpeg").exists() {
            "/opt/homebrew/bin/ffmpeg".into()
        } else {
            "ffmpeg".into()
        }
    })
}

/// Splits an Annex B stream into NAL units.
fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&n| n - 3);
        while end > s && data[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&data[s..end]);
        }
    }
    out
}

/// `(avcC, [(keyframe, AVCC access unit)])` from an Annex B stream with
/// access unit delimiters.
pub fn h264_frames(annexb: &[u8]) -> (Vec<u8>, Vec<(bool, Vec<u8>)>) {
    let mut sps = None;
    let mut pps = None;
    let mut frames = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut key = false;
    for nal in nal_units(annexb) {
        match nal[0] & 0x1f {
            9 => {
                if !cur.is_empty() {
                    frames.push((key, std::mem::take(&mut cur)));
                }
                key = false;
                continue;
            }
            7 => {
                sps.get_or_insert_with(|| nal.to_vec());
                continue;
            }
            8 => {
                pps.get_or_insert_with(|| nal.to_vec());
                continue;
            }
            5 => key = true,
            _ => {}
        }
        cur.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        cur.extend_from_slice(nal);
    }
    if !cur.is_empty() {
        frames.push((key, cur));
    }
    let sps = sps.expect("no SPS");
    let pps = pps.expect("no PPS");
    let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(&sps);
    avcc.push(1);
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(&pps);
    (avcc, frames)
}

/// Opus packets from an Ogg stream (skipping the two header packets).
pub fn ogg_opus_packets(data: &[u8]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut cur = Vec::new();
    let mut pos = 0;
    while pos + 27 <= data.len() && &data[pos..pos + 4] == b"OggS" {
        let nseg = data[pos + 26] as usize;
        let segs = &data[pos + 27..pos + 27 + nseg];
        let mut body = pos + 27 + nseg;
        for &len in segs {
            cur.extend_from_slice(&data[body..body + len as usize]);
            body += len as usize;
            if len < 255 {
                packets.push(std::mem::take(&mut cur));
            }
        }
        pos = body;
    }
    packets.into_iter().skip(2).collect()
}

fn generate(secs: u64, size: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("streamlib-cast-loop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let h264 = dir.join("v.h264");
    let opus = dir.join("a.opus");
    let t = secs.to_string();
    let status = Command::new(ffmpeg())
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={size}:rate=30"),
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", &t, "-map", "0:v"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-tune",
            "zerolatency",
        ])
        .args([
            "-profile:v",
            "high",
            "-pix_fmt",
            "yuv420p",
            "-bf",
            "0",
            "-g",
            "30",
        ])
        .args(["-b:v", "4M", "-x264-params", "aud=1", "-f", "h264"])
        .arg(&h264)
        .args(["-t", &t, "-map", "1:a", "-ac", "2"])
        .args(["-c:a", "libopus", "-b:a", "128k", "-frame_duration", "20"])
        .args(["-application", "lowdelay", "-f", "ogg"])
        .arg(&opus)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed");
    let out = (std::fs::read(&h264).unwrap(), std::fs::read(&opus).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[test]
fn parses_generated_streams() {
    if Command::new(ffmpeg()).arg("-version").output().is_err() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let (v, a) = generate(1, "320x240");
    let (avcc, frames) = h264_frames(&v);
    assert_eq!(frames.len(), 30);
    assert!(frames[0].0);
    assert_eq!(frames.iter().filter(|f| f.0).count(), 1);
    assert!(super::annexb::AvcConfig::parse(&avcc).is_ok());
    let packets = ogg_opus_packets(&a);
    assert!((49..=52).contains(&packets.len()), "{}", packets.len());
}

#[test]
#[ignore = "needs a Cast receiver (scripts/receivers/cast-receiver.sh)"]
fn openscreen_receiver_loop() {
    let addr: SocketAddr = std::env::var("STREAMLIB_CAST_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8010".into())
        .parse()
        .unwrap();
    let secs: u64 = std::env::var("STREAMLIB_CAST_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let audio = std::env::var("STREAMLIB_CAST_AUDIO").map_or(true, |v| v != "0");
    crate::log::set_level(crate::log::Level::Debug);
    let (v, a) = generate(secs, "1280x720");
    let (avcc, vframes) = h264_frames(&v);
    let apackets = ogg_opus_packets(&a);

    let opts = CastOptions {
        latency_ms: 400,
        max_width: 1280,
        max_height: 720,
        max_fps: Ratio::new(30, 1),
        max_bitrate_kbps: 5000,
        audio,
        ..CastOptions::default()
    };
    let mut sink = connect(addr, "cast-receiver", &opts).expect("connect");
    eprintln!("capabilities: {:?}", sink.capabilities());
    sink.start(
        &VideoInfo {
            codec: VideoCodec::H264,
            width: 1280,
            height: 720,
            fps: Ratio::new(30, 1),
            avcc,
        },
        audio
            .then(|| AudioInfo {
                codec: AudioCodec::Opus,
                sample_rate: 48_000,
                channels: 2,
                frame_samples: 960,
                config: Vec::new(),
            })
            .as_ref(),
    )
    .expect("start");

    let start = Instant::now();
    let mut ai = 0usize;
    let mut dropped = 0;
    for (i, (key, data)) in vframes.into_iter().enumerate() {
        let due = start + Duration::from_micros(i as u64 * 33_333);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        let st = sink
            .push(&EncodedFrame {
                track: TrackKind::Video,
                pts: i as u64 * 3000,
                keyframe: key,
                data: Arc::from(data),
            })
            .unwrap();
        match st {
            SinkStatus::Finished => panic!("receiver ended the session"),
            SinkStatus::Dropped => dropped += 1,
            SinkStatus::Accepted => {}
        }
        while audio && ai < apackets.len() && ai * 20 <= (i + 1) * 33 {
            sink.push(&EncodedFrame {
                track: TrackKind::Audio(AudioFamily::Cast),
                pts: ai as u64 * 960,
                keyframe: true,
                data: Arc::from(apackets[ai].clone()),
            })
            .unwrap();
            ai += 1;
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    eprintln!("stats: {:?}, dropped video {dropped}", sink.stats());
    sink.stop().unwrap();
}
