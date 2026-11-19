//! Waiting for one action per player, or a deadline: the shape of most
//! party-game phases (`party.collect` in `@tvpipe/party`).

use crate::party::{Action, PartyError, PlayerId};
use crate::value::Value;
use crate::Party;

/// Collects one action of a type from each of some players, until they
/// all answered or a deadline (plus grace) passed.
///
/// ```no_run
/// # use tvpipe_party::{Collect, Event, Party, PartyError, Value};
/// # fn f(party: &mut Party) {
/// let everyone = party.players().iter().map(|p| p.id().clone());
/// let mut answers = Collect::<String>::new("answer", everyone).until(party.now() + 60_000);
/// while !answers.done(party) {
///     for event in party.wait(std::time::Duration::from_millis(100)) {
///         if let Event::Action(a) = event {
///             if let Some(a) = answers.offer(party, a, |data, _| {
///                 let text = data.as_str().ok_or(PartyError::new("bad-data", "Text, please"))?;
///                 Ok((text.to_owned(), Value::Null))
///             }) {
///                 party.reply(a, Err(PartyError::new("not-now", "Not now")));
///             }
///         }
///     }
///     party.flush();
/// }
/// let answers = answers.take(); // Vec<(PlayerId, String)>
/// # }
/// ```
#[derive(Debug)]
pub struct Collect<T> {
    kind: String,
    from: Vec<PlayerId>,
    answers: Vec<(PlayerId, T)>,
    until: Option<u64>,
    grace: u64,
    replace: bool,
    skip_away: bool,
    ended: bool,
}

impl<T> Collect<T> {
    /// Collects actions of type `kind` from these players.
    pub fn new(kind: &str, from: impl IntoIterator<Item = PlayerId>) -> Collect<T> {
        Collect {
            kind: kind.to_owned(),
            from: from.into_iter().collect(),
            answers: Vec::new(),
            until: None,
            grace: 500,
            replace: false,
            skip_away: false,
            ended: false,
        }
    }

    /// A deadline in [`Party::now`] time; answers are accepted until it
    /// plus the grace (default 500 ms: the tunnel's p95 plus the TV being
    /// behind).
    pub fn until(mut self, deadline_ms: u64) -> Self {
        self.until = Some(deadline_ms);
        self
    }

    /// Answers accepted after the deadline, in ms.
    pub fn grace(mut self, ms: u64) -> Self {
        self.grace = ms;
        self
    }

    /// Whether a player may answer again, replacing the first answer.
    pub fn replace(mut self, yes: bool) -> Self {
        self.replace = yes;
        self
    }

    /// With a deadline: stop waiting for players who are away (by
    /// default they may come back and answer until the deadline).
    pub fn skip_away(mut self, yes: bool) -> Self {
        self.skip_away = yes;
        self
    }

    /// The deadline, if any.
    pub fn deadline(&self) -> Option<u64> {
        self.until
    }

    /// Offers an action. Not this collection's type, or too late: the
    /// action comes back. Otherwise it is answered here: `not-asked`
    /// (not one of the players), `already-answered`, the validator's
    /// error, or the validator's reply value (the answer is kept).
    pub fn offer(
        &mut self,
        party: &mut Party,
        action: Action,
        validate: impl FnOnce(&Value, &PlayerId) -> Result<(T, Value), PartyError>,
    ) -> Option<Action> {
        if action.kind() != self.kind || self.past(party.now()) || self.ended {
            return Some(action);
        }
        let Some(player) = action.player().cloned() else {
            party.reply(action, Err(not_asked()));
            return None;
        };
        if !self.from.contains(&player) {
            party.reply(action, Err(not_asked()));
            return None;
        }
        let existing = self.answers.iter().position(|(p, _)| *p == player);
        if existing.is_some() && !self.replace {
            party.reply(
                action,
                Err(PartyError::new("already-answered", "Already sent")),
            );
            return None;
        }
        match validate(action.data(), &player) {
            Ok((value, reply)) => {
                match existing {
                    Some(i) => self.answers[i].1 = value,
                    None => self.answers.push((player, value)),
                }
                party.reply(action, Ok(reply));
            }
            Err(e) => party.reply(action, Err(e)),
        }
        None
    }

    fn past(&self, now: u64) -> bool {
        self.until
            .is_some_and(|u| now >= u.saturating_add(self.grace))
    }

    /// Ends the collection early (the answers so far stay).
    pub fn end(&mut self) {
        self.ended = true;
    }

    /// Whether it is over: everyone (still in the party) answered, the
    /// deadline plus grace passed, or it was ended. Without a deadline
    /// (or with `skip_away`), only connected players are waited for, and
    /// at least one must be there.
    pub fn done(&self, party: &Party) -> bool {
        if self.ended || self.past(party.now()) {
            return true;
        }
        let present: Vec<&PlayerId> = self
            .from
            .iter()
            .filter(|id| party.player(id).is_some())
            .collect();
        if present.is_empty() {
            return true;
        }
        let answered = |id: &PlayerId| self.answers.iter().any(|(p, _)| p == id);
        if self.until.is_some() && !self.skip_away {
            return present.iter().all(|id| answered(id));
        }
        let connected = |id: &PlayerId| party.player(id).is_some_and(|p| p.connected());
        if !present.iter().any(|id| connected(id)) {
            return false;
        }
        present.iter().all(|id| answered(id) || !connected(id))
    }

    /// The answers so far, in the order they came.
    pub fn answers(&self) -> &[(PlayerId, T)] {
        &self.answers
    }

    /// The answer of a player.
    pub fn answer(&self, id: &PlayerId) -> Option<&T> {
        self.answers.iter().find(|(p, _)| p == id).map(|(_, v)| v)
    }

    /// Takes the answers.
    pub fn take(self) -> Vec<(PlayerId, T)> {
        self.answers
    }
}

fn not_asked() -> PartyError {
    PartyError::new("not-asked", "Not your turn")
}
