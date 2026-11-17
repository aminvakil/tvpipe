//! Sample SLRW generator: writes moving color bars and a sine tone, paced
//! in real time, to stdout. Feed it to streamlib's game input:
//!
//! ```text
//! cargo run --example slrw-gen -- --size 640x360 --fps 30 --seconds 5 |
//!     streamlib debug-pipeline --raw-video 640x360@30:rgba --raw-audio 48000:2:s16le -
//! ```
//!
//! Options: `--size WxH` (default 640x360), `--fps N` (30), `--rate HZ`
//! (48000), `--channels N` (2), `--tone HZ` (440), `--seconds S` (5; 0
//! runs forever), `--no-audio`, `--no-pace` (write as fast as possible).
//! Video is RGBA; audio is s16le, one record per video frame.

#[path = "../src/media/slrw.rs"]
mod slrw;

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use slrw::Kind;

struct Opts {
    width: u32,
    height: u32,
    fps: u32,
    rate: u32,
    channels: u32,
    tone: f64,
    seconds: f64,
    audio: bool,
    pace: bool,
}

fn parse() -> Result<Opts, String> {
    let mut o = Opts {
        width: 640,
        height: 360,
        fps: 30,
        rate: 48_000,
        channels: 2,
        tone: 440.0,
        seconds: 5.0,
        audio: true,
        pace: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or_else(|| format!("{a} needs a value"));
        let num = |v: String| v.parse::<f64>().map_err(|_| format!("bad number {v:?}"));
        match a.as_str() {
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("expected WxH")?;
                o.width = w.parse().map_err(|_| "bad width")?;
                o.height = h.parse().map_err(|_| "bad height")?;
            }
            "--fps" => o.fps = num(val()?)? as u32,
            "--rate" => o.rate = num(val()?)? as u32,
            "--channels" => o.channels = num(val()?)? as u32,
            "--tone" => o.tone = num(val()?)?,
            "--seconds" => o.seconds = num(val()?)?,
            "--no-audio" => o.audio = false,
            "--no-pace" => o.pace = false,
            "-h" | "--help" => {
                return Err(
                    "usage: slrw-gen [--size WxH] [--fps N] [--rate HZ] [--channels N] \
                            [--tone HZ] [--seconds S] [--no-audio] [--no-pace]"
                        .into(),
                )
            }
            _ => return Err(format!("unknown option {a}")),
        }
    }
    if o.width == 0 || o.height == 0 || o.fps == 0 || o.rate == 0 || o.channels == 0 {
        return Err("sizes and rates must be positive".into());
    }
    Ok(o)
}

/// SMPTE-like bars scrolling left by `shift` pixels, with a white box that
/// moves down so dropped frames are visible.
fn frame(o: &Opts, n: u64, buf: &mut [u8]) {
    const BARS: [[u8; 3]; 7] = [
        [192, 192, 192],
        [192, 192, 0],
        [0, 192, 192],
        [0, 192, 0],
        [192, 0, 192],
        [192, 0, 0],
        [0, 0, 192],
    ];
    let (w, h) = (o.width as u64, o.height as u64);
    let shift = n * 4;
    let box_y = (n * 3) % h.max(1);
    let size = (h / 8).max(4);
    for y in 0..h {
        for x in 0..w {
            let bar = (((x + shift) % w) * 7 / w) as usize;
            let mut px = BARS[bar.min(6)];
            if y >= box_y && y < box_y + size && x < size {
                px = [255, 255, 255];
            }
            let i = ((y * w + x) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
}

fn run(o: &Opts) -> io::Result<()> {
    let out = io::stdout();
    let mut out = BufWriter::with_capacity(1 << 20, out.lock());
    slrw::write_magic(&mut out)?;
    let mut video = vec![0u8; (o.width * o.height * 4) as usize];
    let mut audio = Vec::new();
    let mut samples_done = 0u64;
    let start = Instant::now();
    let total = if o.seconds > 0.0 {
        Some((o.seconds * o.fps as f64).round() as u64)
    } else {
        None
    };
    let mut n = 0u64;
    while total.is_none_or(|t| n < t) {
        let pts_us = n * 1_000_000 / o.fps as u64;
        frame(o, n, &mut video);
        slrw::write_packet(&mut out, Kind::Video, pts_us, &video)?;
        if o.audio {
            // Samples due by the end of this frame, so the total is exact.
            let due = (n + 1) * o.rate as u64 / o.fps as u64;
            audio.clear();
            for s in samples_done..due {
                let t = s as f64 / o.rate as f64;
                let v = ((t * o.tone * std::f64::consts::TAU).sin() * 8000.0) as i16;
                for _ in 0..o.channels {
                    audio.extend_from_slice(&v.to_le_bytes());
                }
            }
            let audio_pts = samples_done * 1_000_000 / o.rate as u64;
            slrw::write_packet(&mut out, Kind::Audio, audio_pts, &audio)?;
            samples_done = due;
        }
        out.flush()?;
        n += 1;
        if o.pace {
            let next = start + Duration::from_micros(n * 1_000_000 / o.fps as u64);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }
    slrw::write_packet(&mut out, Kind::End, n * 1_000_000 / o.fps as u64, &[])?;
    out.flush()
}

fn main() -> ExitCode {
    let o = match parse() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("slrw-gen: {e}");
            return ExitCode::from(2);
        }
    };
    match run(&o) {
        Ok(()) => ExitCode::SUCCESS,
        // The consumer went away.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("slrw-gen: {e}");
            ExitCode::from(1)
        }
    }
}
