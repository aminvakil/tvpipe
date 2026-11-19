//! A program streaming its own frames and sound with the tvpipe library:
//! a moving test card drawn in software and a 440 Hz tone, to a browser
//! tab (or the receivers named on the command line).
//!
//! ```text
//! cargo run --release --example stream               # a browser tab
//! cargo run --release --example stream -- "Living Room" browser
//! ```
//!
//! It prints the player page's URL; open it to watch. Ctrl-C ends the
//! program (dropping the stream stops the receivers).

use std::time::Duration;

use tvpipe::{Event, Options, PixelFormat, PlayerOptions, Written};

const W: usize = 640;
const H: usize = 360;
const FPS: u64 = 30;
const RATE: u32 = 48_000;

fn main() -> tvpipe::Result<()> {
    let mut targets: Vec<String> = std::env::args().skip(1).collect();
    if targets.is_empty() {
        targets.push("browser".into());
    }
    let mut options = Options::new()
        .video(W as u32, H as u32, FPS as u32, PixelFormat::Rgba)
        .audio(RATE, 2)
        .player(PlayerOptions::new().port(0))
        // Pairing with an Apple TV that needs it, on the terminal.
        .on_pin(|req| {
            eprint!("PIN shown on {}: ", req.name);
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok()?;
            Some(line.trim().to_owned())
        });
    for t in &targets {
        options = options.to(t.as_str());
    }
    let mut s = tvpipe::stream(options)?;
    if let Some(url) = s.player_url() {
        println!("watch at {url}");
    }

    let mut frame = vec![0u8; W * H * 4];
    let mut pcm = Vec::new();
    let mut phase = 0.0f32;
    let mut sent = 0u64;
    for n in 0u64.. {
        let pts = Duration::from_micros(n * 1_000_000 / FPS);
        // Pace by the stream's clock: a game would render at its own rate.
        if let Some(wait) = pts.checked_sub(s.elapsed()) {
            std::thread::sleep(wait);
        }
        draw(&mut frame, n);
        if s.write_video(&frame, pts)? == Written::Queued {
            sent += 1;
        }
        tone(&mut pcm, &mut phase, (RATE as u64 / FPS) as usize);
        s.write_audio(&pcm, pts)?;
        for e in s.poll_events() {
            match e {
                Event::Receiver(r) => println!("{}: {}", r.name, r.state),
                Event::Stats(st) if st.elapsed_ms / 1000 % 5 == 0 => {
                    println!("{} s, {sent} frames queued", st.elapsed_ms / 1000)
                }
                _ => {}
            }
        }
    }
    s.finish()?;
    Ok(())
}

/// Colour bars that scroll, with a white square that moves.
fn draw(frame: &mut [u8], n: u64) {
    const BARS: [[u8; 3]; 7] = [
        [235, 235, 235],
        [235, 235, 16],
        [16, 235, 235],
        [16, 235, 16],
        [235, 16, 235],
        [235, 16, 16],
        [16, 16, 235],
    ];
    let shift = (n as usize * 4) % W;
    let (sx, sy) = ((n as usize * 7) % (W - 40), (n as usize * 3) % (H - 40));
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            let c = if (sx..sx + 40).contains(&x) && (sy..sy + 40).contains(&y) {
                [255, 255, 255]
            } else {
                BARS[((x + shift) % W) * BARS.len() / W]
            };
            frame[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
}

/// `samples` stereo samples of a 440 Hz tone.
fn tone(pcm: &mut Vec<i16>, phase: &mut f32, samples: usize) {
    pcm.clear();
    for _ in 0..samples {
        let v = (phase.sin() * 6000.0) as i16;
        pcm.extend_from_slice(&[v, v]);
        *phase += 2.0 * std::f32::consts::PI * 440.0 / RATE as f32;
        if *phase > 2.0 * std::f32::consts::PI {
            *phase -= 2.0 * std::f32::consts::PI;
        }
    }
}
