//! The machine interface (`--machine`, docs/machine.md) against the real
//! binary: events on stdout, commands on stdin, the stdin-EOF watchdog.
//! Only `--to browser` (127.0.0.1) receivers; no network, no ffmpeg.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

struct Tvpipe {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<String>,
    seen: Vec<String>,
}

fn config_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("machine-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

impl Tvpipe {
    fn spawn(name: &str, args: &[&str]) -> Tvpipe {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tvpipe"))
            .arg("--machine")
            .args(args)
            .env("TVPIPE_CONFIG_DIR", config_dir(name))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(b"SLRW");
        Tvpipe {
            child,
            stdin: Some(stdin),
            events,
            seen: Vec::new(),
        }
    }

    /// Waits for an event line containing `needle`; returns it.
    fn wait_for(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(line) => {
                    assert!(
                        line.starts_with("{\"type\":\""),
                        "not an event line: {line}"
                    );
                    self.seen.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(_) => panic!("no {needle} event; saw:\n{}", self.seen.join("\n")),
            }
        }
    }

    fn record(&mut self, kind: u8, pts: u64, payload: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        let mut head = vec![kind];
        head.extend_from_slice(&pts.to_le_bytes());
        head.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        stdin.write_all(&head).unwrap();
        stdin.write_all(payload).unwrap();
        stdin.flush().unwrap();
    }

    fn command(&mut self, json: &str) {
        self.record(6, 0, json.as_bytes());
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The exit status and the events after the last one read.
    fn finish(mut self) -> (i32, Vec<String>) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("tvpipe did not exit; saw:\n{}", self.seen.join("\n"));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let rest: Vec<String> = self.events.iter().collect();
        (status.code().unwrap_or(-1), rest)
    }
}

/// An event line's `type`.
fn type_of(line: &str) -> &str {
    let start = "{\"type\":\"".len();
    line.get(start..)
        .and_then(|s| s.split('"').next())
        .unwrap_or("")
}

/// Replaces what changes from run to run: ports, tokens, the version.
fn scrub(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find("127.0.0.1:") {
        out.push_str(&rest[..i + 10]);
        rest = &rest[i + 10..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        out.push_str("8765");
        rest = &rest[digits..];
    }
    out.push_str(rest);
    let mut s = String::new();
    let mut rest = out.as_str();
    while let Some(i) = rest.find("t=") {
        s.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
        let hex = rest.chars().take_while(char::is_ascii_hexdigit).count();
        if hex == 32 {
            s.push_str("0123456789abcdef0123456789abcdef");
        } else {
            s.push_str(&rest[..hex]);
        }
        rest = &rest[hex..];
    }
    s.push_str(rest);
    s.replace(
        &format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION")),
        "\"version\":\"0.0.0\"",
    )
}

/// The session as a transcript for the Node SDK's replay tests
/// (crates/tvpipe/tests/vectors/machine/sessions/NAME.txt): the events
/// without log and stats lines, scrubbed, after the command line and the
/// expected outcome. `TVPIPE_BLESS=1` writes it; otherwise the sequence of
/// event types must match the file's.
fn transcript(name: &str, args: &[&str], lines: &[String], exit: i32, outcome: &str) {
    let events: Vec<String> = lines
        .iter()
        .filter(|l| !matches!(type_of(l), "log" | "stats"))
        .map(|l| scrub(l))
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tvpipe/tests/vectors/machine/sessions")
        .join(format!("{name}.txt"));
    if std::env::var_os("TVPIPE_BLESS").is_some() {
        let mut text = format!(
            "# A recorded `tvpipe --machine` session (crates/tvpipe-cli/tests/machine.rs,\n\
             # TVPIPE_BLESS=1); ports and tokens scrubbed, log and stats lines left out.\n\
             # args: {}\n# exit: {exit}\n# outcome: {outcome}\n",
            args.join(" ")
        );
        for e in &events {
            text.push_str(e);
            text.push('\n');
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (TVPIPE_BLESS=1)", path.display()))
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| type_of(l).to_owned())
        .collect();
    let have: Vec<String> = events.iter().map(|l| type_of(l).to_owned()).collect();
    assert_eq!(have, want, "{} (TVPIPE_BLESS=1 to update)", path.display());
}

fn last_type(lines: &[String]) -> &str {
    let last = lines.last().map(String::as_str).unwrap_or("");
    let start = "{\"type\":\"".len();
    last.get(start..)
        .and_then(|s| s.split('"').next())
        .unwrap_or("")
}

#[test]
fn stop_command_ends_the_session_cleanly() {
    let mut t = Tvpipe::spawn(
        "stop",
        &[
            "test-pattern",
            "--to",
            "browser",
            "--port",
            "0",
            "--size",
            "320x240",
        ],
    );
    let hello = t.wait_for("\"hello\"");
    assert!(hello.contains("\"protocol\":1"), "{hello}");
    assert!(hello.contains("\"command\":\"test-pattern\""), "{hello}");
    let player = t.wait_for("\"player\"");
    assert!(player.contains("\"url\":\"http://127.0.0.1:"), "{player}");
    t.wait_for("\"connected\"");
    let started = t.wait_for("\"started\"");
    assert!(started.contains("\"route\":\"builtin\""), "{started}");
    assert!(started.contains("\"width\":320"), "{started}");
    t.wait_for("\"stats\"");
    t.command(r#"{"type":"stop"}"#);
    let seen = t.seen.clone();
    let (code, rest) = t.finish();
    assert_eq!(code, 0, "{rest:?}");
    transcript(
        "test-pattern-stopped",
        &[
            "test-pattern",
            "--to",
            "browser",
            "--port",
            "0",
            "--size",
            "320x240",
        ],
        &[seen, rest.clone()].concat(),
        code,
        "end stopped",
    );
    let end = rest
        .iter()
        .find(|l| l.starts_with("{\"type\":\"end\""))
        .unwrap_or_else(|| panic!("no end event: {rest:?}"));
    assert!(end.contains("\"reason\":\"stopped\""), "{end}");
}

#[test]
fn stdin_eof_is_the_watchdog() {
    let mut t = Tvpipe::spawn(
        "eof",
        &[
            "test-pattern",
            "--to",
            "browser",
            "--port",
            "0",
            "--size",
            "320x240",
        ],
    );
    t.wait_for("\"started\"");
    t.close_stdin();
    let (code, rest) = t.finish();
    assert_eq!(code, 0);
    assert!(
        rest.iter().any(|l| l.contains("\"reason\":\"stopped\"")),
        "{rest:?}"
    );
}

#[test]
fn stop_while_waiting_for_a_viewer() {
    let mut t = Tvpipe::spawn(
        "wait",
        &[
            "test-pattern",
            "--to",
            "browser",
            "--port",
            "0",
            "--wait-for-viewer",
        ],
    );
    t.wait_for("\"player\"");
    t.command(r#"{"type":"stop"}"#);
    let (code, rest) = t.finish();
    assert_eq!(code, 0, "{rest:?}");
    assert_eq!(last_type(&rest), "end", "{rest:?}");
}

#[test]
fn raw_slrw_frames_play_out_on_eof() {
    let (w, h) = (64u32, 48u32);
    let video = format!("{w}x{h}@30:rgba");
    let mut t = Tvpipe::spawn(
        "raw",
        &[
            "play",
            "--to",
            "browser",
            "--port",
            "0",
            "--encoder",
            "builtin",
            "--raw-video",
            &video,
            "--raw-audio",
            "48000:2:s16le",
            "-",
        ],
    );
    t.wait_for("\"connected\"");
    let frame = vec![0x80u8; (w * h * 4) as usize];
    let pcm = vec![0u8; 1600 * 4];
    for i in 0..45u64 {
        let pts = i * 1_000_000 / 30;
        t.record(1, pts, &frame);
        t.record(2, pts, &pcm);
        std::thread::sleep(Duration::from_millis(33));
    }
    t.wait_for("\"started\"");
    t.close_stdin();
    let seen = t.seen.clone();
    let (code, rest) = t.finish();
    assert_eq!(code, 0, "{rest:?}");
    let end = rest
        .iter()
        .find(|l| l.starts_with("{\"type\":\"end\""))
        .unwrap();
    assert!(end.contains("\"reason\":\"eof\""), "{end}");
    transcript(
        "raw-slrw-eof",
        &[
            "play",
            "--to",
            "browser",
            "--port",
            "0",
            "--raw-video",
            &video,
            "-",
        ],
        &[seen, rest.clone()].concat(),
        code,
        "end eof",
    );
}

#[test]
fn errors_are_events_with_codes() {
    let t = Tvpipe::spawn("error", &["play", "--to", "browser", "/no/such/file.mp4"]);
    let (code, rest) = t.finish();
    assert_eq!(code, 1);
    transcript(
        "input-not-found",
        &["play", "--to", "browser", "/no/such/file.mp4"],
        &rest,
        code,
        "error input-not-found",
    );
    let err = rest.last().unwrap();
    assert!(err.starts_with("{\"type\":\"error\""), "{rest:?}");
    assert!(err.contains("\"code\":\"input-not-found\""), "{err}");
    assert!(err.contains("\"kind\":\"not-found\""), "{err}");
}

#[test]
fn malformed_stdin_is_an_error() {
    let mut t = Tvpipe::spawn(
        "malformed",
        &[
            "test-pattern",
            "--to",
            "browser",
            "--port",
            "0",
            "--size",
            "320x240",
        ],
    );
    t.wait_for("\"started\"");
    t.record(42, 0, b"");
    let (code, rest) = t.finish();
    assert_eq!(code, 1, "{rest:?}");
    let err = rest.last().unwrap();
    assert!(err.contains("\"code\":\"bad-input\""), "{err}");
}

#[test]
fn qr_rows_and_svg_on_stdout() {
    let out = Command::new(env!("CARGO_BIN_EXE_tvpipe"))
        .args(["qr", "--rows", "tvpipe"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 21);
    assert!(rows
        .iter()
        .all(|r| r.len() == 21 && r.bytes().all(|b| b == b'0' || b == b'1')));
    assert!(rows[0].starts_with("1111111"));
    let out = Command::new(env!("CARGO_BIN_EXE_tvpipe"))
        .args(["qr", "--svg", "tvpipe"])
        .output()
        .unwrap();
    assert!(String::from_utf8(out.stdout).unwrap().starts_with("<svg "));
    // Too long for any QR code: an error, not a panic.
    let out = Command::new(env!("CARGO_BIN_EXE_tvpipe"))
        .args(["qr", "--rows", &"x".repeat(3000)])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    // stdout belongs to events with --machine.
    let mut t = Tvpipe::spawn("qr", &["qr", "--rows", "tvpipe"]);
    t.close_stdin();
    let (code, rest) = t.finish();
    assert_eq!(code, 1);
    assert!(rest.iter().all(|l| l.starts_with("{\"type\":")), "{rest:?}");
    assert!(
        rest.iter().any(|l| l.contains("\"type\":\"error\"")),
        "{rest:?}"
    );
}
