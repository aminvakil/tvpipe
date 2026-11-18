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
    let (code, rest) = t.finish();
    assert_eq!(code, 0, "{rest:?}");
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
    let (code, rest) = t.finish();
    assert_eq!(code, 0, "{rest:?}");
    let end = rest
        .iter()
        .find(|l| l.starts_with("{\"type\":\"end\""))
        .unwrap();
    assert!(end.contains("\"reason\":\"eof\""), "{end}");
}

#[test]
fn errors_are_events_with_codes() {
    let t = Tvpipe::spawn("error", &["play", "--to", "browser", "/no/such/file.mp4"]);
    let (code, rest) = t.finish();
    assert_eq!(code, 1);
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
