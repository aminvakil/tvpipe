//! SIGTERM (as Ctrl-C) ends the binary cleanly: the phones are told the
//! room closed, the TV session ends, the exit status is 130. Needs a
//! `kill` command (skipped without one, e.g. on Windows).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use drawbluff::bots::Client;

#[test]
fn sigterm_closes_the_room_and_the_tv() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_drawbluff"))
        .args(["--local", "--to", "browser", "--fast", "-v"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // `phones join at  http://127.0.0.1:PORT/?r=ROOM#k=KEY`
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut join = None;
    let mut line = String::new();
    while join.is_none() {
        line.clear();
        if out.read_line(&mut line).unwrap() == 0 {
            panic!("no join link");
        }
        if let Some(url) = line.trim().strip_prefix("phones join at") {
            join = Some(url.trim().to_owned());
        }
    }
    let join = join.unwrap();
    let rest = join.strip_prefix("http://127.0.0.1:").unwrap();
    let (port, rest) = rest.split_once("/?r=").unwrap();
    let (room, key) = rest.split_once("#k=").unwrap();
    let phone = Client::join(
        &format!("ws://127.0.0.1:{port}/party/ws"),
        room,
        key,
        "Ann",
        None,
    )
    .unwrap();

    match Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
    {
        Ok(s) if s.success() => {}
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("no kill command; skipped");
            return;
        }
    }
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "still running");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(130));
    let err: Vec<String> = BufReader::new(child.stderr.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .collect();
    assert!(err.iter().any(|l| l == "drawbluff: stopping"), "{err:?}");
    assert!(
        err.iter().any(|l| l == "drawbluff: the TV session ended"),
        "{err:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while phone.ended().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(phone.ended().as_deref(), Some("room-closed"));
}
