//! drawbluff on a party (a port of `game/game.ts`): the lobby, then per
//! round everyone draws their prompt, and per drawing the others lie,
//! everyone votes, the truth is revealed; scores; the final podium. The
//! host is the only authority: phones and screens only ever get views.
//!
//! The views are the Node version's, field for field (the phone and web
//! screen pages are shared):
//! - room (every phone): `{phase, round, rounds, deadline?, drawing?, artist?, minPlayers?}`
//! - a player's own state: `{task, …}`, `task` one of `lobby`, `draw`
//!   (`prompt`, `done`), `lie` (`done`, `lie`), `artist`, `vote`
//!   (`options`, `done`, `choice`), `watch`, `scores`/`final` (`score`, `rank`)
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

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tvpipe_party::{
    fold, json, Action, Collect, Event, Moderation, Object, Party, PartyError, Player, PlayerId,
    Value,
};

use crate::rules::{
    build_options, clean_title, options_for, reveal_order, score, shuffle, title_key,
    validate_drawing, Choice, Drawing, Rng, POINTS_ARTIST, POINTS_FOOLED, POINTS_GUESS,
};

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
    };

    /// `--fast`: short timers, for trying it out.
    pub const FAST: Timings = Timings {
        draw: 25_000,
        lie: 20_000,
        vote: 15_000,
        reveal_step: 1500,
        reveal_end: 2500,
        scores: 4000,
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
    /// A phase began (`lobby`, `draw`, `lie`, `vote`, `reveal`, `scores`, `final`).
    Phase(&'static str),
    /// A game ended with this table.
    End(Vec<ScoreLine>),
}

/// One drawing to guess.
#[derive(Debug, Clone)]
struct Turn {
    artist: PlayerId,
    prompt: String,
    drawing: Drawing,
}

#[derive(Debug)]
enum Phase {
    Lobby,
    Draw {
        prompts: Vec<(PlayerId, String)>,
        collect: Collect<Drawing>,
    },
    Lie {
        turn: Turn,
        others: Vec<PlayerId>,
        collect: Collect<String>,
    },
    Vote {
        turn: Turn,
        options: Vec<Choice>,
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
            Phase::Lie { .. } => "lie",
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
    /// Points per player in this game (players who left keep theirs).
    scores: Vec<(PlayerId, i64)>,
    running: bool,
    round: u32,
    /// The scores when the round began.
    before: Vec<(PlayerId, i64)>,
    turns: VecDeque<Turn>,
    events: Vec<GameEvent>,
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
                running: false,
                round: 0,
                before: Vec::new(),
                turns: VecDeque::new(),
                events: Vec::new(),
            },
        }
    }

    /// The phase (`lobby`, `draw`, `lie`, `vote`, `reveal`, `scores`, `final`).
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
        let (min, max, rounds) = (self.t.o.min_players, party.max_players(), self.t.o.rounds);
        party.room().replace(json!({
            "phase": "lobby",
            "minPlayers": min,
            "maxPlayers": max,
            "rounds": rounds,
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
        let due = match &self.phase {
            Phase::Draw { collect, .. } => collect.deadline().map(|d| d + self.t.o.grace),
            Phase::Lie { collect, .. } | Phase::Vote { collect, .. } => {
                collect.deadline().map(|d| d + self.t.o.grace)
            }
            Phase::Reveal { next, .. } => Some(*next),
            Phase::Scores { until } => Some(*until),
            Phase::Lobby | Phase::Final => None,
        };
        let now = party.now();
        let wait = due.map_or(1000, |d| d.saturating_sub(now).min(1000));
        Instant::now() + Duration::from_millis(wait)
    }

    /// Handles an event of the party (actions, joins); call
    /// [`Drawbluff::tick`] after a batch.
    pub fn event(&mut self, party: &mut Party, event: Event) {
        match event {
            Event::Joined(id) if !self.t.running => {
                let state = self.lobby_state(party);
                set_state(party, &id, state);
            }
            Event::Action(a) => self.act(party, a),
            _ => {}
        }
    }

    fn lobby_state(&self, party: &Party) -> Value {
        let task = if matches!(self.phase, Phase::Final) {
            "final"
        } else {
            "lobby"
        };
        let (score, rank) = self.t.rank(party, None);
        json!({ "task": task, "score": score, "rank": rank })
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

    /// An answer for the phase's collection (or not now).
    fn answer(&mut self, party: &mut Party, a: Action) {
        let t = &mut self.t;
        let back = match &mut self.phase {
            Phase::Draw { collect, .. } => {
                let back = collect.offer(party, a, |data, _| {
                    validate_drawing(data).map(|d| (d, Value::Null))
                });
                if back.is_none() {
                    answered(party, collect, |_| json!({ "done": true }));
                }
                back
            }
            Phase::Lie { turn, collect, .. } => {
                // Moderation needs the party, which `offer` holds: ask first.
                let typed = lie_text(a.data());
                let moderated = match typed.as_deref() {
                    Some(text) if !text.is_empty() => Some(party.moderate(text, "lie")),
                    _ => None,
                };
                let prompt = turn.prompt.clone();
                let back = collect.offer(party, a, |data, _| {
                    let text = t.lie(data, &prompt, moderated)?;
                    let reply = json!({ "text": text.as_str() });
                    Ok((text, reply))
                });
                if back.is_none() {
                    answered(
                        party,
                        collect,
                        |text| json!({ "done": true, "lie": text.as_str() }),
                    );
                }
                back
            }
            Phase::Vote {
                options, collect, ..
            } => {
                let back = collect.offer(party, a, |data, p| {
                    let id = match data {
                        Value::Object(o) => o.get("option").unwrap_or(&Value::Null),
                        other => other,
                    };
                    match id.as_str() {
                        Some(id) if options_for(p.as_str(), options).iter().any(|o| o.id == id) => {
                            Ok((id.to_owned(), Value::Null))
                        }
                        _ => Err(PartyError::new("bad-vote", "Pick one of the titles")),
                    }
                });
                if back.is_none() {
                    answered(
                        party,
                        collect,
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
    /// deadline, reveal steps, the end of the scores.
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
            Phase::Lie { collect, .. } => collect.done(party),
            Phase::Vote { collect, .. } => collect.done(party),
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
                let drawings: Vec<Turn> = collect
                    .take()
                    .into_iter()
                    .filter_map(|(artist, drawing)| {
                        let prompt = prompts.iter().find(|(p, _)| *p == artist)?.1.clone();
                        Some(Turn {
                            artist,
                            prompt,
                            drawing,
                        })
                    })
                    .collect();
                self.t.turns = shuffle(&drawings, &mut self.t.rng).into();
                self.next_turn(party);
            }
            Phase::Lie {
                turn,
                others,
                collect,
            } => {
                let lies: Vec<(String, String)> = collect
                    .take()
                    .into_iter()
                    .map(|(p, text)| (p.as_str().to_owned(), text))
                    .collect();
                let spares = self.t.spares(&turn.prompt);
                let spares = shuffle(&spares, &mut self.t.rng);
                let min = 4.min(others.len() + 1);
                let options = build_options(&turn.prompt, &lies, &spares, &mut self.t.rng, min);
                let voters: Vec<PlayerId> = others
                    .into_iter()
                    .filter(|p| party.player(p).is_some())
                    .collect();
                self.vote(party, turn, options, voters);
            }
            Phase::Vote {
                turn,
                options,
                collect,
            } => {
                let votes: Vec<(String, String)> = collect
                    .take()
                    .into_iter()
                    .map(|(p, o)| (p.as_str().to_owned(), o))
                    .collect();
                let round = i64::from(self.t.round);
                let scoring = score(turn.artist.as_str(), &options, &votes, round);
                for (id, n) in &scoring.points {
                    if let Some((_, s)) = self.t.scores.iter_mut().find(|(p, _)| p.as_str() == id) {
                        *s += n;
                    }
                }
                self.reveal(party, &turn, &options, &scoring);
            }
            Phase::Reveal { .. } => self.next_turn(party),
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

    /// Plays a game with the players who are there.
    fn play(&mut self, party: &mut Party) {
        let t = &mut self.t;
        t.running = true;
        party.lock();
        t.scores = ids(party.players()).into_iter().map(|id| (id, 0)).collect();
        t.deck = shuffle(&t.o.prompts, &mut t.rng);
        t.round = 1;
        self.draw(party);
    }

    fn draw(&mut self, party: &mut Party) {
        let t = &mut self.t;
        t.before = t.scores.clone();
        let players: Vec<PlayerId> = party
            .players()
            .iter()
            .filter(|p| p.connected() || t.has(p.id()))
            .map(|p| p.id().clone())
            .collect();
        let prompts: Vec<(PlayerId, String)> = players
            .iter()
            .map(|p| {
                let prompt = t.deck.pop().unwrap_or_else(|| "anything you like".into());
                (p.clone(), prompt)
            })
            .collect();
        let deadline = party.now() + t.o.timings.draw;
        let (round, rounds) = (t.round, t.o.rounds);
        party.room().replace(json!({
            "phase": "draw",
            "round": round,
            "rounds": rounds,
            "deadline": deadline,
        }));
        party.screen().replace(json!({
            "phase": "draw",
            "round": round,
            "rounds": rounds,
            "deadline": deadline,
            "waiting": strs(&players),
            "done": [],
        }));
        for (p, prompt) in &prompts {
            set_state(
                party,
                p,
                json!({ "task": "draw", "prompt": prompt.as_str(), "done": false }),
            );
        }
        for id in ids(party.players()) {
            if !players.contains(&id) {
                set_state(party, &id, json!({ "task": "watch" }));
            }
        }
        let collect = Collect::new("draw", players)
            .until(deadline)
            .grace(t.o.grace);
        self.set_phase(Phase::Draw { prompts, collect });
    }

    /// The next drawing of the round, or the scores.
    fn next_turn(&mut self, party: &mut Party) {
        while let Some(turn) = self.t.turns.pop_front() {
            if party.player(&turn.artist).is_some() {
                return self.lie(party, turn);
            }
        }
        self.scores(party);
    }

    fn lie(&mut self, party: &mut Party, turn: Turn) {
        let t = &self.t;
        let others: Vec<PlayerId> = party
            .players()
            .iter()
            .filter(|p| *p.id() != turn.artist && t.has(p.id()))
            .map(|p| p.id().clone())
            .collect();
        let art = artist(party, &turn.artist);
        let deadline = party.now() + t.o.timings.lie;
        let (round, rounds) = (t.round, t.o.rounds);
        let drawing = turn.drawing.to_value();
        party.room().replace(json!({
            "phase": "lie",
            "round": round,
            "rounds": rounds,
            "deadline": deadline,
            "drawing": drawing.clone(),
            "artist": art.clone(),
        }));
        party.screen().replace(json!({
            "phase": "lie",
            "round": round,
            "deadline": deadline,
            "drawing": drawing,
            "artist": art,
            "waiting": strs(&others),
            "done": [],
        }));
        set_state(party, &turn.artist, json!({ "task": "artist" }));
        for p in &others {
            set_state(party, p, json!({ "task": "lie", "done": false }));
        }
        let collect = Collect::new("lie", others.clone())
            .until(deadline)
            .grace(t.o.grace);
        self.set_phase(Phase::Lie {
            turn,
            others,
            collect,
        });
    }

    fn vote(&mut self, party: &mut Party, turn: Turn, options: Vec<Choice>, voters: Vec<PlayerId>) {
        let t = &self.t;
        let art = artist(party, &turn.artist);
        let deadline = party.now() + t.o.timings.vote;
        let (round, rounds) = (t.round, t.o.rounds);
        let drawing = turn.drawing.to_value();
        party.room().replace(json!({
            "phase": "vote",
            "round": round,
            "rounds": rounds,
            "deadline": deadline,
            "drawing": drawing.clone(),
            "artist": art.clone(),
        }));
        let shown = |list: Vec<&Choice>| -> Value {
            Value::Array(
                list.into_iter()
                    .map(|o| json!({ "id": o.id.as_str(), "text": o.text.as_str() }))
                    .collect(),
            )
        };
        party.screen().replace(json!({
            "phase": "vote",
            "round": round,
            "deadline": deadline,
            "drawing": drawing,
            "artist": art,
            "options": shown(options.iter().collect()),
            "waiting": strs(&voters),
            "done": [],
        }));
        for p in &voters {
            let mine = shown(options_for(p.as_str(), &options));
            set_state(
                party,
                p,
                json!({ "task": "vote", "options": mine, "done": false }),
            );
        }
        let collect = Collect::new("vote", voters)
            .until(deadline)
            .grace(t.o.grace);
        self.set_phase(Phase::Vote {
            turn,
            options,
            collect,
        });
    }

    fn reveal(
        &mut self,
        party: &mut Party,
        turn: &Turn,
        options: &[Choice],
        scoring: &crate::rules::Scoring,
    ) {
        let t = &self.t;
        let round = i64::from(t.round);
        let name = |id: &str| find(party, id).map_or("someone", Player::name).to_owned();
        let items: Vec<Value> = reveal_order(options, scoring)
            .into_iter()
            .map(|o| {
                let who = scoring.voters_of(&o.id);
                let n = who.len() as i64;
                json!({
                    "id": o.id.as_str(),
                    "text": o.text.as_str(),
                    "truth": o.truth,
                    "decoy": o.decoy,
                    "authors": o.authors.iter().map(|a| name(a)).collect::<Vec<_>>(),
                    "voters": who.iter().map(|a| name(a)).collect::<Vec<_>>(),
                    "points": if o.truth { POINTS_GUESS * round } else { POINTS_FOOLED * round * n },
                    "artistPoints": if o.truth { POINTS_ARTIST * round * n } else { 0 },
                })
            })
            .collect();
        let count = items.len();
        let art = artist(party, &turn.artist);
        party.room().replace(json!({
            "phase": "reveal",
            "round": t.round,
            "rounds": t.o.rounds,
            "drawing": turn.drawing.to_value(),
            "artist": art.clone(),
        }));
        party.screen().replace(json!({
            "phase": "reveal",
            "round": t.round,
            "drawing": turn.drawing.to_value(),
            "artist": art,
            "prompt": turn.prompt.as_str(),
            "items": items,
            "step": 0,
        }));
        for id in ids(party.players()) {
            let (score, rank) = t.rank(party, Some(&id));
            set_state(
                party,
                &id,
                json!({ "task": "watch", "score": score, "rank": rank }),
            );
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
        party.room().replace(json!({
            "phase": "scores",
            "round": t.round,
            "rounds": t.o.rounds,
        }));
        party.screen().replace(json!({
            "phase": "scores",
            "round": t.round,
            "rounds": t.o.rounds,
            "scores": lines(&table),
            "last": t.round == t.o.rounds,
        }));
        for id in ids(party.players()) {
            let (score, rank) = t.rank(party, Some(&id));
            set_state(
                party,
                &id,
                json!({ "task": "scores", "score": score, "rank": rank }),
            );
        }
        let until = party.now() + t.o.timings.scores;
        self.set_phase(Phase::Scores { until });
    }

    fn finish(&mut self, party: &mut Party) {
        let t = &self.t;
        let table = t.table(party, None);
        party.room().replace(json!({
            "phase": "final",
            "minPlayers": t.o.min_players,
        }));
        party.screen().replace(json!({
            "phase": "final",
            "scores": lines(&table),
        }));
        for id in ids(party.players()) {
            let (score, rank) = t.rank(party, Some(&id));
            set_state(
                party,
                &id,
                json!({ "task": "final", "score": score, "rank": rank }),
            );
        }
        self.set_phase(Phase::Final);
        self.t.events.push(GameEvent::End(table));
        self.t.running = false;
        party.unlock();
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

    /// Prompts nobody drew that are not this one: "lie for me" and decoys.
    fn spares(&self, prompt: &str) -> Vec<String> {
        let key = title_key(prompt);
        self.deck
            .iter()
            .filter(|t| title_key(t) != key)
            .cloned()
            .collect()
    }

    /// Checks a lie (`{text}`, a string, or `{auto: true}` for "lie for
    /// me": a prompt nobody drew).
    fn lie(
        &mut self,
        data: &Value,
        prompt: &str,
        moderated: Option<Moderation>,
    ) -> Result<String, PartyError> {
        if data.get("auto").and_then(Value::as_bool) == Some(true) {
            let pool = self.spares(prompt);
            if pool.is_empty() {
                return Err(PartyError::new("no-lies", "Out of ideas: write one"));
            }
            let pick = pool[self.rng.below(pool.len())].clone();
            if let Some(i) = self.deck.iter().position(|t| *t == pick) {
                self.deck.remove(i);
            }
            return Ok(pick);
        }
        let text = lie_text(data).unwrap_or_default();
        if text.is_empty() {
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
        // `localeCompare`, roughly: case and accents last.
        table.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                let key = |s: &str| fold(s).to_lowercase();
                key(&a.name).cmp(&key(&b.name)).then(a.name.cmp(&b.name))
            })
        });
        table
    }

    /// A player's score and rank (1 + how many have more); nobody's: 0.
    fn rank(&self, party: &Party, id: Option<&PlayerId>) -> (i64, usize) {
        let s = id.map_or(0, |id| self.score_of(id));
        let above = self
            .table(party, None)
            .iter()
            .filter(|l| l.score > s)
            .count();
        (s, 1 + above)
    }
}

/// The text of a typed lie: `{text}` or a string, cleaned.
fn lie_text(data: &Value) -> Option<String> {
    let text = match data {
        Value::Object(o) => o.get("text")?,
        other => other,
    };
    text.as_str().map(clean_title)
}

/// After an accepted answer: the player's state and the screen's `done`.
fn answered<T>(party: &mut Party, collect: &Collect<T>, state: impl FnOnce(&T) -> Value) {
    let Some((p, value)) = collect.answers().last() else {
        return;
    };
    let state = state(value);
    if let Some(s) = party.player_state(p) {
        s.set(state);
    }
    let done: Vec<&str> = collect.answers().iter().map(|(p, _)| p.as_str()).collect();
    party.screen().set(json!({ "done": done }));
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
