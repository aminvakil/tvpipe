//! A joystick party: each phone is a stick whose position streams to the
//! host as continuous input (`party.input` on the phone, [`Event::Input`]
//! here), over WebRTC data channels on the LAN with the `webrtc` feature,
//! else over the WebSocket. The terminal shows every stick, and whether
//! it arrives over the fast path (⚡).
//!
//! ```text
//! cargo run -p tvpipe-party --features webrtc --example stick -- lan
//! cargo run -p tvpipe-party --example stick -- local   # WebSocket only
//! ```
//!
//! Phones send their clock with each update; the `stats` action answers
//! the input latency the host saw since the last `stats` (phones on this
//! computer share its clock: scripts/live/party-rtc.ts measures with it).

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tvpipe_party::{assets, json, Assets, Event, Join, Link, Party, PartyOptions, PlayerId, Value};

static PHONE: Assets = assets!("examples/stick", ["index.html", "stick.js"]);

/// What arrived from one phone since the last `stats`.
#[derive(Default)]
struct Seen {
    latencies: Vec<f64>,
    first: Option<i64>,
    last: i64,
}

fn main() {
    let join = match std::env::args().nth(1).as_deref() {
        None | Some("auto") => Join::Auto,
        Some("local") => Join::Local,
        Some("lan") => Join::Lan(None),
        Some(other) => match tvpipe::TunnelProvider::parse(other) {
            Some(p) => Join::Tunnel(p),
            None => {
                eprintln!("usage: stick [auto|lan|local|cloudflare|opentunnel]");
                std::process::exit(2);
            }
        },
    };
    let mut party = match Party::start(PartyOptions::new().phone(&PHONE).join(join)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("stick: {e}");
            std::process::exit(1);
        }
    };
    if party.link() == Link::Lan {
        println!("phones must be on the same Wi-Fi as this computer");
    }
    #[cfg(feature = "webrtc")]
    println!("fast path: {:?}", party.fast_addrs());
    println!("join: {}", party.join_url());

    let waker = party.waker();
    tvpipe::signals::on_signal(move || waker.wake());
    let mut sticks: BTreeMap<PlayerId, (f64, f64)> = BTreeMap::new();
    let mut seen: BTreeMap<PlayerId, Seen> = BTreeMap::new();
    let mut shown = Instant::now();
    while !tvpipe::signals::requested() {
        for event in party.wait(Duration::from_millis(250)) {
            match event {
                Event::Joined(id) => {
                    println!("+ {}", party.player(&id).map_or("?", |p| p.name()));
                }
                Event::Input { player, data } => {
                    let f = |k: &str| data.get(k).and_then(Value::as_f64);
                    sticks.insert(
                        player.clone(),
                        (f("x").unwrap_or(0.0), f("y").unwrap_or(0.0)),
                    );
                    let s = seen.entry(player).or_default();
                    if let Some(t) = f("t") {
                        s.latencies.push(now_ms() - t);
                    }
                    if let Some(n) = data.get("n").and_then(Value::as_i64) {
                        s.first.get_or_insert(n);
                        s.last = s.last.max(n);
                    }
                }
                Event::Action(a) => {
                    let reply = match (a.kind(), a.player().cloned()) {
                        ("echo", _) => Ok(a.data().clone()),
                        ("stats", Some(id)) => Ok(stats(seen.remove(&id).unwrap_or_default())),
                        _ => Err(tvpipe_party::PartyError::new("unknown-action", "No")),
                    };
                    party.reply(a, reply);
                }
                _ => {}
            }
        }
        party.flush();
        if shown.elapsed() >= Duration::from_secs(1) && !sticks.is_empty() {
            shown = Instant::now();
            let line: Vec<String> = sticks
                .iter()
                .map(|(id, (x, y))| {
                    let name = party.player(id).map_or("?", |p| p.name());
                    #[cfg(feature = "webrtc")]
                    let fast = if party.fast_path(id) { " ⚡" } else { "" };
                    #[cfg(not(feature = "webrtc"))]
                    let fast = "";
                    format!("{name}{fast} {x:+.2},{y:+.2}")
                })
                .collect();
            println!("{}", line.join("   "));
        }
    }
    party.shutdown(Duration::from_secs(2));
    std::process::exit(130);
}

/// `{count, missing, p50, p95, max}`: latencies in ms; `missing` counts
/// sequence numbers that never arrived (lost, or coalesced in one poll).
fn stats(mut s: Seen) -> Value {
    s.latencies.sort_by(f64::total_cmp);
    let at = |q: f64| {
        let n = s.latencies.len();
        if n == 0 {
            return Value::Null;
        }
        let v = s.latencies[((n - 1) as f64 * q).round() as usize];
        json!((v * 100.0).round() / 100.0)
    };
    let count = s.latencies.len() as i64;
    let missing = s.first.map_or(0, |f| s.last - f + 1 - count);
    json!({
        "count": count,
        "missing": missing,
        "p50": at(0.5),
        "p95": at(0.95),
        "max": at(1.0),
    })
}

/// Milliseconds since the epoch, with the fraction (phones send
/// `performance.timeOrigin + performance.now()`).
fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_pages_match_the_directory() {
        super::PHONE.check_dir().unwrap();
    }
}
