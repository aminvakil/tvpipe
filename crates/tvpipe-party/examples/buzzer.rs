//! A buzzer party: phones join with the link (or the QR code printed on
//! the terminal), the VIP arms a round, and after a random wait everyone
//! races to press the button. The fastest player is printed here.
//!
//! ```text
//! cargo run -p tvpipe-party --example buzzer            # a public link, else the LAN
//! cargo run -p tvpipe-party --example buzzer -- lan     # http://IP:7420, same Wi-Fi
//! cargo run -p tvpipe-party --example buzzer -- local   # this computer only
//! cargo run -p tvpipe-party --example buzzer -- opentunnel
//! ```
//!
//! The phone page is `examples/buzzer/` (set `TVPIPE_PARTY_DEV=1` to
//! serve it from disk while editing). Ctrl-C ends the party: the phones
//! are told the room closed.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::{Duration, Instant};

use tvpipe_party::{
    assets, json, Action, Assets, Event, Join, Link, Party, PartyError, PartyOptions, PlayerId,
    Value,
};

static PHONE: Assets = assets!("examples/buzzer", ["index.html", "buzzer.js"]);

/// Where a round is.
enum Phase {
    /// Waiting for the VIP.
    Lobby,
    /// Armed: "go" at this instant (pressing now is a false start).
    Wait { go: Instant },
    /// Racing since `go` (server clock, ms).
    Go { go: Instant, at: u64 },
}

struct Game {
    phase: Phase,
    round: u64,
    /// Rounds won per player.
    wins: Vec<(PlayerId, u64)>,
}

fn main() {
    let join = match std::env::args().nth(1).as_deref() {
        None | Some("auto") => Join::Auto,
        Some("local") => Join::Local,
        Some("lan") => Join::Lan(None),
        Some(other) => match tvpipe::TunnelProvider::parse(other) {
            Some(p) => Join::Tunnel(p),
            None => {
                eprintln!("usage: buzzer [auto|lan|local|cloudflare|opentunnel]");
                std::process::exit(2);
            }
        },
    };
    let mut party = match Party::start(PartyOptions::new().phone(&PHONE).join(join)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("buzzer: {e}");
            std::process::exit(1);
        }
    };
    if let Some(qr) = party.join_qr() {
        print_qr(qr);
    }
    if party.link() == Link::Lan {
        println!("phones must be on the same Wi-Fi as this computer");
    }
    println!("join: {}", party.join_url());
    println!("local: {}", party.local_join_url());

    let mut game = Game {
        phase: Phase::Lobby,
        round: 0,
        wins: Vec::new(),
    };
    game.publish(&mut party, Value::Null);
    party.flush();
    // Ctrl-C ends the wait below (tvpipe's `signals` feature).
    let waker = party.waker();
    tvpipe::signals::on_signal(move || waker.wake());
    while !tvpipe::signals::requested() {
        let deadline = match game.phase {
            Phase::Wait { go } => go,
            _ => Instant::now() + Duration::from_secs(1),
        };
        for event in party.wait_until(deadline) {
            match event {
                Event::Joined(id) => {
                    let name = party.player(&id).map_or("?", |p| p.name()).to_owned();
                    println!("+ {name}");
                }
                Event::Left { name, .. } => println!("- {name}"),
                Event::Action(a) => game.act(&mut party, a),
                _ => {}
            }
        }
        if let Phase::Wait { go } = game.phase {
            if Instant::now() >= go {
                game.phase = Phase::Go {
                    go,
                    at: party.now(),
                };
                game.publish(&mut party, Value::Null);
            }
        }
        party.flush();
    }
    party.shutdown(Duration::from_secs(2));
    std::process::exit(130);
}

impl Game {
    fn act(&mut self, party: &mut Party, a: Action) {
        let Some(id) = a.player().cloned() else {
            return party.reply(a, Err(PartyError::new("not-player", "Phones only")));
        };
        match (a.kind(), &self.phase) {
            ("start", Phase::Lobby) => {
                if party.vip().map(|p| p.id()) != Some(&id) {
                    return party.reply(a, Err(PartyError::new("not-vip", "The VIP starts")));
                }
                self.round += 1;
                let wait = 1500 + random() % 2500;
                self.phase = Phase::Wait {
                    go: Instant::now() + Duration::from_millis(wait),
                };
                self.publish(party, Value::Null);
                party.reply(a, Ok(Value::Null));
            }
            ("start", _) => party.reply(a, Err(PartyError::new("busy", "A round is on"))),
            ("buzz", Phase::Wait { .. }) => {
                party.reply(a, Err(PartyError::new("too-early", "False start!")))
            }
            ("buzz", &Phase::Go { go, at }) => {
                let arrived = go.elapsed().as_millis() as u64;
                // The phone's own reading of the server clock at the tap;
                // trusted only within the arrival time.
                let tapped = a.data().get("at").and_then(Value::as_u64);
                let reaction = tapped.map_or(arrived, |t| t.saturating_sub(at).min(arrived));
                let name = party.player(&id).map_or("?", |p| p.name()).to_owned();
                println!(
                    "round {}: {name} in {reaction} ms (arrived after {arrived} ms)",
                    self.round
                );
                match self.wins.iter_mut().find(|(p, _)| *p == id) {
                    Some((_, n)) => *n += 1,
                    None => self.wins.push((id.clone(), 1)),
                }
                self.phase = Phase::Lobby;
                self.publish(
                    party,
                    json!({ "id": id.as_str(), "name": name, "ms": reaction }),
                );
                party.reply(a, Ok(json!({ "first": true, "ms": reaction })));
            }
            ("buzz", Phase::Lobby) => party.reply(a, Ok(json!({ "first": false }))),
            _ => party.reply(a, Err(PartyError::new("unknown-action", "Not now"))),
        }
    }

    /// The room view: the phase, the last winner, the wins.
    fn publish(&self, party: &mut Party, winner: Value) {
        let phase = match self.phase {
            Phase::Lobby => "lobby",
            Phase::Wait { .. } => "wait",
            Phase::Go { .. } => "go",
        };
        let mut wins = tvpipe_party::Object::new();
        for (id, n) in &self.wins {
            wins.insert(id.as_str(), *n);
        }
        party.room().set(json!({
            "phase": phase,
            "round": self.round,
            "wins": Value::Object(wins),
            "winner": winner,
        }));
    }
}

/// A few random bits without a crate (std's hasher keys are random).
fn random() -> u64 {
    RandomState::new().hash_one(Instant::now())
}

/// The QR code with half blocks (two modules per character).
fn print_qr(qr: &tvpipe::QrCode) {
    let n = qr.size() as isize;
    let dark = |x: isize, y: isize| {
        (0..n).contains(&x) && (0..n).contains(&y) && qr.dark(x as usize, y as usize)
    };
    let mut out = String::new();
    for y in (-2..n + 2).step_by(2) {
        for x in -2..n + 2 {
            // Light modules print dark on most terminals' light-on-dark.
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        out.push('\n');
    }
    print!("{out}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_pages_match_the_directory() {
        super::PHONE.check_dir().unwrap();
    }
}
