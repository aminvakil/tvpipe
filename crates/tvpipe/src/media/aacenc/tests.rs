//! Encoder tests. The oracle tests decode our ADTS output with ffmpeg
//! (run as a black-box decoder) and skip cleanly when it is missing.

use super::*;
use std::path::PathBuf;
use std::process::Command;

fn ffmpeg() -> Option<String> {
    let bin = std::env::var("TVPIPE_FFMPEG").unwrap_or_else(|_| {
        if std::path::Path::new("/opt/homebrew/bin/ffmpeg").exists() {
            "/opt/homebrew/bin/ffmpeg".into()
        } else {
            "ffmpeg".into()
        }
    });
    Command::new(&bin)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn noise(&mut self) -> f64 {
        2.0 * self.next() - 1.0
    }
}

/// Test signals, `frames` frames of interleaved PCM.
fn signal(kind: &str, rate: u32, channels: usize, frames: usize) -> Vec<i16> {
    let n = frames * FRAME_LEN;
    let fs = rate as f64;
    let mut rng = Lcg(12345);
    let mut out = Vec::with_capacity(n * channels);
    let tau = std::f64::consts::TAU;
    let mut phase = 0.0;
    let mut burst = 0.0f64;
    let mut lp = [0.0f64; 3];
    for i in 0..n {
        let t = i as f64 / fs;
        let mono = match kind {
            "sine" => 0.5 * (tau * 1000.0 * t).sin(),
            "chirp" => {
                // 50 Hz -> 15 kHz exponential sweep.
                let dur = n as f64 / fs;
                let f = 50.0 * (300f64).powf(t / dur);
                phase += tau * f / fs;
                0.4 * phase.sin()
            }
            "noise" => 0.25 * rng.noise(),
            "music" => {
                // Harmonic tone with vibrato, a chord, light noise and
                // percussive bursts every 250 ms (exercises short blocks).
                let vib = 1.0 + 0.004 * (tau * 5.0 * t).sin();
                let mut v = 0.0;
                for (h, a) in [(1.0, 0.2), (2.0, 0.1), (3.0, 0.06), (5.0, 0.03)] {
                    v += a * (tau * 220.0 * h * vib * t).sin();
                }
                v += 0.08 * (tau * 277.18 * t).sin() + 0.06 * (tau * 329.63 * t).sin();
                if i % (rate as usize / 4) == rate as usize / 8 {
                    burst = 0.6;
                }
                burst *= 0.9985;
                // Bursts of noise through a third-order lowpass (~4 kHz),
                // so the coded bandwidth does not limit the SNR.
                let mut x = rng.noise();
                for p in &mut lp {
                    *p += 0.45 * (x - *p);
                    x = *p;
                }
                v + burst * 2.0 * x
            }
            _ => 0.0,
        };
        for ch in 0..channels {
            let v = if channels == 2 && kind == "noise" && ch == 1 {
                0.25 * rng.noise()
            } else if ch == 1 {
                mono * 0.8
            } else {
                mono
            };
            out.push((v * 32767.0).round().clamp(-32768.0, 32767.0) as i16);
        }
    }
    out
}

fn encode_all(enc: &mut Encoder, pcm: &[i16]) -> Vec<Vec<u8>> {
    let step = FRAME_LEN * enc.channels() as usize;
    let mut aus: Vec<Vec<u8>> = pcm.chunks(step).map(|f| enc.encode(f).unwrap()).collect();
    aus.extend(enc.flush());
    aus
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tvpipe-aacenc-{}-{name}", std::process::id()))
}

/// Decodes ADTS with ffmpeg to interleaved s16.
fn decode(ffmpeg: &str, adts: &[u8], name: &str) -> Vec<i16> {
    let input = tmp(&format!("{name}.aac"));
    let output = tmp(&format!("{name}.raw"));
    std::fs::write(&input, adts).unwrap();
    let st = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "aac",
            "-i",
        ])
        .arg(&input)
        .args(["-f", "s16le", "-acodec", "pcm_s16le"])
        .arg(&output)
        .status()
        .unwrap();
    assert!(st.success(), "ffmpeg failed to decode {name}");
    let raw = std::fs::read(&output).unwrap();
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    raw.chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// SNR in dB of `dec` against `src` delayed by `delay` samples per
/// channel, over the middle of the signal.
fn snr(src: &[i16], dec: &[i16], channels: usize, delay: usize) -> f64 {
    let frames = src.len() / channels;
    let (mut s, mut n) = (0f64, 0f64);
    for i in FRAME_LEN..frames.saturating_sub(FRAME_LEN) {
        for ch in 0..channels {
            let a = src[i * channels + ch] as f64;
            let b = *dec.get((i + delay) * channels + ch).unwrap_or(&0) as f64;
            s += a * a;
            n += (a - b) * (a - b);
        }
    }
    10.0 * (s / n.max(1e-9)).log10()
}

#[test]
fn audio_specific_config() {
    let e = Encoder::new(48000, 2, 128000).unwrap();
    assert_eq!(e.audio_specific_config(), [0x11, 0x90]);
    let e = Encoder::new(44100, 2, 128000).unwrap();
    assert_eq!(e.audio_specific_config(), [0x12, 0x10]);
    let e = Encoder::new(48000, 1, 64000).unwrap();
    assert_eq!(e.audio_specific_config(), [0x11, 0x88]);
}

#[test]
fn adts_header_fields() {
    let e = Encoder::new(48000, 2, 128000).unwrap();
    let f = e.adts(&[0xaa; 100]);
    assert_eq!(f.len(), 107);
    assert_eq!(&f[..2], &[0xff, 0xf1]);
    // profile 01, sf index 0011, private 0, channel config 010.
    assert_eq!(f[2], 0b0100_1100);
    let len = ((f[3] as usize & 3) << 11) | ((f[4] as usize) << 3) | (f[5] as usize >> 5);
    assert_eq!(len, 107);
    assert_eq!(f[6] & 3, 0);
}

#[test]
fn rejects_bad_parameters() {
    assert!(Encoder::new(47000, 2, 128000).is_err());
    assert!(Encoder::new(48000, 3, 128000).is_err());
    assert!(Encoder::new(48000, 0, 128000).is_err());
    assert!(Encoder::new(48000, 2, 1000).is_err());
    assert!(Encoder::new(48000, 1, 400_000).is_err());
    let mut e = Encoder::new(48000, 2, 128000).unwrap();
    assert!(e.encode(&[0; 1024]).is_err());
    assert!(e.encode(&[0; 2049]).is_err());
}

#[test]
fn silence_is_tiny() {
    let mut e = Encoder::new(48000, 2, 128000).unwrap();
    for _ in 0..10 {
        let au = e.encode(&[0; 2048]).unwrap();
        assert!(au.len() < 16, "{}", au.len());
    }
}

/// Arbitrary inputs (extreme levels, clipping squares, impulses, noise)
/// at every rate never panic and stay within the decoder buffer limit.
#[test]
fn fuzz_inputs_stay_in_bounds() {
    let mut rng = Lcg(99);
    for (i, &rate) in RATES.iter().enumerate() {
        for channels in [1u8, 2] {
            let max = (6144u64 * rate as u64 / 1024) as u32 * channels as u32;
            let bitrate = [8000 * channels as u32, 96000, max][i % 3].min(max);
            let mut e = Encoder::new(rate, channels, bitrate).unwrap();
            for frame in 0..12 {
                let pcm: Vec<i16> = (0..FRAME_LEN * channels as usize)
                    .map(|k| match (frame + i) % 6 {
                        0 => (rng.noise() * 32768.0) as i16,
                        1 => {
                            if (k / 7) % 2 == 0 {
                                i16::MAX
                            } else {
                                i16::MIN
                            }
                        }
                        2 => {
                            if k % 333 == 0 {
                                i16::MIN
                            } else {
                                0
                            }
                        }
                        3 => (rng.noise() * 3.0) as i16,
                        4 => i16::MIN,
                        _ => ((k as f64 * 0.3).sin() * 32767.0) as i16,
                    })
                    .collect();
                let au = e.encode(&pcm).unwrap();
                assert!(au.len() * 8 <= 6144 * channels as usize, "{}", au.len());
            }
        }
    }
}

/// The reservoir starts empty, so every prefix of the stream stays within
/// the target rate (and every AU within the decoder buffer).
#[test]
fn rate_never_exceeds_target() {
    for (kind, br) in [("noise", 64000u32), ("music", 96000), ("chirp", 32000)] {
        let pcm = signal(kind, 48000, 2, 60);
        let mut e = Encoder::new(48000, 2, br).unwrap();
        let mut total = 0usize;
        for (i, f) in pcm.chunks(2048).enumerate() {
            total += e.encode(f).unwrap().len() * 8;
            let allowed = (i + 1) as f64 * br as f64 * 1024.0 / 48000.0;
            assert!(
                total as f64 <= allowed,
                "{kind}: {total} > {allowed} after {i}"
            );
        }
        assert!(total as f64 > 0.8 * 60.0 * br as f64 * 1024.0 / 48000.0 || kind == "chirp");
    }
}

/// Round trips through ffmpeg; returns (SNR dB, kbit/s).
fn roundtrip(ffmpeg: &str, kind: &str, rate: u32, channels: u8, bitrate: u32) -> (f64, f64) {
    let frames = (rate as usize * 3) / FRAME_LEN;
    let pcm = signal(kind, rate, channels as usize, frames);
    let mut e = Encoder::new(rate, channels, bitrate).unwrap();
    let delay = e.delay_samples();
    let aus = encode_all(&mut e, &pcm);
    let bytes: usize = aus.iter().map(|a| a.len()).sum();
    let adts: Vec<u8> = aus.iter().flat_map(|a| e.adts(a)).collect();
    let dec = decode(
        ffmpeg,
        &adts,
        &format!("{kind}-{rate}-{channels}-{bitrate}"),
    );
    assert_eq!(dec.len(), aus.len() * FRAME_LEN * channels as usize);
    let kbps = bytes as f64 * 8.0 / (aus.len() as f64 * FRAME_LEN as f64 / rate as f64) / 1000.0;
    (snr(&pcm, &dec, channels as usize, delay), kbps)
}

#[test]
fn oracle_ffprobe_accepts_stream() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    let probe = ff.replace("ffmpeg", "ffprobe");
    let mut e = Encoder::new(44100, 2, 128000).unwrap();
    let aus = encode_all(&mut e, &signal("music", 44100, 2, 20));
    let path = tmp("probe.aac");
    std::fs::write(
        &path,
        aus.iter().flat_map(|a| e.adts(a)).collect::<Vec<u8>>(),
    )
    .unwrap();
    let out = Command::new(&probe)
        .args(["-v", "error", "-show_streams", "-of", "compact"])
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);
    let Ok(out) = out else {
        eprintln!("ffprobe not found; skipping");
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "{text} {:?}",
        out.stderr
    );
    for want in [
        "codec_name=aac",
        "profile=LC",
        "sample_rate=44100",
        "channels=2",
    ] {
        assert!(text.contains(want), "{want} missing in {text}");
    }
}

#[test]
fn oracle_quality() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    // (signal, rate, channels, bitrate, minimum SNR dB)
    let cases = [
        ("sine", 48000, 2, 128000, 30.0),
        ("chirp", 48000, 2, 128000, 18.0),
        ("noise", 48000, 2, 256000, 4.0),
        ("music", 48000, 2, 128000, 15.0),
        ("music", 44100, 2, 192000, 18.0),
        ("music", 48000, 1, 96000, 15.0),
        ("music", 48000, 2, 64000, 12.0),
        ("sine", 96000, 1, 96000, 30.0),
        ("sine", 32000, 2, 96000, 30.0),
        ("music", 24000, 2, 64000, 15.0),
        ("music", 16000, 1, 32000, 12.0),
        ("sine", 8000, 1, 16000, 20.0),
    ];
    for (kind, rate, ch, br, min) in cases {
        let (snr, kbps) = roundtrip(&ff, kind, rate, ch, br);
        eprintln!("{kind} {rate} Hz {ch} ch {br} bps: SNR {snr:.1} dB, {kbps:.1} kbit/s");
        assert!(snr >= min, "{kind} {br}: SNR {snr:.1} < {min}");
        assert!(kbps <= br as f64 / 1000.0 * 1.02, "{kind}: {kbps} kbit/s");
    }
}

/// Prints an SNR / bitrate / speed table (`cargo test -- --ignored
/// aacenc_report --nocapture`).
#[test]
#[ignore]
fn aacenc_report() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    for kind in ["sine", "chirp", "noise", "music"] {
        for br in [96000, 128000, 192000, 256000] {
            let (snr, kbps) = roundtrip(&ff, kind, 48000, 2, br);
            eprintln!(
                "{kind:6} {:3}k: SNR {snr:5.1} dB at {kbps:6.1} kbit/s",
                br / 1000
            );
        }
    }
    let pcm = signal("music", 48000, 2, 470);
    let mut e = Encoder::new(48000, 2, 128000).unwrap();
    let t = std::time::Instant::now();
    let _ = encode_all(&mut e, &pcm);
    let secs = 470.0 * 1024.0 / 48000.0;
    eprintln!(
        "speed: {:.0}x realtime (48 kHz stereo 128k)",
        secs / t.elapsed().as_secs_f64()
    );
}

#[test]
#[ignore]
fn aacenc_diag() {
    let Some(ff) = ffmpeg() else { return };
    let kind = std::env::var("SIG").unwrap_or("music".into());
    let br: u32 = std::env::var("BR")
        .map(|s| s.parse().unwrap())
        .unwrap_or(256000);
    let rate = 48000;
    let frames: usize = std::env::var("FRAMES")
        .map(|s| s.parse().unwrap())
        .unwrap_or(60);
    let pcm = signal(&kind, rate, 2, frames);
    let mut e = Encoder::new(rate, 2, br).unwrap();
    let mut aus = Vec::new();
    let mut seqs = Vec::new();
    for f in pcm.chunks(2048) {
        aus.push(e.encode(f).unwrap());
        seqs.push((e.core.last_seq(), e.core.last_t()));
    }
    aus.extend(e.flush());
    let adts: Vec<u8> = aus.iter().flat_map(|a| e.adts(a)).collect();
    let dec = decode(&ff, &adts, "diag");
    for f in 1..frames - 2 {
        let (mut s, mut n) = (0f64, 0f64);
        for i in f * 1024..(f + 1) * 1024 {
            for ch in 0..2 {
                let a = pcm[i * 2 + ch] as f64;
                let b = dec[(i + 2048) * 2 + ch] as f64;
                s += a * a;
                n += (a - b) * (a - b);
            }
        }
        // AU f + 2 decodes to frame f; its block was decided at call f + 2.
        eprintln!(
            "frame {f:3}: {:5.1} dB  au {:4} B  {:?}",
            10.0 * (s / n.max(1e-9)).log10(),
            aus[f + 2].len(),
            seqs[f + 2]
        );
    }
}

/// A loud burst after near-silence: block switching must keep the
/// pre-echo (error before the attack) far below the burst.
#[test]
fn oracle_pre_echo() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    let rate = 48000;
    let frames = 30;
    let attack = 15 * FRAME_LEN + 300;
    let mut rng = Lcg(5);
    let pcm: Vec<i16> = (0..frames * FRAME_LEN)
        .map(|i| {
            let quiet = 0.003 * (std::f64::consts::TAU * 440.0 * i as f64 / rate as f64).sin();
            let burst = if i >= attack {
                0.7 * rng.noise() * 0.9995f64.powi((i - attack) as i32)
            } else {
                0.0
            };
            ((quiet + burst) * 32767.0) as i16
        })
        .collect();
    let mut e = Encoder::new(rate, 1, 96000).unwrap();
    let mut seqs = Vec::new();
    let mut aus = Vec::new();
    for f in pcm.chunks(FRAME_LEN) {
        aus.push(e.encode(f).unwrap());
        seqs.push(e.core.last_seq());
    }
    aus.extend(e.flush());
    assert!(seqs.contains(&encoder::Seq::EightShort), "{seqs:?}");
    let adts: Vec<u8> = aus.iter().flat_map(|a| e.adts(a)).collect();
    let dec = decode(&ff, &adts, "preecho");
    let d = e.delay_samples();
    let err = |r: std::ops::Range<usize>| {
        r.map(|i| (pcm[i] as f64 - dec[i + d] as f64).powi(2))
            .sum::<f64>()
    };
    let energy = |r: std::ops::Range<usize>| r.map(|i| (pcm[i] as f64).powi(2)).sum::<f64>();
    // A short window (256 samples) spreads its noise over its whole span,
    // so look before the window holding the attack.
    let pre = err(attack - 1024..attack - 256) / 768.0;
    let burst = energy(attack..attack + 768) / 768.0;
    let ratio = 10.0 * (burst / pre.max(1e-9)).log10();
    eprintln!("pre-echo {ratio:.1} dB below the burst");
    assert!(ratio > 60.0, "pre-echo only {ratio:.1} dB below the burst");
}

/// SNR on a real recording: `AACENC_INPUT=file.raw` (s16le, 48 kHz,
/// stereo), optionally `AACENC_BITRATE`. Also reports ffmpeg's own AAC
/// encoder (run as a black box) on the same input for reference.
#[test]
#[ignore]
fn aacenc_file_report() {
    let (Some(ff), Ok(path)) = (ffmpeg(), std::env::var("AACENC_INPUT")) else {
        eprintln!("needs ffmpeg and AACENC_INPUT; skipping");
        return;
    };
    let br: u32 = std::env::var("AACENC_BITRATE").map_or(128000, |s| s.parse().unwrap());
    let raw = std::fs::read(&path).unwrap();
    let mut pcm: Vec<i16> = raw
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    pcm.truncate(pcm.len() / 2048 * 2048);
    let mut e = Encoder::new(48000, 2, br).unwrap();
    let t = std::time::Instant::now();
    let aus = encode_all(&mut e, &pcm);
    let el = t.elapsed().as_secs_f64();
    let bytes: usize = aus.iter().map(|a| a.len()).sum();
    let secs = pcm.len() as f64 / 2.0 / 48000.0;
    let adts: Vec<u8> = aus.iter().flat_map(|a| e.adts(a)).collect();
    let dec = decode(&ff, &adts, "file");
    eprintln!(
        "ours:   SNR {:.1} dB at {:.1} kbit/s, {:.0}x realtime",
        snr(&pcm, &dec, 2, e.delay_samples()),
        bytes as f64 * 8.0 / secs / 1000.0,
        secs / el
    );
    // Reference: ffmpeg's encoder, aligned by searching the delay.
    let out = tmp("ref.aac");
    let st = Command::new(&ff)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "s16le"])
        .args(["-ar", "48000", "-ac", "2", "-i", &path, "-c:a", "aac"])
        .args(["-b:a", &br.to_string(), "-f", "adts"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(st.success());
    let adts = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    let dec = decode(&ff, &adts, "ref");
    let best = (0..3000)
        .map(|d| (snr(&pcm[..pcm.len().min(480000)], &dec, 2, d), d))
        .fold((f64::MIN, 0), |a, b| if b.0 > a.0 { b } else { a });
    eprintln!(
        "ffmpeg: SNR {:.1} dB at {:.1} kbit/s (delay {})",
        snr(&pcm, &dec, 2, best.1),
        adts.len() as f64 * 8.0 / secs / 1000.0,
        best.1
    );
}
