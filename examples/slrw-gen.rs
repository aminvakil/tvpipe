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
//!
//! `--encoded` sends H.264 instead (decision 0007), the way a game with
//! its own hardware encoder would. This example uses ffmpeg (libx264,
//! no B-frames) as that encoder, so it needs ffmpeg in `PATH` (or
//! `$STREAMLIB_FFMPEG`); streamlib itself then runs without ffmpeg for
//! video:
//!
//! ```text
//! cargo run --example slrw-gen -- --encoded --no-audio |
//!     streamlib play --to TV --raw-video 640x360@30:h264 -
//! ```
//!
//! `--exec PROGRAM ARGS...` (the rest of the command line) runs streamlib
//! as a child, like a game would: SLRW goes to its stdin and keyframe
//! requests come back on its stdout. On a request the example restarts its
//! encoder, which starts with an IDR (an engine would call its encoder's
//! "force IDR" instead):
//!
//! ```text
//! cargo run --example slrw-gen -- --encoded --gop 300 --exec \
//!     streamlib play --to TV --raw-video 640x360@30:h264 --raw-audio 48000:2:s16le -
//! ```
//!
//! `--gop N` sets the encoder's keyframe interval in frames (default 2 s).

#[path = "../src/media/slrw.rs"]
mod slrw;

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
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
    encoded: bool,
    gop: Option<u32>,
    exec: Vec<String>,
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
        encoded: false,
        gop: None,
        exec: Vec::new(),
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
            "--encoded" => o.encoded = true,
            "--gop" => o.gop = Some(num(val()?)? as u32),
            "--exec" => {
                o.exec = args.by_ref().collect();
                if o.exec.is_empty() {
                    return Err("--exec needs a program".into());
                }
            }
            "-h" | "--help" => {
                return Err(
                    "usage: slrw-gen [--size WxH] [--fps N] [--rate HZ] [--channels N] \
                            [--tone HZ] [--seconds S] [--no-audio] [--no-pace] \
                            [--encoded [--gop N]] [--exec PROGRAM ARGS...]"
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

fn ffmpeg_bin() -> String {
    std::env::var("STREAMLIB_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

/// NAL unit types in an Annex B buffer.
fn nal_types(au: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            out.push(au[i + 3] & 0x1f);
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// An ffmpeg libx264 process turning RGBA frames into Annex B access
/// units (AUD-delimited, so a unit is complete when the next begins).
struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    units: Receiver<Vec<u8>>,
    /// pts of frames written but not yet returned as access units.
    pending: VecDeque<u64>,
}

impl Encoder {
    fn start(o: &Opts) -> io::Result<Encoder> {
        let gop = o.gop.unwrap_or(o.fps * 2).max(1).to_string();
        let mut child = Command::new(ffmpeg_bin())
            .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
            .args(["-pixel_format", "rgba", "-video_size"])
            .arg(format!("{}x{}", o.width, o.height))
            .arg("-framerate")
            .arg(o.fps.to_string())
            .args(["-i", "pipe:0", "-c:v", "libx264", "-preset", "veryfast"])
            .args(["-tune", "zerolatency", "-bf", "0", "-pix_fmt", "yuv420p"])
            .args(["-g", &gop, "-b:v", "2M"])
            .args(["-bsf:v", "h264_metadata=aud=insert"])
            .args(["-f", "h264", "-flush_packets", "1", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("cannot run ffmpeg: {e}")))?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
        let (tx, units) = mpsc::channel();
        std::thread::spawn(move || {
            // Split at AUD NAL units (00 00 00 01 09).
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 65536];
            let aud = [0u8, 0, 0, 1, 9];
            loop {
                let n = match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                while let Some(at) = buf.windows(5).skip(1).position(|w| w == aud).map(|p| p + 1) {
                    let unit: Vec<u8> = buf.drain(..at).collect();
                    if tx.send(unit).is_err() {
                        return;
                    }
                }
            }
            if !buf.is_empty() {
                let _ = tx.send(buf);
            }
        });
        Ok(Encoder {
            child,
            stdin,
            units,
            pending: VecDeque::new(),
        })
    }

    fn write(&mut self, pts_us: u64, rgba: &[u8]) -> io::Result<()> {
        self.pending.push_back(pts_us);
        self.stdin
            .as_mut()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .write_all(rgba)
    }

    /// Access units ready now (or all of them after `close`).
    fn drain(&mut self, wait: bool, out: &mut impl Write) -> io::Result<()> {
        loop {
            let unit = if wait {
                match self.units.recv() {
                    Ok(u) => u,
                    Err(_) => return Ok(()),
                }
            } else {
                match self.units.try_recv() {
                    Ok(u) => u,
                    Err(_) => return Ok(()),
                }
            };
            let Some(pts) = self.pending.pop_front() else {
                continue;
            };
            let key = nal_types(&unit).contains(&5);
            slrw::write_h264(out, pts, key, true, &unit)?;
        }
    }

    /// Ends the encoder, writing what it still had.
    fn close(mut self, out: &mut impl Write) -> io::Result<()> {
        drop(self.stdin.take());
        self.drain(true, out)?;
        let _ = self.child.wait();
        Ok(())
    }
}

fn run(o: &Opts) -> io::Result<()> {
    if o.exec.is_empty() {
        let out = io::stdout();
        let out = BufWriter::with_capacity(1 << 20, out.lock());
        return generate(o, out, None);
    }
    // Like a game: streamlib as a child, SLRW on its stdin, keyframe
    // requests on its stdout.
    let mut child = Command::new(&o.exec[0])
        .args(&o.exec[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot run {}: {e}", o.exec[0])))?;
    let stdin = child.stdin.take().ok_or(io::ErrorKind::BrokenPipe)?;
    let stdout = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
    let requested = Arc::new(AtomicBool::new(false));
    let flag = requested.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) if l.trim() == "keyframe" => flag.store(true, Ordering::SeqCst),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
    let r = generate(o, BufWriter::with_capacity(1 << 20, stdin), Some(requested));
    let _ = child.wait();
    r
}

fn generate(o: &Opts, mut out: impl Write, requests: Option<Arc<AtomicBool>>) -> io::Result<()> {
    slrw::write_magic(&mut out)?;
    let mut encoder = if o.encoded {
        if requests.is_some() {
            slrw::write_control(&mut out, slrw::CONTROL_KEYFRAME_REQUESTS)?;
        }
        Some(Encoder::start(o)?)
    } else {
        None
    };
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
        if encoder.is_some() {
            if requests
                .as_ref()
                .is_some_and(|r| r.swap(false, Ordering::SeqCst))
            {
                eprintln!("slrw-gen: keyframe requested; restarting the encoder");
                if let Some(old) = encoder.take() {
                    old.close(&mut out)?;
                }
                encoder = Some(Encoder::start(o)?);
            }
            let enc = encoder.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
            enc.write(pts_us, &video)?;
            enc.drain(false, &mut out)?;
        } else {
            slrw::write_packet(&mut out, Kind::Video, pts_us, &video)?;
        }
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
    if let Some(enc) = encoder.take() {
        enc.close(&mut out)?;
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
