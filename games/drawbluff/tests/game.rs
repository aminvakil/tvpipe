//! Whole games with fake phones over real WebSockets (the Rust twin of
//! `test/game.test.ts`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use drawbluff::bots::{bots, Bot, BotOptions};
use drawbluff::game::{Drawbluff, GameEvent, GameOptions, ScoreLine, Timings};
use tvpipe_party::{Join, Party, PartyOptions, Value};

const FAST: Timings = Timings {
    draw: 1500,
    lie: 1200,
    vote: 1200,
    reveal_step: 20,
    reveal_end: 20,
    scores: 20,
};

type Cmd = Box<dyn FnOnce(&mut Party, &mut Drawbluff) + Send>;

/// The party and the game on a loop thread, as `main` runs them.
struct Host {
    cmds: mpsc::Sender<Cmd>,
    events: Arc<Mutex<Vec<GameEvent>>>,
    stop: Arc<AtomicBool>,
    port: u16,
    room: String,
    key: String,
}

impl Host {
    fn start(rounds: u32) -> Host {
        let mut party = Party::start(PartyOptions::new().join(Join::Local)).unwrap();
        let mut game = Drawbluff::new(GameOptions {
            rounds,
            timings: FAST,
            grace: 100,
            seed: Some(7),
            ..GameOptions::default()
        });
        game.open(&mut party);
        party.flush();
        let (port, room, key) = (
            party.port(),
            party.code().to_owned(),
            party.join_key().to_owned(),
        );
        let (cmds, rx) = mpsc::channel::<Cmd>();
        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (ev, st) = (events.clone(), stop.clone());
        thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                let wake = game
                    .wake(&party)
                    .min(Instant::now() + Duration::from_millis(5));
                for e in party.wait_until(wake) {
                    game.event(&mut party, e);
                }
                while let Ok(cmd) = rx.try_recv() {
                    cmd(&mut party, &mut game);
                }
                game.tick(&mut party);
                ev.lock().unwrap().extend(game.take_events());
                party.flush();
            }
            party.close();
            party.flush();
        });
        Host {
            cmds,
            events,
            stop,
            port,
            room,
            key,
        }
    }

    fn with<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Party, &mut Drawbluff) -> R + Send + 'static,
    ) -> R {
        let (tx, rx) = mpsc::channel();
        self.cmds
            .send(Box::new(move |p, g| {
                let _ = tx.send(f(p, g));
            }))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn bots(&self, n: usize, seed: u32) -> Vec<Bot> {
        let mut o = BotOptions::new(self.port, &self.room, &self.key);
        o.seed = seed;
        o.think = (5, 5);
        bots(n, &o).unwrap()
    }

    fn bot(&self, name: &str) -> Result<Bot, tvpipe_party::PartyError> {
        let mut o = BotOptions::new(self.port, &self.room, &self.key);
        o.think = (5, 5);
        Bot::join(name, o)
    }

    fn phases(&self) -> Vec<&'static str> {
        let events = self.events.lock().unwrap();
        events
            .iter()
            .filter_map(|e| match e {
                GameEvent::Phase(p) => Some(*p),
                GameEvent::End(_) => None,
            })
            .collect()
    }

    fn wait_end(&self, timeout: Duration) -> Vec<ScoreLine> {
        let end = Instant::now() + timeout;
        loop {
            let found = self.events.lock().unwrap().iter().find_map(|e| match e {
                GameEvent::End(t) => Some(t.clone()),
                GameEvent::Phase(_) => None,
            });
            if let Some(t) = found {
                return t;
            }
            assert!(Instant::now() < end, "no end; phases {:?}", self.phases());
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !cond() {
        assert!(Instant::now() < end, "timed out: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn task(b: &Bot) -> String {
    b.client.view()["me"]["task"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_whole_game_with_3_players() {
    let host = Host::start(2);
    let players = host.bots(3, 3);
    players[0].client.act("start", Value::Null).unwrap();
    let table = host.wait_end(Duration::from_secs(30));
    assert_eq!(table.len(), 3);
    let phases = host.phases();
    assert_eq!(phases.iter().filter(|p| **p == "draw").count(), 2);
    assert_eq!(
        phases.iter().filter(|p| **p == "reveal").count(),
        6,
        "each player's drawing, twice"
    );
    assert!(
        table.iter().map(|l| l.score).sum::<i64>() > 0,
        "someone scored"
    );
    for b in &players {
        assert_eq!(b.errors(), Vec::<String>::new());
    }
    // Phones show the final screen with their rank.
    until("final on phones", || {
        players.iter().all(|b| task(b) == "final")
    });
    let (phase, screen, locked) = host.with(|party, game| {
        (
            game.phase(),
            party.screen_view()["screen"]["phase"].clone(),
            party.locked(),
        )
    });
    assert_eq!(phase, "final");
    assert_eq!(screen.as_str(), Some("final"));
    assert!(!locked);
    // A new game can start from the podium.
    players[0].client.act("start", Value::Null).unwrap();
    until("a second game", || host.with(|_, g| g.phase()) == "draw");
}

#[test]
fn eight_players_one_round_lies_for_me() {
    let host = Host::start(1);
    let mut o = BotOptions::new(host.port, &host.room, &host.key);
    o.seed = 8;
    o.think = (5, 5);
    o.auto_lie = true;
    let players = bots(8, &o).unwrap();
    players[0].client.act("start", Value::Null).unwrap();
    let table = host.wait_end(Duration::from_secs(60));
    assert_eq!(table.len(), 8);
    for b in &players {
        assert_eq!(b.errors(), Vec::<String>::new());
    }
}

#[test]
fn too_few_players_non_vip_start_join_while_running() {
    let host = Host::start(1);
    let mut players = host.bots(2, 2);
    let e = players[0].client.act("start", Value::Null).unwrap_err();
    assert_eq!(e.code, "too-few");
    players.push(host.bot("Zed").unwrap());
    let e = players[1].client.act("start", Value::Null).unwrap_err();
    assert_eq!(e.code, "not-vip");
    players[0].client.act("start", Value::Null).unwrap();
    let e = host.bot("Late").err().unwrap();
    assert_eq!(e.code, "locked");
    host.wait_end(Duration::from_secs(30));
}

#[test]
fn a_silent_player_and_one_who_drops_deadlines_move_the_game_on() {
    let host = Host::start(1);
    let players = host.bots(4, 4);
    players[3].close(); // gone: never answers, never comes back
    let t0 = Instant::now();
    players[0].client.act("start", Value::Null).unwrap();
    // A player who drops during the drawing phase and comes back.
    players[2].client.drop_connection();
    let table = host.wait_end(Duration::from_secs(30));
    assert_eq!(table.len(), 4, "the away player keeps their place");
    assert!(
        t0.elapsed() >= Duration::from_millis(FAST.draw),
        "the draw phase waited for the deadline"
    );
    until("back", || players[2].client.connected());
}

#[test]
fn vip_removes_a_player_in_the_lobby() {
    let host = Host::start(1);
    let players = host.bots(3, 3);
    let id = players[2].client.id();
    players[0]
        .client
        .act("kick", tvpipe_party::json!({ "id": id }))
        .unwrap();
    assert_eq!(host.with(|p, _| p.players().len()), 2);
    let id = players[0].client.id();
    let e = players[1]
        .client
        .act("kick", tvpipe_party::json!({ "id": id }))
        .unwrap_err();
    assert_eq!(e.code, "not-vip");
    until("kicked", || {
        players[2].client.ended().as_deref() == Some("kicked")
    });
}
