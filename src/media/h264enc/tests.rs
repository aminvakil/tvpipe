//! Oracle tests: ffmpeg must decode our streams to exactly our
//! reconstruction. Skipped when ffmpeg is not installed.

use std::path::PathBuf;
use std::process::Command;

use super::*;

pub(crate) fn ffmpeg() -> Option<PathBuf> {
    for cand in [
        "ffmpeg",
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ] {
        let ok = Command::new(cand)
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(PathBuf::from(cand));
        }
    }
    None
}

fn tmp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("streamlib-h264enc-{}-{}", std::process::id(), name));
    p
}

/// Deterministic "natural-ish" content: a moving gradient, a scrolling
/// textured layer, moving rectangles with sharp edges at sub-pixel speeds.
pub(crate) fn synth(w: u32, h: u32, t: u32) -> Yuv420Frame {
    let mut f = Yuv420Frame::new(w, h);
    let (wu, hu) = (w as usize, h as usize);
    let tf = t as f64;
    let noise = |x: usize, y: usize| -> i32 {
        let mut s = (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
        s ^= s >> 15;
        s = s.wrapping_mul(0x2c1b_3c6d);
        s ^= s >> 12;
        (s & 31) as i32 - 16
    };
    for y in 0..hu {
        for x in 0..wu {
            let gx = x as f64 + tf * 1.5;
            let gy = y as f64 + tf * 0.75;
            let mut v = 60.0 + 0.25 * gx % 120.0 + 30.0 * ((gx * 0.05).sin() * (gy * 0.07).cos());
            // Texture scrolling at 2 px / frame.
            v += noise(x + 2 * t as usize, y) as f64 * 0.3;
            v += 12.0 * (((x + 2 * t as usize) as f64 * 0.31).sin() * ((y as f64) * 0.23).sin());
            // Rectangles.
            for k in 0..3u32 {
                let rx = ((40 + 70 * k) as f64 + tf * (1.25 + k as f64)) % (w as f64);
                let ry = ((30 + 40 * k) as f64 + tf * (0.5 * k as f64)) % (h as f64);
                if (x as f64) >= rx
                    && (x as f64) < rx + 48.0
                    && (y as f64) >= ry
                    && (y as f64) < ry + 32.0
                {
                    v = [230.0, 20.0, 128.0][k as usize] + noise(x, y) as f64 * 0.2;
                }
            }
            f.y[y * wu + x] = v.clamp(0.0, 255.0) as u8;
        }
    }
    let cw = f.chroma_width();
    for y in 0..f.chroma_height() {
        for x in 0..cw {
            let u = 128.0 + 40.0 * (((x as f64) + tf) * 0.04).sin();
            let v = 128.0 + 40.0 * (((y as f64) - tf * 0.5) * 0.05).cos();
            f.u[y * cw + x] = (u + noise(x, y + 7) as f64 * 0.2) as u8;
            f.v[y * cw + x] = v as u8;
        }
    }
    f
}

/// AVCC access units → Annex B stream with the parameter sets in front.
pub(crate) fn annexb(enc: &Encoder, aus: &[Encoded]) -> Vec<u8> {
    let mut out = Vec::new();
    let (sps, pps) = enc.parameter_sets();
    for ps in [sps, pps] {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(ps);
    }
    for au in aus {
        for nal in crate::media::h264::avcc_nals(&au.data, 4) {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal.unwrap());
        }
    }
    out
}

/// Decodes an Annex B stream with ffmpeg into raw I420.
pub(crate) fn decode(ffmpeg: &PathBuf, stream: &[u8], name: &str) -> Vec<u8> {
    let inp = tmp_path(&format!("{name}.264"));
    let out = tmp_path(&format!("{name}.yuv"));
    std::fs::write(&inp, stream).unwrap();
    let o = Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-threads", "1", "-f", "h264", "-i"])
        .arg(&inp)
        .args([
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&out)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(o.status.success(), "ffmpeg failed: {stderr}");
    assert!(stderr.trim().is_empty(), "ffmpeg complained: {stderr}");
    let data = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&inp);
    let _ = std::fs::remove_file(&out);
    data
}

pub(crate) fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = x as i64 - y as i64;
            (d * d) as u64
        })
        .sum();
    if se == 0 {
        return 99.0;
    }
    let mse = se as f64 / a.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// Encodes `n` synthetic frames and checks ffmpeg's output against our
/// reconstruction frame by frame. Returns (mean luma PSNR vs source, total
/// bytes).
fn roundtrip(
    name: &str,
    cfg: Config,
    tools: Tools,
    n: u32,
    idr_at: &[u32],
) -> Option<(f64, usize)> {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping {name}");
        return None;
    };
    let (w, h) = (cfg.width, cfg.height);
    let mut enc = Encoder::new(cfg).unwrap();
    enc.set_tools(tools);
    let mut aus = Vec::new();
    let mut recon = Vec::new();
    let mut psnr_sum = 0.0;
    for t in 0..n {
        let f = synth(w, h, t);
        let au = enc.encode(&f, idr_at.contains(&t)).unwrap();
        let r = enc.recon().unwrap();
        psnr_sum += psnr(&f.y, &r.y);
        recon.push(r.to_i420());
        aus.push(au);
    }
    let bytes = aus.iter().map(|a| a.data.len()).sum();
    let dec = decode(&ff, &annexb(&enc, &aus), name);
    let fsize = recon[0].len();
    assert_eq!(dec.len(), fsize * n as usize, "{name}: frame count");
    for (i, r) in recon.iter().enumerate() {
        let d = &dec[i * fsize..(i + 1) * fsize];
        if d != &r[..] {
            let first = d.iter().zip(r.iter()).position(|(a, b)| a != b).unwrap();
            let ysz = (w * h) as usize;
            let (plane, off, pw) = if first < ysz {
                ("Y", first, w as usize)
            } else if first < ysz * 5 / 4 {
                ("U", first - ysz, w as usize / 2)
            } else {
                ("V", first - ysz * 5 / 4, w as usize / 2)
            };
            panic!(
                "{name}: frame {i} differs first at {plane} x={} y={} (dec {} ours {}), kind {}",
                off % pw,
                off / pw,
                d[first],
                r[first],
                if aus[i].keyframe { "IDR" } else { "P" }
            );
        }
    }
    Some((psnr_sum / n as f64, bytes))
}

fn cfg(w: u32, h: u32, qp: u8) -> Config {
    Config {
        width: w,
        height: h,
        fps: Ratio::integer(30),
        bitrate_kbps: 4000,
        gop: 0,
        threads: 1,
        qp: Some(qp),
        ..Config::default()
    }
}

#[test]
fn pcm_is_lossless() {
    let tools = Tools {
        pcm: true,
        ..Tools::default()
    };
    let c = Config {
        gop: 1,
        ..cfg(64, 48, 26)
    };
    if let Some((p, _)) = roundtrip("pcm", c, tools, 2, &[]) {
        assert_eq!(p, 99.0);
    }
}

#[test]
fn intra16_only_matches_ffmpeg() {
    let tools = Tools {
        i4x4: false,
        ..Tools::default()
    };
    let c = Config {
        gop: 1,
        ..cfg(96, 64, 28)
    };
    roundtrip("i16", c, tools, 2, &[]);
}

#[test]
fn intra_matches_ffmpeg() {
    for qp in [12, 30, 45] {
        let c = Config {
            gop: 1,
            ..cfg(96, 64, qp)
        };
        if let Some((p, _)) = roundtrip(&format!("intra{qp}"), c, Tools::default(), 2, &[]) {
            eprintln!("intra qp {qp}: {p:.2} dB");
        }
    }
}

#[test]
fn inter_matches_ffmpeg() {
    for qp in [18, 32] {
        let c = cfg(128, 96, qp);
        if let Some((p, b)) = roundtrip(&format!("inter{qp}"), c, Tools::default(), 8, &[5]) {
            eprintln!("inter qp {qp}: {p:.2} dB, {b} bytes");
        }
    }
}

#[test]
fn cropped_multislice_matches_ffmpeg() {
    let c = Config {
        threads: 3,
        ..cfg(150, 86, 27)
    };
    roundtrip("slices", c, Tools::default(), 6, &[]);
}

#[test]
fn long_sequence_wraps_frame_num() {
    // > 256 frames (frame_num wraps), periodic IDRs (idr_pic_id changes),
    // rate control on.
    let c = Config {
        gop: 100,
        qp: None,
        bitrate_kbps: 300,
        ..cfg(48, 32, 0)
    };
    roundtrip("long", c, Tools::default(), 300, &[7, 8]);
}

#[test]
fn keyframes_on_request_and_gop() {
    let c = Config {
        gop: 5,
        ..cfg(32, 32, 30)
    };
    let mut enc = Encoder::new(c).unwrap();
    let f = synth(32, 32, 0);
    let kinds: Vec<bool> = (0..12)
        .map(|i| {
            if i == 7 {
                enc.request_keyframe();
            }
            enc.encode(&f, i == 2).unwrap().keyframe
        })
        .collect();
    let idx: Vec<usize> = (0..12).filter(|&i| kinds[i]).collect();
    assert_eq!(idx, vec![0, 2, 7]);
    let mut enc = Encoder::new(Config {
        gop: 4,
        ..cfg(32, 32, 30)
    })
    .unwrap();
    let k: Vec<usize> = (0..10)
        .filter(|_| enc.encode(&f, false).unwrap().keyframe)
        .collect();
    assert_eq!(k, vec![0, 4, 8]);
}

#[test]
fn rate_control_hits_target() {
    let (w, h) = (160u32, 96u32);
    let frames: Vec<_> = (0..40).map(|t| synth(w, h, t)).collect();
    for kbps in [250u32, 800] {
        let mut enc = Encoder::new(Config {
            width: w,
            height: h,
            fps: Ratio::integer(30),
            bitrate_kbps: kbps,
            threads: 2,
            ..Config::default()
        })
        .unwrap();
        let n = frames.len();
        let mut bits = 0usize;
        let mut max_p = 0usize;
        for f in &frames {
            let au = enc.encode(f, false).unwrap();
            bits += au.data.len() * 8;
            if !au.keyframe {
                max_p = max_p.max(au.data.len() * 8);
            }
        }
        let rate = bits as f64 / (n as f64 / 30.0) / 1000.0;
        let avg = kbps as f64 * 1000.0 / 30.0;
        eprintln!(
            "rc {kbps}k: {rate:.0} kbit/s, max P {:.2}x",
            max_p as f64 / avg
        );
        assert!(
            (rate / kbps as f64 - 1.0).abs() < 0.15,
            "{kbps}k: got {rate:.0}"
        );
        assert!((max_p as f64) < 2.5 * avg, "P frame {max_p} bits");
    }
}

#[test]
fn tiny_pictures() {
    for (w, h) in [(2u32, 2u32), (18, 2), (2, 34)] {
        let c = Config {
            qp: None,
            bitrate_kbps: 50,
            threads: 4,
            ..cfg(w, h, 0)
        };
        roundtrip(&format!("tiny{w}x{h}"), c, Tools::default(), 4, &[]);
    }
}

#[test]
fn rejects_bad_input() {
    assert!(Encoder::new(Config {
        width: 33,
        ..Config::default()
    })
    .is_err());
    let mut enc = Encoder::new(cfg(64, 48, 26)).unwrap();
    assert!(enc.encode(&Yuv420Frame::new(32, 32), false).is_err());
    let mut f = Yuv420Frame::new(64, 48);
    f.u.pop();
    assert!(enc.encode(&f, false).is_err());
}

/// Speed/quality report: `cargo test --release -- --ignored --nocapture bench_`.
#[test]
#[ignore]
fn bench_speed() {
    for (w, h, kbps, fps) in [
        (1280u32, 720u32, 8000u32, 60u32),
        (1920, 1080, 12000, 30),
        (1920, 1080, 20000, 60),
    ] {
        for threads in [1u32, 0] {
            let c = Config {
                width: w,
                height: h,
                fps: Ratio::integer(fps),
                bitrate_kbps: kbps,
                threads,
                qp: None,
                ..Config::default()
            };
            let mut enc = Encoder::new(c).unwrap();
            let n = 40;
            let frames: Vec<_> = (0..n as u32).map(|t| synth(w, h, t)).collect();
            let start = std::time::Instant::now();
            let mut bytes = 0;
            let mut ps = 0.0;
            let mut qps = Vec::new();
            for t in 0..n {
                let f = &frames[t % frames.len()];
                let au = enc.encode(f, false).unwrap();
                bytes += au.data.len();
                qps.push(au.qp);
                ps += psnr(&f.y, &enc.recon().unwrap().y);
            }
            let el = start.elapsed().as_secs_f64();
            let kbps_out = bytes as f64 * 8.0 / (n as f64 / fps as f64) / 1000.0;
            eprintln!(
                "{w}x{h}@{fps} {kbps}k threads={threads}: {:.1} fps, {:.0} kbit/s, PSNR {:.2} dB, stats {:?}, qp {:?}",
                n as f64 / el,
                kbps_out,
                ps / n as f64,
                enc.last_stats(),
                &qps[..12]
            );
        }
    }
}

#[test]
#[ignore]
fn bench_rq() {
    let (w, h) = (1280u32, 720u32);
    let frames: Vec<_> = (0..12u32).map(|t| synth(w, h, t)).collect();
    for qp in [20u8, 26, 32, 38] {
        let mut enc = Encoder::new(Config {
            width: w,
            height: h,
            qp: Some(qp),
            threads: 0,
            ..Config::default()
        })
        .unwrap();
        let mut sizes = Vec::new();
        for f in &frames {
            sizes.push(enc.encode(f, false).unwrap().data.len() * 8 / 1000);
        }
        eprintln!("qp {qp}: kbit {:?} stats {:?}", sizes, enc.last_stats());
    }
}

#[test]
fn ffprobe_accepts_stream() {
    let Some(ff) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    let probe = ff.with_file_name("ffprobe");
    let probe = if probe.is_absolute() {
        probe
    } else {
        PathBuf::from("ffprobe")
    };
    let mut enc = Encoder::new(cfg(150, 86, 30)).unwrap();
    let aus: Vec<_> = (0..3)
        .map(|t| enc.encode(&synth(150, 86, t), false).unwrap())
        .collect();
    let inp = tmp_path("probe.264");
    std::fs::write(&inp, annexb(&enc, &aus)).unwrap();
    let Ok(o) = Command::new(&probe)
        .args(["-v", "error", "-show_streams", "-of", "flat"])
        .arg(&inp)
        .output()
    else {
        eprintln!("ffprobe not found; skipping");
        return;
    };
    let _ = std::fs::remove_file(&inp);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success() && o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    for want in [
        "codec_name=\"h264\"",
        "profile=\"Constrained Baseline\"",
        "width=150",
        "height=86",
        "has_b_frames=0",
        "color_space=\"bt709\"",
        "color_range=\"tv\"",
    ] {
        assert!(out.contains(want), "missing {want} in {out}");
    }
}

/// Raw I420 frames from an ffmpeg lavfi source.
pub(crate) fn lavfi_frames(ff: &PathBuf, src: &str, w: u32, h: u32, n: u32) -> Vec<Yuv420Frame> {
    let out = tmp_path(&format!("lavfi-{}.yuv", src.len()));
    let o = Command::new(ff)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "{src}{}size={w}x{h}:rate=60",
            if src.contains('=') { ":" } else { "=" }
        ))
        .args([
            "-frames:v",
            &n.to_string(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&out)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let data = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    let fs = (w * h * 3 / 2) as usize;
    data.chunks_exact(fs)
        .map(|c| Yuv420Frame::from_i420(w, h, c).unwrap())
        .collect()
}

/// Quality report on lavfi content:
/// `cargo test --release -- --ignored --nocapture bench_quality`.
#[test]
#[ignore]
fn bench_quality() {
    let Some(ff) = ffmpeg() else { return };
    let (w, h) = (1280u32, 720u32);
    for src in ["testsrc2", "mandelbrot", "gradients=speed=0.02"] {
        let frames = lavfi_frames(&ff, src, w, h, 120);
        for kbps in [2000u32, 4000, 8000] {
            let mut enc = Encoder::new(Config {
                width: w,
                height: h,
                fps: Ratio::integer(60),
                bitrate_kbps: kbps,
                threads: 0,
                ..Config::default()
            })
            .unwrap();
            let mut bytes = 0;
            let mut ps = 0.0;
            let mut max_frame = 0;
            let start = std::time::Instant::now();
            for f in &frames {
                let au = enc.encode(f, false).unwrap();
                bytes += au.data.len();
                max_frame = max_frame.max(au.data.len());
                ps += psnr(&f.y, &enc.recon().unwrap().y);
            }
            let el = start.elapsed().as_secs_f64();
            let secs = frames.len() as f64 / 60.0;
            let avg = bytes as f64 / frames.len() as f64;
            eprintln!(
                "{src:>18} {kbps:>5}k: {:>6.0} kbit/s ({:+.1}%), PSNR-Y {:.2} dB, max frame {:.1}x avg, {:.0} fps",
                bytes as f64 * 8.0 / secs / 1000.0,
                (bytes as f64 * 8.0 / secs / 1000.0 / kbps as f64 - 1.0) * 100.0,
                ps / frames.len() as f64,
                max_frame as f64 / avg,
                frames.len() as f64 / el
            );
        }
    }
}

#[test]
#[ignore]
fn bench_trace() {
    let Some(ff) = ffmpeg() else { return };
    let src = std::env::var("H264ENC_SRC").unwrap_or("cellauto=rule=110".into());
    let kbps: u32 = std::env::var("H264ENC_KBPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4000);
    let (w, h) = (1280u32, 720u32);
    let frames = lavfi_frames(&ff, &src, w, h, 120);
    let mut enc = Encoder::new(Config {
        width: w,
        height: h,
        bitrate_kbps: kbps,
        threads: 0,
        ..Config::default()
    })
    .unwrap();
    for (i, f) in frames.iter().enumerate() {
        let au = enc.encode(f, false).unwrap();
        eprintln!(
            "{i:>3} {} qp {:>2} {:>7} bits psnr {:.2} stats {:?}",
            if au.keyframe { "I" } else { "P" },
            au.qp,
            au.data.len() * 8,
            psnr(&f.y, &enc.recon().unwrap().y),
            enc.last_stats()
        );
    }
}

/// Single-threaded profile target: `H264ENC_N=200 cargo test --release
/// -- --ignored bench_profile`.
#[test]
#[ignore]
fn bench_profile() {
    let Some(ff) = ffmpeg() else { return };
    let n: usize = std::env::var("H264ENC_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let (w, h) = (1920u32, 1080u32);
    let src = std::env::var("H264ENC_SRC").unwrap_or("synth".into());
    let frames = if src == "synth" {
        (0..30).map(|t| synth(w, h, t)).collect()
    } else {
        lavfi_frames(&ff, &src, w, h, 30)
    };
    let mut enc = Encoder::new(Config {
        width: w,
        height: h,
        fps: Ratio::integer(30),
        bitrate_kbps: 12000,
        threads: 1,
        ..Config::default()
    })
    .unwrap();
    let start = std::time::Instant::now();
    for i in 0..n {
        enc.encode(&frames[i % frames.len()], false).unwrap();
    }
    eprintln!(
        "1080p single thread: {:.1} fps",
        n as f64 / start.elapsed().as_secs_f64()
    );
}

/// Bit-exactness at full HD with threads and rate control on lavfi
/// content: `cargo test --release -- --ignored oracle_1080p`.
#[test]
#[ignore]
fn oracle_1080p() {
    let Some(ff) = ffmpeg() else { return };
    let (w, h) = (1920u32, 1080u32);
    for src in ["testsrc2", "mandelbrot"] {
        let frames = lavfi_frames(&ff, src, w, h, 40);
        let mut enc = Encoder::new(Config {
            width: w,
            height: h,
            fps: Ratio::integer(60),
            bitrate_kbps: 15000,
            threads: 0,
            ..Config::default()
        })
        .unwrap();
        let mut aus = Vec::new();
        let mut recon = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            aus.push(enc.encode(f, i == 25).unwrap());
            recon.extend_from_slice(&enc.recon().unwrap().to_i420());
        }
        let dec = decode(&ff, &annexb(&enc, &aus), src);
        assert!(
            dec == recon,
            "{src}: decoded frames differ from reconstruction"
        );
    }
}

#[test]
#[ignore]
fn bench_scene_cuts() {
    let Some(ff) = ffmpeg() else { return };
    let (w, h) = (1280u32, 720u32);
    let a = lavfi_frames(&ff, "testsrc2", w, h, 30);
    let b = lavfi_frames(&ff, "mandelbrot", w, h, 30);
    let mut enc = Encoder::new(Config {
        width: w,
        height: h,
        bitrate_kbps: 8000,
        ..Config::default()
    })
    .unwrap();
    let avg = 8000.0 * 1000.0 / 60.0;
    for i in 0..120 {
        let f = if (i / 30) % 2 == 0 {
            &a[i % 30]
        } else {
            &b[i % 30]
        };
        let t = std::time::Instant::now();
        let au = enc.encode(f, false).unwrap();
        if i % 30 < 3 {
            eprintln!(
                "{i:>3} qp {} {:.2}x avg, {:.1} ms, psnr {:.2}",
                au.qp,
                au.data.len() as f64 * 8.0 / avg,
                t.elapsed().as_secs_f64() * 1000.0,
                psnr(&f.y, &enc.recon().unwrap().y)
            );
        }
    }
}

#[test]
fn parameter_set_vectors() {
    for b in crate::util::vectors::load("h264enc/params.txt") {
        let enc = Encoder::new(Config {
            width: b.parse("width"),
            height: b.parse("height"),
            fps: Ratio::integer(b.parse("fps")),
            bitrate_kbps: b.parse("kbps"),
            ..Config::default()
        })
        .unwrap();
        let (sps, pps) = enc.parameter_sets();
        assert_eq!(sps, &b.hex("sps")[..], "{}", b.name());
        assert_eq!(pps, &b.hex("pps")[..], "{}", b.name());
        assert_eq!(enc.avcc(), b.hex("avcc"), "{}", b.name());
    }
}

#[test]
#[ignore]
fn bench_fixed_qp() {
    let Some(ff) = ffmpeg() else { return };
    let (w, h) = (1280u32, 720u32);
    let qp: u8 = std::env::var("H264ENC_QP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    for src in ["mandelbrot", "testsrc2"] {
        let frames = lavfi_frames(&ff, src, w, h, 120);
        let mut enc = Encoder::new(Config {
            width: w,
            height: h,
            qp: Some(qp),
            threads: 1,
            ..Config::default()
        })
        .unwrap();
        let mut bytes = 0;
        let mut ps = 0.0;
        for f in &frames {
            bytes += enc.encode(f, false).unwrap().data.len();
            ps += psnr(&f.y, &enc.recon().unwrap().y);
        }
        eprintln!(
            "{src} qp {qp}: {:.2} dB {:.0} kbps",
            ps / 120.0,
            bytes as f64 * 8.0 / 2.0 / 1000.0
        );
    }
}

/// A fast pan over a still picture (`dx`, `dy` px per frame).
pub(crate) fn pan_frames(
    big: &Yuv420Frame,
    w: u32,
    h: u32,
    n: u32,
    dx: u32,
    dy: u32,
) -> Vec<Yuv420Frame> {
    (0..n)
        .map(|t| {
            let (ox, oy) = ((t * dx) as usize, (t * dy) as usize);
            let mut f = Yuv420Frame::new(w, h);
            let bw = big.width as usize;
            for y in 0..h as usize {
                let sy = (y + oy).min(big.height as usize - 1);
                for x in 0..w as usize {
                    f.y[y * w as usize + x] = big.y[sy * bw + (x + ox).min(bw - 1)];
                }
            }
            let (cw, bcw) = (f.chroma_width(), big.chroma_width());
            for y in 0..f.chroma_height() {
                let sy = (y + oy / 2).min(big.chroma_height() - 1);
                for x in 0..cw {
                    let sx = (x + ox / 2).min(bcw - 1);
                    f.u[y * cw + x] = big.u[sy * bcw + sx];
                    f.v[y * cw + x] = big.v[sy * bcw + sx];
                }
            }
            f
        })
        .collect()
}

#[test]
#[ignore]
fn bench_pan() {
    let Some(ff) = ffmpeg() else { return };
    let big = lavfi_frames(&ff, "mandelbrot", 3840, 2160, 1).remove(0);
    let (w, h) = (1280u32, 720u32);
    let frames = pan_frames(&big, w, h, 60, 24, 10);
    for qp in [26u8, 32] {
        let mut enc = Encoder::new(Config {
            width: w,
            height: h,
            qp: Some(qp),
            threads: 1,
            ..Config::default()
        })
        .unwrap();
        let mut bytes = 0;
        let mut ps = 0.0;
        for f in &frames {
            bytes += enc.encode(f, false).unwrap().data.len();
            ps += psnr(&f.y, &enc.recon().unwrap().y);
        }
        eprintln!(
            "pan qp {qp}: {:.2} dB {:.0} kbps stats {:?}",
            ps / 60.0,
            bytes as f64 * 8.0 / 1000.0,
            enc.last_stats()
        );
    }
}
