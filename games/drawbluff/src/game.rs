//! drawbluff on a party, for 3 to 50 players (a port of `game/game.ts`;
//! docs/drawbluff-scale.md): the lobby, then per round everyone draws
//! their prompt, a gallery shows the drawings that do not get the stage,
//! and per spotlight drawing its liars write fake titles, the others vote,
//! the truth is revealed; scores; the final podium. The host is the only
//! authority: phones and screens only ever get views.
//!
//! The views are the Node version's, field for field (the phone and web
//! screen pages are shared):
//! - room (every phone): `{phase, round, rounds, players, spot?, spots?, deadline?, drawing?, artist?, minPlayers?}`
//! - a player's own state: `{task, score, rank, of, …}`, `task` one of
//!   `lobby`, `wait` (a late joiner), `draw` (`prompt`, `deadline`, `done`),
//!   `lie` (`key`, `drawing`, `artist`, `liars`, `early`, `deadline`, `done`, `lie`),
//!   `artist` (`key`), `vote` (`key`, `options`, `deadline`, `done`, `choice`),
//!   `watch` (`note`, `result`), `scores`/`final`
//! - screen (the TV): `{phase, …}` per phase, see each method.
//!
//! The game is a state machine driven by the host's loop, as tvpipe-party
//! wants it (no async, no callbacks):
//!
//! ```no_run
//! # use drawbluff::game::{Drawbluff, GameOptions};
//! # fn run(mut party: tvpipe_party::Party) {
//! let mut game = Drawbluff::new(GameOptions::default());
//! game.open(&mut party);
//! loop {
//!     for event in party.wait_until(game.wake(&party)) {
//!         game.event(&mut party, event);
//!     }
//!     game.tick(&mut party);
//!     party.flush();
//! }
//! # }
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tvpipe_party::{
    fold, json, Action, Collect, Event, LateJoin, Moderation, Object, Party, PartyError, Player,
    PlayerId, Value,
};

use crate::rules::{
    build_options, clean_title, deal, display_title, pick_liars, plan, reveal_order, rotate, score,
    share, shuffle, title_key, validate_drawing, Choice, Drawing, Plan, Rng, Scoring, PHONE_LIES,
    POINTS_ARTIST, POINTS_FOOLED, POINTS_GUESS,
};

/// Newcomers the TV names ("+ Anna"); more: a count.
pub const JOINED_NAMES: usize = 3;

/// Drawings per gallery page on the TV.
pub const GALLERY_PAGE: usize = 8;
/// Lines of the score table on the TV.
pub const BOARD_LINES: usize = 8;
/// Voter names a reveal line lists (more: a count).
pub const REVEAL_NAMES: usize = 6;

/// How long the phases last, in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// Drawing.
    pub draw: u64,
    /// Writing lies.
    pub lie: u64,
    /// Voting.
    pub vote: u64,
    /// Between two reveal steps.
    pub reveal_step: u64,
    /// After the truth.
    pub reveal_end: u64,
    /// The scores between rounds.
    pub scores: u64,
    /// One page of the gallery.
    pub gallery: u64,
}

impl Timings {
    /// The normal game.
    pub const DEFAULT: Timings = Timings {
        draw: 90_000,
        lie: 45_000,
        vote: 25_000,
        reveal_step: 2600,
        reveal_end: 4500,
        scores: 9000,
        gallery: 5000,
    };

    /// `--fast`: short timers, for trying it out.
    pub const FAST: Timings = Timings {
        draw: 25_000,
        lie: 20_000,
        vote: 15_000,
        reveal_step: 1500,
        reveal_end: 2500,
        scores: 4000,
        gallery: 3000,
    };
}

impl Default for Timings {
    fn default() -> Timings {
        Timings::DEFAULT
    }
}

/// How a game is played.
#[derive(Debug, Clone)]
pub struct GameOptions {
    /// Rounds per game (default 2).
    pub rounds: u32,
    /// Players needed to start (default 3).
    pub min_players: usize,
    /// The phase lengths.
    pub timings: Timings,
    /// Answers accepted after a deadline, ms (default 500: the tunnel's
    /// p95 plus the TV being behind).
    pub grace: u64,
    /// A seed for the shuffles (tests); random by default.
    pub seed: Option<u32>,
    /// The prompts (default [`crate::prompts`]).
    pub prompts: Vec<String>,
    /// Who may join during a game (the VIP can change it in the lobby):
    /// `Wait` from the next round, `Open` from the next drawing, `Closed`
    /// not at all; `None` leaves the party's setting (docs/drawbluff-scale.md §9).
    pub late_join: Option<LateJoin>,
}

impl Default for GameOptions {
    fn default() -> GameOptions {
        GameOptions {
            rounds: 2,
            min_players: 3,
            timings: Timings::DEFAULT,
            grace: 500,
            seed: None,
            prompts: crate::prompts(),
            late_join: None,
        }
    }
}

/// One line of the score table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreLine {
    /// The player.
    pub id: PlayerId,
    /// Their name.
    pub name: String,
    /// Points so far.
    pub score: i64,
    /// Points this round (or since the last table).
    pub delta: i64,
}

/// What the game did, from [`Drawbluff::take_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// A phase began (`lobby`, `draw`, `gallery`, `lie`, `vote`,
    /// `reveal`, `scores`, `final`).
    Phase(&'static str),
    /// A game ended with this table.
    End(Vec<ScoreLine>),
}

/// One drawing of the round.
#[derive(Debug, Clone)]
struct Entry {
    artist: PlayerId,
    prompt: String,
    drawing: Drawing,
}

/// A spotlight's liars at work (maybe while the TV shows something else).
#[derive(Debug)]
struct Lies {
    /// Which spotlight (an index into `Table::spots`).
    index: usize,
    /// `round.spot`, the phones' key for the task.
    key: String,
    liars: Vec<PlayerId>,
    deadline: u64,
    collect: Collect<String>,
}

/// What one player did this round (the highlights).
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    fooled: i64,
    found: i64,
    votes: i64,
}

#[derive(Debug)]
enum Phase {
    Lobby,
    Draw {
        prompts: Vec<(PlayerId, String)>,
        collect: Collect<Drawing>,
    },
    Gallery {
        page: usize,
        next: u64,
    },
    /// The TV shows the drawing while its liars are still writing
    /// (`Table::lies`).
    Lie,
    Vote {
        index: usize,
        options: Vec<Choice>,
        shown: Vec<(String, Vec<String>)>,
        collect: Collect<String>,
    },
    Reveal {
        items: usize,
        step: usize,
        next: u64,
    },
    Scores {
        until: u64,
    },
    Final,
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::Lobby => "lobby",
            Phase::Draw { .. } => "draw",
            Phase::Gallery { .. } => "gallery",
            Phase::Lie => "lie",
            Phase::Vote { .. } => "vote",
            Phase::Reveal { .. } => "reveal",
            Phase::Scores { .. } => "scores",
            Phase::Final => "final",
        }
    }
}

/// The game's state apart from the phase (so the phase's collection and
/// the rest can be borrowed at once).
#[derive(Debug)]
struct Table {
    o: GameOptions,
    rng: Rng,
    deck: Vec<String>,
    /// Points per player in this game (players who left keep theirs;
    /// late joiners from their first round).
    scores: Vec<(PlayerId, i64)>,
    /// Lies written so far this game (rotation).
    lied: HashMap<PlayerId, i64>,
    /// Spotlights so far this game (rotation).
    spotted: HashMap<PlayerId, i64>,
    tally: Vec<(String, Tally)>,
    running: bool,
    round: u32,
    /// The scores when the round began.
    before: Vec<(PlayerId, i64)>,
    plan: Plan,
    /// The round's spotlight drawings, in order.
    spots: Vec<Entry>,
    /// The spotlight on now (or next).
    spot: usize,
    /// The round's other drawings.
    gallery: Vec<Entry>,
    /// The lies being written (the current or the next spotlight's).
    lies: Option<Lies>,
    events: Vec<GameEvent>,
    /// The draw phase is on.
    drawing: bool,
    /// Dealt in during a round without a drawing of their own: first in
    /// the next round's spotlights.
    fresh: Vec<PlayerId>,
    /// Names of this round's newcomers, oldest first (the TV's "+ Anna").
    joined: Vec<String>,
    /// Spotlights of this round whose vote has not started (a newcomer
    /// with `Open` can still vote).
    votes_left: usize,
}

/// A game of drawbluff on a [`Party`].
#[derive(Debug)]
pub struct Drawbluff {
    phase: Phase,
    t: Table,
}

impl Drawbluff {
    /// A game (call [`Drawbluff::open`] to show the lobby).
    pub fn new(options: GameOptions) -> Drawbluff {
        let rng = options.seed.map_or_else(Rng::from_entropy, Rng::new);
        Drawbluff {
            phase: Phase::Lobby,
            t: Table {
                o: options,
                rng,
                deck: Vec::new(),
                scores: Vec::new(),
                lied: HashMap::new(),
                spotted: HashMap::new(),
                tally: Vec::new(),
                running: false,
                round: 0,
                before: Vec::new(),
                plan: plan(0),
                spots: Vec::new(),
                spot: 0,
                gallery: Vec::new(),
                lies: None,
                events: Vec::new(),
                drawing: false,
                fresh: Vec::new(),
                joined: Vec::new(),
                votes_left: 0,
            },
        }
    }

    /// The phase (`lobby`, `draw`, `gallery`, `lie`, `vote`, `reveal`,
    /// `scores`, `final`).
    pub fn phase(&self) -> &'static str {
        self.phase.name()
    }

    /// Whether a game is on.
    pub fn running(&self) -> bool {
        self.t.running
    }

    /// The score table now.
    pub fn table(&self, party: &Party) -> Vec<ScoreLine> {
        self.t.table(party, None)
    }

    /// What happened since the last call.
    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.t.events)
    }

    /// Shows the lobby; the VIP starts the game from their phone.
    pub fn open(&mut self, party: &mut Party) {
        self.set_phase(Phase::Lobby);
        if let Some(mode) = self.t.o.late_join {
            party.set_late_join(mode);
        }
        let (min, max, rounds) = (self.t.o.min_players, party.max_players(), self.t.o.rounds);
        let late = party.late_join().as_str();
        party.room().replace(json!({
            "phase": "lobby",
            "minPlayers": min,
            "maxPlayers": max,
            "rounds": rounds,
            "lateJoin": late,
        }));
        party.screen().replace(json!({
            "phase": "lobby",
            "minPlayers": min,
            "maxPlayers": max,
        }));
        for id in ids(party.players()) {
            set_state(party, &id, json!({ "task": "lobby" }));
        }
    }

    /// When [`Drawbluff::tick`] has something to do next (a deadline, a
    /// reveal step); a second from now when nothing is due.
    pub fn wake(&self, party: &Party) -> Instant {
        let grace = self.t.o.grace;
        let lies = self
            .t
            .lies
            .as_ref()
            .and_then(|l| l.collect.deadline())
            .map(|d| d + grace);
        let due = match &self.phase {
            Phase::Draw { collect, .. } => collect.deadline().map(|d| d + grace),
            Phase::Vote { collect, .. } => collect.deadline().map(|d| d + grace),
            Phase::Gallery { next, .. } | Phase::Reveal { next, .. } => Some(*next),
            Phase::Scores { until } => Some(*until),
            Phase::Lie | Phase::Lobby | Phase::Final => None,
        };
        let due = match (due, lies) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let now = party.now();
        let wait = due.map_or(1000, |d| d.saturating_sub(now).min(1000));
        Instant::now() + Duration::from_millis(wait)
    }

    /// Handles an event of the party (actions, joins); call
    /// [`Drawbluff::tick`] after a batch.
    pub fn event(&mut self, party: &mut Party, event: Event) {
        match event {
            // Joiners in the lobby (or after the game) see the lobby;
            // during a game they wait (the party marks them waiting) until
            // they are dealt in: at the next round, or with `Open` at the
            // next drawing.
            Event::Joined(id) => {
                if !self.t.running {
                    let state = self.lobby_state(party, &id);
                    return set_state(party, &id, state);
                }
                let state = self.t.wait_state(party, &id);
                set_state(party, &id, state);
                if let Some(p) = party.player(&id) {
                    self.t.joined.push(p.name().to_owned());
                }
                let joined = self.t.joined_view();
                party.screen().set(joined);
            }
            Event::Action(a) => self.act(party, a),
            _ => {}
        }
    }

    fn lobby_state(&self, party: &Party, id: &PlayerId) -> Value {
        let task = if matches!(self.phase, Phase::Final) {
            "final"
        } else {
            "lobby"
        };
        let who = self.t.has(id).then_some(id);
        self.t.with_rank(party, who, json!({ "task": task }))
    }

    fn act(&mut self, party: &mut Party, a: Action) {
        match a.kind() {
            "start" => {
                let result = self.start(party, &a);
                party.reply(a, result.map(|()| Value::Null));
            }
            "kick" => {
                let result = self.kick(party, &a);
                party.reply(a, result.map(|()| Value::Null));
            }
            "late-join" => {
                let result = self.late_join(party, &a);
                party.reply(a, result.map(|()| Value::Null));
            }
            _ => self.answer(party, a),
        }
    }

    fn start(&mut self, party: &mut Party, a: &Action) -> Result<(), PartyError> {
        let vip = a
            .player()
            .and_then(|id| party.player(id))
            .is_some_and(Player::vip);
        if !vip {
            return Err(PartyError::new("not-vip", "Only the VIP can start"));
        }
        if self.t.running {
            return Err(PartyError::new("running", "The game is on"));
        }
        let ready = party.players().iter().filter(|p| p.connected()).count();
        let min = self.t.o.min_players;
        if ready < min {
            let n = min - ready;
            return Err(PartyError::new(
                "too-few",
                format!("Wait for {n} more player{}", if n == 1 { "" } else { "s" }),
            ));
        }
        self.play(party);
        Ok(())
    }

    fn kick(&mut self, party: &mut Party, a: &Action) -> Result<(), PartyError> {
        let from = a.player().cloned();
        let vip = from
            .as_ref()
            .and_then(|id| party.player(id))
            .is_some_and(Player::vip);
        if !vip {
            return Err(PartyError::new(
                "not-vip",
                "Only the VIP can remove players",
            ));
        }
        if self.t.running {
            return Err(PartyError::new("running", "Not during a game"));
        }
        let target = a
            .data()
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| find(party, id))
            .map(|p| p.id().clone())
            .filter(|id| Some(id) != from.as_ref())
            .ok_or_else(|| PartyError::new("no-player", "No such player"))?;
        party.kick(&target, "kicked");
        Ok(())
    }

    fn late_join(&mut self, party: &mut Party, a: &Action) -> Result<(), PartyError> {
        let vip = a
            .player()
            .and_then(|id| party.player(id))
            .is_some_and(Player::vip);
        if !vip {
            return Err(PartyError::new("not-vip", "Only the VIP can change this"));
        }
        if self.t.running {
            return Err(PartyError::new("running", "Not during a game"));
        }
        let mode = match a.data() {
            Value::Object(o) => o.get("mode").and_then(Value::as_str),
            other => other.as_str(),
        };
        let mode = mode
            .and_then(LateJoin::parse)
            .ok_or_else(|| PartyError::new("bad-mode", "closed, wait or open"))?;
        party.set_late_join(mode);
        party.room().set(json!({ "lateJoin": mode.as_str() }));
        Ok(())
    }

    /// An answer for a collection (or not now).
    fn answer(&mut self, party: &mut Party, a: Action) {
        let t = &mut self.t;
        let back = match (a.kind(), &mut self.phase) {
            ("draw", Phase::Draw { collect, .. }) => {
                let back = collect.offer(party, a, |data, _| {
                    validate_drawing(data).map(|d| (d, Value::Null))
                });
                if back.is_none() {
                    answered(party, collect, true, |_| json!({ "done": true }));
                }
                back
            }
            ("lie", phase) if t.lies.is_some() => {
                let on_screen = matches!(phase, Phase::Lie | Phase::Gallery { .. });
                // Moderation needs the party, which `offer` holds: ask first.
                let typed = lie_text(a.data());
                let moderated = match typed.as_deref() {
                    Some(text) if !text.is_empty() => Some(party.moderate(text, "lie")),
                    _ => None,
                };
                let mut lies = t.lies.take().expect("checked");
                let prompt = t.spots[lies.index].prompt.clone();
                let back = lies.collect.offer(party, a, |data, _| {
                    let text = t.lie(data, &prompt, moderated)?;
                    let reply = json!({ "text": display_title(&text) });
                    Ok((text, reply))
                });
                if back.is_none() {
                    answered(
                        party,
                        &lies.collect,
                        on_screen,
                        |text| json!({ "done": true, "lie": display_title(text) }),
                    );
                }
                t.lies = Some(lies);
                back
            }
            ("vote", Phase::Vote { shown, collect, .. }) => {
                let back = collect.offer(party, a, |data, p| {
                    let id = match data {
                        Value::Object(o) => o.get("option").unwrap_or(&Value::Null),
                        other => other,
                    };
                    let mine = shown
                        .iter()
                        .find(|(v, _)| v == p.as_str())
                        .map(|(_, ids)| ids.as_slice())
                        .unwrap_or(&[]);
                    match id.as_str() {
                        Some(id) if mine.iter().any(|o| o == id) => {
                            Ok((id.to_owned(), Value::Null))
                        }
                        _ => Err(PartyError::new("bad-vote", "Pick one of the titles")),
                    }
                });
                if back.is_none() {
                    answered(
                        party,
                        collect,
                        true,
                        |id| json!({ "done": true, "choice": id.as_str() }),
                    );
                }
                back
            }
            _ => Some(a),
        };
        if let Some(a) = back {
            let message = format!("No {} now", a.kind());
            party.reply(a, Err(PartyError::new("unknown-action", message)));
        }
    }

    /// Moves the game on: collections that are complete or past their
    /// deadline, gallery pages, reveal steps, the end of the scores.
    pub fn tick(&mut self, party: &mut Party) {
        // Each step may finish the next phase at once (everyone left).
        for _ in 0..64 {
            if !self.step(party) {
                break;
            }
        }
    }

    /// One step; whether the phase changed.
    fn step(&mut self, party: &mut Party) -> bool {
        let now = party.now();
        let done = match &self.phase {
            Phase::Draw { collect, .. } => collect.done(party),
            Phase::Vote { collect, .. } => collect.done(party),
            Phase::Lie => self.t.lies.as_ref().is_none_or(|l| l.collect.done(party)),
            Phase::Gallery { page, next } => {
                if now < *next {
                    return false;
                }
                let pages = self.t.gallery.len().div_ceil(GALLERY_PAGE);
                if page + 1 < pages {
                    let page = page + 1;
                    let items = self.t.gallery_page(party, page);
                    party
                        .screen()
                        .set(json!({ "page": page + 1, "items": items }));
                    self.phase = Phase::Gallery {
                        page,
                        next: now + self.t.o.timings.gallery,
                    };
                    return false;
                }
                true
            }
            Phase::Reveal { items, step, next } => {
                if now < *next {
                    return false;
                }
                if step < items {
                    let (items, step) = (*items, *step + 1);
                    party.screen().set(json!({ "step": step }));
                    let wait = if step < items {
                        self.t.o.timings.reveal_step
                    } else {
                        self.t.o.timings.reveal_end
                    };
                    self.phase = Phase::Reveal {
                        items,
                        step,
                        next: now + wait,
                    };
                    return false;
                }
                true
            }
            Phase::Scores { until } => now >= *until,
            Phase::Lobby | Phase::Final => false,
        };
        if !done {
            return false;
        }
        match std::mem::replace(&mut self.phase, Phase::Lobby) {
            Phase::Draw { prompts, collect } => {
                let entries: Vec<Entry> = collect
                    .take()
                    .into_iter()
                    .filter_map(|(artist, drawing)| {
                        let prompt = prompts.iter().find(|(p, _)| *p == artist)?.1.clone();
                        Some(Entry {
                            artist,
                            prompt,
                            drawing,
                        })
                    })
                    .collect();
                self.spotlights(party, entries, prompts.len());
            }
            Phase::Gallery { .. } => self.next_spot(party),
            Phase::Reveal { .. } => {
                self.t.spot += 1;
                self.next_spot(party);
            }
            Phase::Lie => self.vote(party),
            Phase::Vote {
                index,
                options,
                shown,
                collect,
            } => {
                let votes: Vec<(String, String)> = collect
                    .take()
                    .into_iter()
                    .map(|(p, o)| (p.as_str().to_owned(), o))
                    .collect();
                self.scored(party, index, &options, &shown, &votes);
            }
            Phase::Scores { .. } => {
                if self.t.round < self.t.o.rounds {
                    self.t.round += 1;
                    self.draw(party);
                } else {
                    self.finish(party);
                }
            }
            p @ (Phase::Lobby | Phase::Final) => self.phase = p,
        }
        true
    }

    fn set_phase(&mut self, phase: Phase) {
        self.t.events.push(GameEvent::Phase(phase.name()));
        self.phase = phase;
    }

    /// Plays a game with the players who are there (and those who join).
    fn play(&mut self, party: &mut Party) {
        let t = &mut self.t;
        t.running = true;
        party.set_in_progress(true);
        t.scores.clear();
        t.lied.clear();
        t.spotted.clear();
        t.fresh.clear();
        t.lies = None;
        t.deck = shuffle(&t.o.prompts, &mut t.rng);
        t.round = 1;
        self.draw(party);
    }

    fn draw(&mut self, party: &mut Party) {
        let t = &mut self.t;
        t.joined.clear();
        t.votes_left = 0;
        t.drawing = true;
        // Everybody in the room plays this round, late joiners too.
        t.deal_in(party, false);
        for id in ids(party.players()) {
            if !t.has(&id) {
                t.scores.push((id, 0));
            }
        }
        t.before = t.scores.clone();
        t.tally.clear();
        t.lies = None;
        let players = t.in_game(party);
        let prompts: Vec<(PlayerId, String)> = players
            .iter()
            .map(|p| {
                let prompt = t.deck.pop().unwrap_or_else(|| "anything you like".into());
                (p.clone(), prompt)
            })
            .collect();
        let deadline = party.now() + t.o.timings.draw;
        t.room(party, json!({ "phase": "draw", "deadline": deadline }));
        let (round, rounds) = (t.round, t.o.rounds);
        t.screen(
            party,
            json!({
                "phase": "draw",
                "round": round,
                "rounds": rounds,
                "deadline": deadline,
                "waiting": strs(&players),
                "done": [],
            }),
        );
        for (p, prompt) in &prompts {
            let state = json!({ "task": "draw", "prompt": display_title(prompt), "deadline": deadline, "done": false });
            let state = t.with_rank(party, Some(p), state);
            set_state(party, p, state);
        }
        for id in ids(party.players()) {
            if !players.contains(&id) {
                let state = t.wait_state(party, &id);
                set_state(party, &id, state);
            }
        }
        let collect = Collect::new("draw", players)
            .until(deadline)
            .grace(t.o.grace);
        self.set_phase(Phase::Draw { prompts, collect });
    }

    /// Picks the round's spotlights and gallery, then shows the gallery
    /// (the first liars write meanwhile) or the first spotlight.
    fn spotlights(&mut self, party: &mut Party, entries: Vec<Entry>, players: usize) {
        let t = &mut self.t;
        t.drawing = false;
        t.plan = plan(players);
        let live: Vec<Entry> = entries
            .into_iter()
            .filter(|e| party.player(&e.artist).is_some())
            .collect();
        let pool: Vec<String> = live.iter().map(|e| e.artist.as_str().to_owned()).collect();
        // Newcomers of the last round first, then the fewest spotlights,
        // then connected artists, then drawings with strokes.
        let key = |id: &str| {
            let e = live
                .iter()
                .find(|e| e.artist.as_str() == id)
                .expect("in pool");
            let spotted = t.spotted.get(&e.artist).copied().unwrap_or(0);
            let fresh = if t.fresh.contains(&e.artist) { 4 } else { 0 };
            let away = party.player(&e.artist).is_some_and(|p| !p.connected());
            4 * spotted - fresh + if away { 2 } else { 0 } + i64::from(e.drawing.strokes.is_empty())
        };
        let chosen = rotate(&pool, key, t.plan.spotlights, &mut t.rng);
        t.fresh.clear();
        t.votes_left = chosen.len();
        t.spots = chosen
            .iter()
            .filter_map(|id| live.iter().find(|e| e.artist.as_str() == id).cloned())
            .collect();
        for s in &t.spots {
            *t.spotted.entry(s.artist.clone()).or_default() += 1;
        }
        t.gallery = live
            .into_iter()
            .filter(|e| !chosen.iter().any(|c| c == e.artist.as_str()))
            .collect();
        t.spot = 0;
        t.lies = None;
        if !t.spots.is_empty() {
            let early = !t.gallery.is_empty();
            t.lies = Some(t.start_lies(party, 0, &[], None, early));
        }
        if self.t.gallery.is_empty() {
            self.next_spot(party);
        } else {
            self.gallery(party);
        }
    }

    /// The gallery: the round's other drawings, a page at a time.
    fn gallery(&mut self, party: &mut Party) {
        let t = &self.t;
        let count = t.gallery.len();
        let pages = count.div_ceil(GALLERY_PAGE);
        let (liars, done, artist_v) = match &t.lies {
            Some(l) => (
                strs(&l.liars)
                    .into_iter()
                    .map(Value::from)
                    .collect::<Vec<_>>(),
                answered_ids(&l.collect),
                artist(party, &t.spots[l.index].artist),
            ),
            None => (Vec::new(), Vec::new(), Value::Null),
        };
        t.room(party, json!({ "phase": "gallery", "count": count }));
        let items = t.gallery_page(party, 0);
        t.screen(
            party,
            json!({
                "phase": "gallery",
                "round": t.round,
                "rounds": t.o.rounds,
                "page": 1,
                "pages": pages,
                "count": count,
                "items": items,
                "liars": liars,
                "done": done,
                "artist": artist_v,
            }),
        );
        let busy: Vec<PlayerId> = t.lies.as_ref().map(|l| l.liars.clone()).unwrap_or_default();
        for id in ids(party.players()) {
            if busy.contains(&id) {
                continue;
            }
            let mine = t
                .gallery
                .iter()
                .position(|e| e.artist == id)
                .map(|i| Value::from(i / GALLERY_PAGE + 1))
                .unwrap_or(Value::Null);
            let state = if t.has(&id) {
                t.with_rank(
                    party,
                    Some(&id),
                    json!({ "task": "watch", "note": "gallery", "mine": mine }),
                )
            } else {
                t.wait_state(party, &id)
            };
            set_state(party, &id, state);
        }
        let next = party.now() + t.o.timings.gallery;
        self.set_phase(Phase::Gallery { page: 0, next });
    }

    /// The spotlight `t.spot` (skipping those whose artist left): its lie
    /// moment while lies are missing, else its vote; or the scores.
    fn next_spot(&mut self, party: &mut Party) {
        let t = &mut self.t;
        loop {
            if t.spot >= t.spots.len() {
                t.lies = None;
                return self.scores(party);
            }
            if party.player(&t.spots[t.spot].artist).is_none() {
                if t.lies.as_ref().is_some_and(|l| l.index == t.spot) {
                    t.lies = None;
                }
                t.spot += 1;
                continue;
            }
            break;
        }
        if t.lies.as_ref().is_none_or(|l| l.index != t.spot) {
            t.lies = Some(t.start_lies(party, t.spot, &[], None, false));
        }
        let lies = t.lies.as_ref().expect("just set");
        if lies.collect.done(party) {
            return self.vote(party);
        }
        // The TV's lie moment: the drawing, while its liars are still writing.
        let spot = &t.spots[t.spot];
        let art = artist(party, &spot.artist);
        let (index, spots, deadline) = (t.spot + 1, t.spots.len(), lies.deadline);
        t.room(
            party,
            json!({ "phase": "lie", "spot": index, "spots": spots, "deadline": deadline, "artist": art.clone() }),
        );
        t.screen(
            party,
            json!({
                "phase": "lie",
                "round": t.round,
                "spot": index,
                "spots": spots,
                "deadline": deadline,
                "drawing": spot.drawing.to_value(),
                "artist": art.clone(),
                "waiting": strs(&lies.liars),
                "done": answered_ids(&lies.collect),
            }),
        );
        let key = lies.key.clone();
        let busy = lies.liars.clone();
        for id in ids(party.players()) {
            if busy.contains(&id) {
                continue;
            }
            let state = if id == spot.artist {
                json!({ "task": "artist", "key": key.as_str() })
            } else if t.has(&id) {
                json!({ "task": "watch", "note": "rest", "artist": art.clone() })
            } else {
                let state = t.wait_state(party, &id);
                set_state(party, &id, state);
                continue;
            };
            let state = t.with_rank(party, Some(&id), state);
            set_state(party, &id, state);
        }
        self.set_phase(Phase::Lie);
    }

    /// A spotlight's vote (the next drawing's liars start writing, when
    /// pipelined).
    fn vote(&mut self, party: &mut Party) {
        let t = &mut self.t;
        let Some(lies) = t.lies.take() else {
            return self.next_spot(party);
        };
        let index = lies.index;
        let spot = t.spots[index].clone();
        let lied: Vec<(String, String)> = lies
            .collect
            .take()
            .into_iter()
            .map(|(p, text)| (p.as_str().to_owned(), text))
            .collect();
        let spares = t.spares(&spot.prompt);
        let spares = shuffle(&spares, &mut t.rng);
        let min = 4.min(lies.liars.len() + 1);
        let options = build_options(&spot.prompt, &lied, &spares, &mut t.rng, min);
        if let Some(next) = t.spots.get(index + 1) {
            if t.plan.pipelined && party.player(&next.artist).is_some() {
                t.lies =
                    Some(t.start_lies(party, index + 1, &lies.liars, Some(&spot.artist), true));
            }
        }
        // With `Open`, newcomers vote from here (never in a vote that runs).
        if party.late_join() == LateJoin::Open {
            t.deal_in(party, true);
        }
        t.votes_left = t.votes_left.saturating_sub(1);
        let writing: Vec<PlayerId> = t.lies.as_ref().map(|l| l.liars.clone()).unwrap_or_default();
        let voters: Vec<PlayerId> = t
            .in_game(party)
            .into_iter()
            .filter(|p| *p != spot.artist && !writing.contains(p))
            .collect();
        let names: Vec<String> = voters.iter().map(|p| p.as_str().to_owned()).collect();
        let shown = deal(&options, &names, PHONE_LIES, &mut t.rng);
        let art = artist(party, &spot.artist);
        let deadline = party.now() + t.o.timings.vote;
        let drawing = spot.drawing.to_value();
        let spots = t.spots.len();
        t.room(
            party,
            json!({ "phase": "vote", "spot": index + 1, "spots": spots, "deadline": deadline, "drawing": drawing.clone(), "artist": art.clone() }),
        );
        let text = |id: &str| {
            options
                .iter()
                .find(|o| o.id == id)
                .map_or("", |o| o.text.as_str())
        };
        t.screen(party, json!({
            "phase": "vote",
            "round": t.round,
            "spot": index + 1,
            "spots": spots,
            "deadline": deadline,
            "drawing": drawing,
            "artist": art,
            "options": Value::Array(options.iter().map(|o| json!({ "id": o.id.as_str(), "text": o.text.as_str() })).collect()),
            "waiting": strs(&voters),
            "done": [],
            "writing": writing.len(),
        }));
        for p in &voters {
            let ids = shown
                .iter()
                .find(|(v, _)| v == p.as_str())
                .map(|(_, ids)| ids.as_slice())
                .unwrap_or(&[]);
            let mine = Value::Array(
                ids.iter()
                    .map(|id| json!({ "id": id.as_str(), "text": text(id) }))
                    .collect(),
            );
            let state = json!({ "task": "vote", "key": lies.key.as_str(), "options": mine, "deadline": deadline, "done": false });
            let state = t.with_rank(party, Some(p), state);
            set_state(party, p, state);
        }
        if !writing.contains(&spot.artist) {
            let state = t.with_rank(
                party,
                Some(&spot.artist),
                json!({ "task": "artist", "key": lies.key.as_str() }),
            );
            set_state(party, &spot.artist, state);
        }
        for id in ids(party.players()) {
            if !t.has(&id) && !writing.contains(&id) {
                let state = t.wait_state(party, &id);
                set_state(party, &id, state);
            }
        }
        let collect = Collect::new("vote", voters)
            .until(deadline)
            .grace(t.o.grace);
        self.set_phase(Phase::Vote {
            index,
            options,
            shown,
            collect,
        });
    }

    /// Points for a spotlight, then its reveal.
    fn scored(
        &mut self,
        party: &mut Party,
        index: usize,
        options: &[Choice],
        shown: &[(String, Vec<String>)],
        votes: &[(String, String)],
    ) {
        let t = &mut self.t;
        let spot = t.spots[index].clone();
        let round = i64::from(t.round);
        let scoring = score(spot.artist.as_str(), options, shown, votes, round);
        for (id, n) in &scoring.points {
            if let Some((_, s)) = t.scores.iter_mut().find(|(p, _)| p.as_str() == id) {
                *s += n;
            }
        }
        for (voter, option) in votes {
            let Some(o) = options.iter().find(|o| o.id == *option) else {
                continue;
            };
            if !scoring.voters_of(&o.id).iter().any(|v| v == voter) {
                continue;
            }
            t.count(voter).votes += 1;
            if o.truth {
                t.count(voter).found += 1;
            } else {
                for a in &o.authors {
                    t.count(a).fooled += 1;
                }
            }
        }
        self.reveal(party, index, &spot, options, &scoring, votes);
    }

    fn reveal(
        &mut self,
        party: &mut Party,
        index: usize,
        spot: &Entry,
        options: &[Choice],
        scoring: &Scoring,
        votes: &[(String, String)],
    ) {
        let t = &self.t;
        let round = i64::from(t.round);
        let name = |id: &str| find(party, id).map_or("someone", Player::name).to_owned();
        let items: Vec<Value> = reveal_order(options, scoring)
            .into_iter()
            .map(|o| {
                let who = scoring.voters_of(&o.id);
                let n = who.len() as i64;
                let seen = scoring.seen_of(&o.id);
                json!({
                    "id": o.id.as_str(),
                    "text": o.text.as_str(),
                    "truth": o.truth,
                    "decoy": o.decoy,
                    "authors": o.authors.iter().map(|a| name(a)).collect::<Vec<_>>(),
                    "voters": who.iter().take(REVEAL_NAMES).map(|a| name(a)).collect::<Vec<_>>(),
                    "count": n,
                    "seen": seen,
                    "points": if o.truth { POINTS_GUESS * round } else { share(n, POINTS_FOOLED, seen) * round },
                    "artistPoints": if o.truth { share(n, POINTS_ARTIST, seen) * round } else { 0 },
                    "same": o.authors.len() > 1,
                })
            })
            .collect();
        let count = items.len();
        let art = artist(party, &spot.artist);
        let spots = t.spots.len();
        t.room(
            party,
            json!({ "phase": "reveal", "spot": index + 1, "spots": spots, "drawing": spot.drawing.to_value(), "artist": art.clone() }),
        );
        t.screen(
            party,
            json!({
                "phase": "reveal",
                "round": t.round,
                "spot": index + 1,
                "spots": spots,
                "drawing": spot.drawing.to_value(),
                "artist": art,
                "prompt": display_title(&spot.prompt),
                "items": items,
                "step": 0,
            }),
        );
        // The next drawing's liars who are still writing keep writing.
        let writing: Vec<PlayerId> = match &t.lies {
            Some(l) if !l.collect.done(party) => l
                .liars
                .iter()
                .filter(|p| !l.collect.answers().iter().any(|(a, _)| a == *p))
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        for id in ids(party.players()) {
            if writing.contains(&id) {
                continue;
            }
            let state = if t.has(&id) {
                let result = t.result(party, &id, spot, options, scoring, votes);
                json!({ "task": "watch", "note": "reveal", "result": result })
            } else {
                let state = t.wait_state(party, &id);
                set_state(party, &id, state);
                continue;
            };
            let state = t.with_rank(party, Some(&id), state);
            set_state(party, &id, state);
        }
        let next = party.now() + t.o.timings.reveal_step;
        self.set_phase(Phase::Reveal {
            items: count,
            step: 0,
            next,
        });
    }

    fn scores(&mut self, party: &mut Party) {
        let t = &self.t;
        let table = t.table(party, Some(&t.before));
        t.room(party, json!({ "phase": "scores" }));
        let highlights = t.highlights(party);
        t.screen(
            party,
            json!({
                "phase": "scores",
                "round": t.round,
                "rounds": t.o.rounds,
                "scores": lines(&table[..table.len().min(BOARD_LINES)]),
                "total": table.len(),
                "highlights": highlights,
                "last": t.round == t.o.rounds,
            }),
        );
        for id in ids(party.players()) {
            let state = if t.has(&id) {
                t.with_rank(party, Some(&id), json!({ "task": "scores" }))
            } else {
                t.wait_state(party, &id)
            };
            set_state(party, &id, state);
        }
        let until = party.now() + t.o.timings.scores;
        self.set_phase(Phase::Scores { until });
    }

    fn finish(&mut self, party: &mut Party) {
        let t = &self.t;
        let table = t.table(party, None);
        let late = party.late_join().as_str();
        party.room().replace(json!({
            "phase": "final",
            "minPlayers": t.o.min_players,
            "lateJoin": late,
        }));
        party.screen().replace(json!({
            "phase": "final",
            "scores": lines(&table[..table.len().min(BOARD_LINES)]),
            "total": table.len(),
        }));
        for id in ids(party.players()) {
            let who = t.has(&id).then_some(&id);
            let state = t.with_rank(party, who, json!({ "task": "final" }));
            set_state(party, &id, state);
        }
        self.set_phase(Phase::Final);
        self.t.events.push(GameEvent::End(table));
        self.t.running = false;
        party.set_in_progress(false);
        for id in ids(party.players()) {
            party.activate(&id);
        }
    }
}

impl Table {
    fn has(&self, id: &PlayerId) -> bool {
        self.scores.iter().any(|(p, _)| p == id)
    }

    fn score_of(&self, id: &PlayerId) -> i64 {
        self.scores
            .iter()
            .find(|(p, _)| p == id)
            .map_or(0, |(_, s)| *s)
    }

    /// Players of this game still in the room, in join order.
    fn in_game(&self, party: &Party) -> Vec<PlayerId> {
        party
            .players()
            .iter()
            .filter(|p| self.has(p.id()))
            .map(|p| p.id().clone())
            .collect()
    }

    /// A waiting player's task: when they get in (`note`: `drawing`,
    /// `round` or `game`).
    fn wait_state(&self, party: &Party, id: &PlayerId) -> Value {
        let open = party.late_join() == LateJoin::Open && (self.drawing || self.votes_left > 0);
        let note = if open {
            "drawing"
        } else if self.round < self.o.rounds {
            "round"
        } else {
            "game"
        };
        self.with_rank(party, Some(id), json!({ "task": "wait", "note": note }))
    }

    /// The TV's newcomers: `joined` (the last few names), `joinedMore`
    /// (how many others).
    fn joined_view(&self) -> Value {
        let n = self.joined.len();
        let last = &self.joined[n.saturating_sub(JOINED_NAMES)..];
        json!({ "joined": last.to_vec(), "joinedMore": n.saturating_sub(JOINED_NAMES) })
    }

    /// The TV's view of a phase, with the newcomers.
    fn screen(&self, party: &mut Party, fields: Value) {
        let mut o = match fields {
            Value::Object(o) => o,
            _ => Object::new(),
        };
        if let Value::Object(j) = self.joined_view() {
            for (k, v) in j.iter() {
                o.insert(k, v.clone());
            }
        }
        party.screen().replace(Value::Object(o));
    }

    /// Deals waiting players in (`Open`: at a spotlight's lies and vote;
    /// every mode: at a round's drawing): in the game from now on, their
    /// lies counted as the fewest so far (so they join the rotation).
    fn deal_in(&mut self, party: &mut Party, mid_round: bool) {
        let waiting: Vec<PlayerId> = party
            .players()
            .iter()
            .filter(|p| p.waiting())
            .map(|p| p.id().clone())
            .collect();
        if waiting.is_empty() {
            return;
        }
        let least = self
            .in_game(party)
            .iter()
            .map(|p| self.lied.get(p).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);
        for id in waiting {
            party.activate(&id);
            if self.has(&id) {
                continue;
            }
            self.scores.push((id.clone(), 0));
            self.lied.insert(id.clone(), least);
            if mid_round {
                self.fresh.push(id);
            }
        }
    }

    /// The room view: `round`, `rounds`, `players` and these fields.
    fn room(&self, party: &mut Party, fields: Value) {
        let mut o = Object::new();
        o.insert("round", self.round);
        o.insert("rounds", self.o.rounds);
        o.insert("players", self.in_game(party).len());
        if let Value::Object(f) = fields {
            for (k, v) in f.iter() {
                o.insert(k, v.clone());
            }
        }
        party.room().replace(Value::Object(o));
    }

    fn count(&mut self, id: &str) -> &mut Tally {
        let i = match self.tally.iter().position(|(p, _)| p.as_str() == id) {
            Some(i) => i,
            None => {
                self.tally.push((id.to_owned(), Tally::default()));
                self.tally.len() - 1
            }
        };
        &mut self.tally[i].1
    }

    /// Gives spotlight `index` to its liars: `busy` may not lie (the
    /// current liars), `first` lies first if they can (the current
    /// artist); `early`: the TV shows something else meanwhile.
    fn start_lies(
        &mut self,
        party: &mut Party,
        index: usize,
        busy: &[PlayerId],
        first: Option<&PlayerId>,
        early: bool,
    ) -> Lies {
        if party.late_join() == LateJoin::Open {
            self.deal_in(party, true);
        }
        let spot = self.spots[index].clone();
        let pool_ids: Vec<PlayerId> = self
            .in_game(party)
            .into_iter()
            .filter(|p| *p != spot.artist && !busy.contains(p))
            .collect();
        let pool: Vec<String> = pool_ids.iter().map(|p| p.as_str().to_owned()).collect();
        let first = first
            .filter(|f| self.has(f) && party.player(f).is_some_and(Player::connected))
            .map(PlayerId::as_str);
        // Away players last.
        let lied_so_far = &self.lied;
        let lied = |id: &str| {
            let p = find(party, id);
            let away = p.is_some_and(|p| !p.connected());
            let n = p
                .and_then(|p| lied_so_far.get(p.id()))
                .copied()
                .unwrap_or(0);
            n + if away { 1000 } else { 0 }
        };
        let picked = pick_liars(&pool, lied, self.plan.liars, &mut self.rng, first);
        let liars: Vec<PlayerId> = picked
            .iter()
            .filter_map(|s| pool_ids.iter().find(|p| p.as_str() == s).cloned())
            .collect();
        for l in &liars {
            *self.lied.entry(l.clone()).or_default() += 1;
        }
        let key = format!("{}.{}", self.round, index + 1);
        let deadline = party.now() + self.o.timings.lie;
        let art = artist(party, &spot.artist);
        for l in &liars {
            let state = json!({
                "task": "lie",
                "key": key.as_str(),
                "drawing": spot.drawing.to_value(),
                "artist": art.clone(),
                "liars": liars.len(),
                "early": early,
                "deadline": deadline,
                "done": false,
            });
            let state = self.with_rank(party, Some(l), state);
            set_state(party, l, state);
        }
        let collect = Collect::new("lie", liars.clone())
            .until(deadline)
            .grace(self.o.grace);
        Lies {
            index,
            key,
            liars,
            deadline,
            collect,
        }
    }

    /// One page of the gallery (`{id, name, prompt, drawing}`).
    fn gallery_page(&self, party: &Party, page: usize) -> Value {
        Value::Array(
            self.gallery
                .iter()
                .skip(page * GALLERY_PAGE)
                .take(GALLERY_PAGE)
                .map(|e| {
                    let name = party.player(&e.artist).map_or("someone", Player::name);
                    json!({ "id": e.artist.as_str(), "name": name, "prompt": display_title(&e.prompt), "drawing": e.drawing.to_value() })
                })
                .collect(),
        )
    }

    /// What one phone says after a vote: what you found, who fooled you,
    /// what your lie did.
    fn result(
        &self,
        party: &Party,
        id: &PlayerId,
        spot: &Entry,
        options: &[Choice],
        scoring: &Scoring,
        votes: &[(String, String)],
    ) -> Value {
        let points = scoring.points_of(id.as_str());
        if *id == spot.artist {
            let truth = options.iter().find(|o| o.truth);
            let (count, seen) = truth.map_or((0, 0), |o| {
                (scoring.voters_of(&o.id).len(), scoring.seen_of(&o.id))
            });
            return json!({ "kind": "artist", "points": points, "count": count, "seen": seen });
        }
        let me = id.as_str();
        let lie = options.iter().find(|o| o.authors.iter().any(|a| a == me));
        let choice = votes
            .iter()
            .rev()
            .find(|(v, _)| v == me)
            .map(|(_, o)| o.as_str());
        let picked = options.iter().find(|o| {
            Some(o.id.as_str()) == choice && scoring.voters_of(&o.id).iter().any(|v| v == me)
        });
        let kind = match picked {
            Some(o) if o.truth => "found",
            Some(_) => "fooled",
            None => "none",
        };
        let mut out = json!({ "kind": kind, "points": points });
        if let Some(o) = picked.filter(|o| !o.truth) {
            let by: Vec<String> = if o.decoy {
                Vec::new()
            } else {
                o.authors
                    .iter()
                    .map(|a| find(party, a).map_or("someone", Player::name).to_owned())
                    .collect()
            };
            out = out.with("by", by);
        }
        if let Some(l) = lie {
            out = out
                .with("lie", l.text.as_str())
                .with("fooledCount", scoring.voters_of(&l.id).len());
        }
        out
    }

    /// The round's highlights: the best liar, the sharpest eye.
    fn highlights(&self, party: &Party) -> Value {
        let lines: Vec<(&str, &str, Tally)> = self
            .tally
            .iter()
            .filter_map(|(id, t)| find(party, id).map(|p| (id.as_str(), p.name(), *t)))
            .collect();
        let best = |key: fn(&Tally) -> i64| {
            let mut c: Vec<&(&str, &str, Tally)> = lines.iter().filter(|l| key(&l.2) > 0).collect();
            c.sort_by(|a, b| key(&b.2).cmp(&key(&a.2)).then_with(|| name_order(a.1, b.1)));
            c.first().map(|l| (l.0.to_owned(), l.1.to_owned(), l.2))
        };
        let mut out = Vec::new();
        if let Some((id, name, t)) = best(|t| t.fooled) {
            out.push(json!({ "kind": "liar", "id": id, "name": name, "count": t.fooled }));
        }
        if let Some((id, name, t)) = best(|t| t.found) {
            out.push(
                json!({ "kind": "eye", "id": id, "name": name, "count": t.found, "of": t.votes }),
            );
        }
        Value::Array(out)
    }

    /// Prompts nobody drew that are not this one: the decoys.
    fn spares(&self, prompt: &str) -> Vec<String> {
        let key = title_key(prompt);
        self.deck
            .iter()
            .filter(|t| title_key(t) != key)
            .cloned()
            .collect()
    }

    /// Checks a lie (`{text}` or a string).
    fn lie(
        &self,
        data: &Value,
        prompt: &str,
        moderated: Option<Moderation>,
    ) -> Result<String, PartyError> {
        // Kept as typed; shown and compared in the display form.
        let text = lie_text(data).unwrap_or_default();
        if display_title(&text).is_empty() {
            return Err(PartyError::new("empty", "Write a title"));
        }
        if title_key(&text) == title_key(prompt) {
            return Err(PartyError::new(
                "truth",
                "That's the real title! Write a lie instead.",
            ));
        }
        if title_key(&text).is_empty() {
            return Err(PartyError::new("empty", "Write a title with letters"));
        }
        match moderated {
            Some(Moderation::Refuse) => Err(PartyError::new("refused", "Try another one")),
            Some(Moderation::Replace(other)) => Ok(clean_title(&other)),
            _ => Ok(text),
        }
    }

    /// The table: players still in the party, best first.
    fn table(&self, party: &Party, before: Option<&[(PlayerId, i64)]>) -> Vec<ScoreLine> {
        let mut table: Vec<ScoreLine> = party
            .players()
            .iter()
            .filter(|p| self.has(p.id()))
            .map(|p| {
                let score = self.score_of(p.id());
                let was = before
                    .and_then(|b| b.iter().find(|(id, _)| id == p.id()))
                    .map_or(0, |(_, s)| *s);
                ScoreLine {
                    id: p.id().clone(),
                    name: p.name().to_owned(),
                    score,
                    delta: score - was,
                }
            })
            .collect();
        table.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| name_order(&a.name, &b.name))
        });
        table
    }

    /// A player's score, rank (1 + how many have more) and the number of
    /// players; nobody's: 0.
    fn rank(&self, party: &Party, id: Option<&PlayerId>) -> (i64, usize, usize) {
        let s = id.map_or(0, |id| self.score_of(id));
        let scores: Vec<i64> = self
            .scores
            .iter()
            .filter(|(p, _)| party.player(p).is_some())
            .map(|(_, s)| *s)
            .collect();
        let above = scores.iter().filter(|&&x| x > s).count();
        (s, 1 + above, scores.len())
    }

    /// A state with `score`, `rank` and `of`.
    fn with_rank(&self, party: &Party, id: Option<&PlayerId>, state: Value) -> Value {
        let (score, rank, of) = self.rank(party, id);
        state.with("score", score).with("rank", rank).with("of", of)
    }
}

/// `localeCompare`, roughly: case and accents last.
fn name_order(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| fold(s).to_lowercase();
    key(a).cmp(&key(b)).then(a.cmp(b))
}

/// The ids that answered a collection.
fn answered_ids<T>(collect: &Collect<T>) -> Vec<Value> {
    collect
        .answers()
        .iter()
        .map(|(p, _)| Value::from(p.as_str()))
        .collect()
}

/// The text of a typed lie: `{text}` or a string, cleaned.
fn lie_text(data: &Value) -> Option<String> {
    let text = match data {
        Value::Object(o) => o.get("text")?,
        other => other,
    };
    text.as_str().map(clean_title)
}

/// After an accepted answer: the player's state and (when the TV shows
/// this collection) the screen's `done`.
fn answered<T>(
    party: &mut Party,
    collect: &Collect<T>,
    on_screen: bool,
    state: impl FnOnce(&T) -> Value,
) {
    let Some((p, value)) = collect.answers().last() else {
        return;
    };
    let state = state(value);
    if let Some(s) = party.player_state(p) {
        s.set(state);
    }
    if on_screen {
        let done: Vec<&str> = collect.answers().iter().map(|(p, _)| p.as_str()).collect();
        party.screen().set(json!({ "done": done }));
    }
}

/// Replaces a player's own state.
fn set_state(party: &mut Party, id: &PlayerId, value: Value) {
    if let Some(s) = party.player_state(id) {
        s.replace(value);
    }
}

fn find<'a>(party: &'a Party, id: &str) -> Option<&'a Player> {
    party.players().iter().find(|p| p.id().as_str() == id)
}

fn ids(players: &[Player]) -> Vec<PlayerId> {
    players.iter().map(|p| p.id().clone()).collect()
}

fn strs(ids: &[PlayerId]) -> Vec<&str> {
    ids.iter().map(PlayerId::as_str).collect()
}

fn artist(party: &Party, id: &PlayerId) -> Value {
    let name = party.player(id).map_or("someone", Player::name);
    json!({ "id": id.as_str(), "name": name })
}

fn lines(table: &[ScoreLine]) -> Value {
    Value::Array(
        table
            .iter()
            .map(|l| {
                let mut o = Object::new();
                o.insert("id", l.id.as_str());
                o.insert("name", l.name.as_str());
                o.insert("score", l.score);
                o.insert("delta", l.delta);
                Value::Object(o)
            })
            .collect(),
    )
}
