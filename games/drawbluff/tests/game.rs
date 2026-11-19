//! Whole games with fake phones over real WebSockets (the Rust twin of
//! `test/game.test.ts`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use drawbluff::bots::{bots, Bot, BotOptions};
use drawbluff::game::{Drawbluff, GameEvent, GameOptions, ScoreLine, Timings};
use tvpipe_party::{Join, LateJoin, Party, PartyOptions, Value};

const FAST: Timings = Timings {
    draw: 1500,
    lie: 1200,
    vote: 1200,
    reveal_step: 20,
    reveal_end: 20,
    scores: 20,
    gallery: 20,
};

type Cmd = Box<dyn FnOnce(&mut Party, &mut Drawbluff) + Send>;
/// Runs on the loop thread after every tick (to watch the views).
type Hook = Box<dyn FnMut(&mut Party) + Send>;

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
        Host::start_with(rounds, None, None)
    }

    fn start_with(rounds: u32, late_join: Option<LateJoin>, mut hook: Option<Hook>) -> Host {
        let mut party = Party::start(
            PartyOptions::new()
                .join(Join::Local)
                .max_players(50)
                .max_sockets(128),
        )
        .unwrap();
        let mut game = Drawbluff::new(GameOptions {
            rounds,
            timings: FAST,
            grace: 100,
            seed: Some(7),
            late_join,
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
                if let Some(h) = hook.as_mut() {
                    h(&mut party);
                }
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
        self.bots_thinking(n, seed, (5, 5))
    }

    fn bots_thinking(&self, n: usize, seed: u32, think: (u64, u64)) -> Vec<Bot> {
        let mut o = BotOptions::new(self.port, &self.room, &self.key);
        o.seed = seed;
        o.think = think;
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
fn eight_players_one_round_the_classic_games_most() {
    let host = Host::start(1);
    let mut o = BotOptions::new(host.port, &host.room, &host.key);
    o.seed = 8;
    o.think = (5, 5);
    let players = bots(8, &o).unwrap();
    players[0].client.act("start", Value::Null).unwrap();
    let table = host.wait_end(Duration::from_secs(60));
    assert_eq!(table.len(), 8);
    for b in &players {
        assert_eq!(b.errors(), Vec::<String>::new());
    }
}

#[test]
fn too_few_players_non_vip_start_a_late_joiner_plays_from_the_next_round() {
    let host = Host::start(2);
    let mut players = host.bots(2, 2);
    let e = players[0].client.act("start", Value::Null).unwrap_err();
    assert_eq!(e.code, "too-few");
    players.push(host.bot("Zed").unwrap());
    let e = players[1].client.act("start", Value::Null).unwrap_err();
    assert_eq!(e.code, "not-vip");
    players[0].client.act("start", Value::Null).unwrap();
    until("the first lies", || {
        matches!(host.with(|_, g| g.phase()), "lie" | "vote")
    });
    let late = host.bot("Late").unwrap();
    until("waiting", || task(&late) == "wait");
    assert_eq!(late.client.view()["me"]["note"].as_str(), Some("round"));
    let id = late.client.id();
    let waiting = move |party: &mut Party, _: &mut Drawbluff| {
        party
            .players()
            .iter()
            .find(|p| p.id().as_str() == id)
            .is_some_and(|p| p.waiting())
    };
    assert!(host.with(waiting.clone()));
    until("+ Late on the TV", || {
        host.with(|party, _| {
            party.screen_view()["screen"]["joined"]
                .as_array()
                .is_some_and(|a| a.iter().any(|n| n.as_str() == Some("Late")))
        })
    });
    let end = Instant::now() + Duration::from_secs(15);
    while task(&late) != "draw" {
        assert!(Instant::now() < end, "no second round for the late joiner");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.with(waiting));
    let table = host.wait_end(Duration::from_secs(30));
    let mut names: Vec<&str> = table.iter().map(|l| l.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Ada", "Bea", "Late", "Zed"]);
}

#[test]
fn late_join_open_dealt_in_at_the_next_drawing_first_in_the_next_rounds_spotlights() {
    // 13: pipelined, and in round 2 seven never-spotted players compete
    // for six spotlights.
    let seen: Arc<Mutex<(Vec<String>, Vec<String>)>> = Arc::default();
    let rec = seen.clone();
    let hook: Hook = Box::new(move |party: &mut Party| {
        let round = party.room().get().get("round").and_then(Value::as_u64);
        let round = round.map_or("-".to_owned(), |r| r.to_string());
        let Some(id) = party
            .players()
            .iter()
            .find(|p| p.name() == "Late")
            .map(|p| p.id().clone())
        else {
            return;
        };
        let me = Value::Object(party.player_state(&id).unwrap().get().clone());
        let what = me["key"]
            .as_str()
            .or(me["note"].as_str())
            .unwrap_or_default();
        let t = format!("{round}:{}:{what}", me["task"].as_str().unwrap_or_default());
        let s = party.screen_view()["screen"].clone();
        let tv = format!(
            "{}:{}:{}",
            s["round"].as_u64().unwrap_or(0),
            s["phase"].as_str().unwrap_or_default(),
            s["artist"]["id"].as_str().unwrap_or_default()
        );
        let mut g = rec.lock().unwrap();
        if g.0.last() != Some(&t) {
            g.0.push(t);
        }
        if g.1.last() != Some(&tv) {
            g.1.push(tv);
        }
    });
    let host = Host::start_with(2, Some(LateJoin::Open), Some(hook));
    let mut players = host.bots(13, 13);
    players[0].client.act("start", Value::Null).unwrap();
    assert!(host.with(|party, _| party.in_progress()));
    let end = Instant::now() + Duration::from_secs(15);
    while host.with(|_, g| g.phase()) != "vote" {
        assert!(Instant::now() < end, "no vote");
        thread::sleep(Duration::from_millis(2));
    }
    let running = host.with(|party, _| {
        let s = &party.screen_view()["screen"];
        format!(
            "{}.{}",
            s["round"].as_u64().unwrap_or(0),
            s["spot"].as_u64().unwrap_or(0)
        )
    });
    let late = host.bot("Late").unwrap();
    until("waiting", || task(&late) == "wait");
    assert_eq!(late.client.view()["me"]["note"].as_str(), Some("drawing"));
    let id = late.client.id();
    players.push(late);
    let table = host.wait_end(Duration::from_secs(60));
    assert!(table.iter().any(|l| l.name == "Late"));
    let (tasks, tv) = seen.lock().unwrap().clone();
    let all = tasks.join(" ");
    assert!(
        !tasks.iter().any(|t| t.starts_with("1:draw")),
        "no drawing in round 1: {all}"
    );
    assert!(
        !tasks.contains(&format!("1:vote:{running}")),
        "not in the running vote: {all}"
    );
    assert!(
        tasks
            .iter()
            .any(|t| t.starts_with("1:vote:") || t.starts_with("1:lie:")),
        "dealt in during round 1: {all}"
    );
    assert!(
        tasks.iter().any(|t| t.starts_with("2:draw")),
        "draws in round 2: {all}"
    );
    assert!(
        tv.iter()
            .any(|t| *t == format!("2:lie:{id}") || *t == format!("2:vote:{id}")),
        "a spotlight for the newcomer in round 2: {tv:?}"
    );
    assert!(!host.with(|party, _| party.in_progress()));
    for b in &players {
        assert_eq!(b.errors(), Vec::<String>::new());
    }
}

#[test]
fn late_join_closed_refused_during_the_game_welcome_after_the_vips_toggle() {
    let host = Host::start(1);
    let mut players = host.bots(3, 3);
    let room = |b: &Bot| {
        b.client.view()["room"]["lateJoin"]
            .as_str()
            .map(str::to_owned)
    };
    assert_eq!(room(&players[0]).as_deref(), Some("wait"));
    let mode = |m: &str| tvpipe_party::json!({ "mode": m });
    let e = players[1]
        .client
        .act("late-join", mode("closed"))
        .unwrap_err();
    assert_eq!(e.code, "not-vip");
    let e = players[0]
        .client
        .act("late-join", mode("maybe"))
        .unwrap_err();
    assert_eq!(e.code, "bad-mode");
    players[0].client.act("late-join", mode("closed")).unwrap();
    assert_eq!(host.with(|party, _| party.late_join()), LateJoin::Closed);
    until("the room says closed", || {
        room(&players[1]).as_deref() == Some("closed")
    });
    players[0].client.act("start", Value::Null).unwrap();
    let e = players[0]
        .client
        .act("late-join", mode("open"))
        .unwrap_err();
    assert_eq!(e.code, "running");
    let e = host.bot("Late").err().expect("refused");
    assert_eq!(e.code, "in-progress");
    host.wait_end(Duration::from_secs(30));
    let late = host.bot("Late").unwrap();
    until("final", || task(&late) == "final");
    players.push(late);
    assert_eq!(host.with(|party, _| party.players().len()), 4);
}

#[test]
fn fifty_players_spotlights_pipelined_lies_sampled_options_every_phone_busy() {
    let host = Host::start(2);
    let players = host.bots_thinking(50, 50, (150, 150));
    players[0].client.act("start", Value::Null).unwrap();
    let mut problems: Vec<String> = Vec::new();
    let mut checked = 0;
    let mut last_vote = String::new();
    let end = Instant::now() + Duration::from_secs(90);
    loop {
        let (phase, screen) = host.with(|p, g| (g.phase(), p.screen_view()["screen"].clone()));
        if phase == "final" {
            break;
        }
        assert!(Instant::now() < end, "no end; phases {:?}", host.phases());
        let key = format!("{}:{}", screen["round"], screen["spot"]);
        if phase == "vote" && key != last_vote {
            // Checked once the vote's views are out.
            thread::sleep(Duration::from_millis(60));
            last_vote = key;
            let screen = host.with(|p, _| p.screen_view()["screen"].clone());
            if screen["phase"].as_str() != Some("vote") {
                problems.push("the vote was over too soon to check".into());
                continue;
            }
            checked += 1;
            let voters: Vec<&str> = screen["waiting"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let tv: Vec<&str> = screen["options"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|o| o["id"].as_str())
                .collect();
            let mut liars = 0;
            for b in &players {
                let view = b.client.view();
                let me = &view["me"];
                let id = b.client.id();
                match me["task"].as_str().unwrap_or_default() {
                    "vote" => {
                        if !voters.contains(&id.as_str()) {
                            problems.push(format!("{id} votes but is not waited for"));
                        }
                        let opts = me["options"].as_array().unwrap();
                        if opts.len() > 6 {
                            problems.push(format!("{id} has {} options", opts.len()));
                        }
                        let order: Vec<Option<usize>> = opts
                            .iter()
                            .map(|o| tv.iter().position(|t| Some(*t) == o["id"].as_str()))
                            .collect();
                        if order.iter().any(Option::is_none)
                            || order.windows(2).any(|w| w[1] < w[0])
                        {
                            problems.push(format!("{id}: options not in the TV's order"));
                        }
                    }
                    "lie" => liars += 1,
                    "artist" => {}
                    other => problems.push(format!("{id} has nothing to do: {other}")),
                }
            }
            if screen["spot"].as_u64() < screen["spots"].as_u64() && liars == 0 {
                problems.push(format!("no liars writing during vote {}", screen["spot"]));
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    let table = host.wait_end(Duration::from_secs(5));
    assert_eq!(table.len(), 50);
    assert!(checked >= 6, "{checked} votes checked");
    assert_eq!(
        problems.iter().take(5).collect::<Vec<_>>(),
        Vec::<&String>::new()
    );
    let phases = host.phases();
    assert_eq!(phases.iter().filter(|p| **p == "reveal").count(), 12);
    assert_eq!(phases.iter().filter(|p| **p == "gallery").count(), 2);
    assert!(table[0].score < 40_000, "points scaled: {}", table[0].score);
    let screen = host.with(|p, _| p.screen_view()["screen"].clone());
    assert_eq!(screen["total"].as_u64(), Some(50));
    assert_eq!(screen["scores"].as_array().map(<[Value]>::len), Some(8));
    until("ranks of 50", || {
        players
            .iter()
            .all(|b| b.client.view()["me"]["of"].as_u64() == Some(50))
    });
    for b in &players {
        assert_eq!(b.errors(), Vec::<String>::new());
    }
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
