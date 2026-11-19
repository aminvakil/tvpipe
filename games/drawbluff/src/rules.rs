//! drawbluff's rules as pure functions: drawings, titles, options, votes
//! and points (a port of `game/rules.ts`, which is the reference:
//! `test/vectors/rules.txt` holds its answers). No I/O, no clocks;
//! [`crate::game`] drives them.

use tvpipe_party::{clean_text, fold, json, PartyError, Value};

/// The 8 colours a drawing may use (index → `0xRRGGBB`).
pub const PALETTE: [u32; 8] = [
    0x1d1d1f, 0xffffff, 0xe5383b, 0xf9a620, 0xffd60a, 0x2a9d8f, 0x3a86ff, 0x8338ec,
];

/// The drawing grid: coordinates are 0..=1000.
pub const GRID: i32 = 1000;
/// The most strokes in a drawing.
pub const MAX_STROKES: usize = 300;
/// The most points in a drawing (all strokes).
pub const MAX_POINTS: usize = 12_000;
/// The longest title, in user-perceived characters.
pub const MAX_TITLE: usize = 40;

/// A drawing on the 1000×1000 grid: strokes of `[colour, width, x0, y0,
/// x1, y1, …]` (a stroke with one point is a dot).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Drawing {
    /// The strokes, validated: a palette index, a width of 2–120, and
    /// coordinates on the grid.
    pub strokes: Vec<Vec<i32>>,
}

impl Drawing {
    /// As JSON (`{"strokes": [[…], …]}`), for views.
    pub fn to_value(&self) -> Value {
        let strokes: Vec<Value> = self
            .strokes
            .iter()
            .map(|s| Value::Array(s.iter().map(|&v| Value::from(v)).collect()))
            .collect();
        json!({ "strokes": strokes })
    }
}

/// Checks and normalises a drawing from a phone: rounded, clamped to the
/// grid and widths, refused (`bad-drawing`) when it is not one.
pub fn validate_drawing(data: &Value) -> Result<Drawing, PartyError> {
    let bad = |why: &str| {
        PartyError::new(
            "bad-drawing",
            format!("That drawing cannot be sent ({why})"),
        )
    };
    let strokes = data
        .as_object()
        .and_then(|o| o.get("strokes"))
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no strokes"))?;
    if strokes.len() > MAX_STROKES {
        return Err(bad("too many strokes"));
    }
    let mut points = 0;
    let mut out = Vec::with_capacity(strokes.len());
    for s in strokes {
        let s = s
            .as_array()
            .filter(|s| s.len() >= 4 && s.len() % 2 == 0)
            .ok_or_else(|| bad("a broken stroke"))?;
        let mut nums = Vec::with_capacity(s.len());
        for v in s {
            match v.as_f64() {
                Some(f) if f.is_finite() => nums.push(f),
                _ => return Err(bad("not numbers")),
            }
        }
        let color = nums[0];
        if color.fract() != 0.0 || !(0.0..PALETTE.len() as f64).contains(&color) {
            return Err(bad("unknown colour"));
        }
        points += (nums.len() - 2) / 2;
        if points > MAX_POINTS {
            return Err(bad("too many points"));
        }
        let mut stroke = Vec::with_capacity(nums.len());
        stroke.push(color as i32);
        stroke.push(js_round(nums[1]).clamp(2.0, 120.0) as i32);
        stroke.extend(
            nums[2..]
                .iter()
                .map(|&v| js_round(v).clamp(0.0, GRID as f64) as i32),
        );
        out.push(stroke);
    }
    Ok(Drawing { strokes: out })
}

/// JavaScript's `Math.round`: halves go up (`-2.5` → `-2`).
fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// A title as typed, cleaned up for display (`""` when nothing is left).
pub fn clean_title(text: &str) -> String {
    clean_text(text, MAX_TITLE)
}

/// How titles compare: case, accents, punctuation, spacing and a leading
/// article do not matter ("A nervous cactus!" = "nervous cactus").
pub fn title_key(text: &str) -> String {
    let lower = fold(text).to_lowercase();
    // `[^\p{L}\p{N}]+` → one space.
    let mut spaced = String::with_capacity(lower.len());
    for c in lower.chars() {
        if is_letter_or_number(c) {
            spaced.push(c);
        } else if !spaced.ends_with(' ') {
            spaced.push(' ');
        }
    }
    let trimmed = spaced.trim_matches(' ');
    let rest = ["a ", "an ", "the "]
        .iter()
        .find_map(|a| trimmed.strip_prefix(a))
        .unwrap_or(trimmed);
    rest.chars().filter(|&c| c != ' ').collect()
}

/// `\p{L}` or `\p{N}`: Rust's alphanumeric is `Alphabetic` or `N`, which
/// also has marks (gone after [`fold`]) and these symbols (`So`).
fn is_letter_or_number(c: char) -> bool {
    c.is_alphanumeric()
        && !matches!(
            c,
            '\u{24b6}'..='\u{24e9}'
                | '\u{1f130}'..='\u{1f149}'
                | '\u{1f150}'..='\u{1f169}'
                | '\u{1f170}'..='\u{1f189}'
        )
}

/// One choice on the vote screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// `o1`, `o2`, …
    pub id: String,
    /// The title.
    pub text: String,
    /// The real title.
    pub truth: bool,
    /// Who wrote this lie (player ids; several when their lies were the same).
    pub authors: Vec<String>,
    /// A lie the game added (too few players' lies).
    pub decoy: bool,
}

impl Choice {
    /// As JSON, as `game/rules.ts` has it (`{id, text, truth, authors, decoy}`).
    pub fn to_value(&self) -> Value {
        json!({
            "id": self.id.as_str(),
            "text": self.text.as_str(),
            "truth": self.truth,
            "authors": self.authors.clone(),
            "decoy": self.decoy,
        })
    }
}

/// The options for one drawing: the truth, the players' lies (`(author,
/// text)` in arrival order; equal lies merged, ones equal to the truth
/// dropped), decoys from `spares` until there are at least `min`, in a
/// shuffled order.
pub fn build_options(
    truth: &str,
    lies: &[(String, String)],
    spares: &[String],
    rng: &mut Rng,
    min: usize,
) -> Vec<Choice> {
    let truth_key = title_key(truth);
    let mut n = 0;
    let mut next_id = || {
        n += 1;
        format!("o{n}")
    };
    // Keys of the lies (and decoys), with their index in `lied`.
    let mut keys: Vec<(String, Option<usize>)> = Vec::new();
    let mut lied: Vec<Choice> = Vec::new();
    for (author, text) in lies {
        let k = title_key(text);
        if k.is_empty() || k == truth_key {
            continue; // refused when typed; never shown
        }
        match keys.iter().find(|(key, _)| *key == k) {
            Some(&(_, Some(i))) => lied[i].authors.push(author.clone()),
            _ => {
                keys.push((k, Some(lied.len())));
                lied.push(Choice {
                    id: next_id(),
                    text: text.clone(),
                    truth: false,
                    authors: vec![author.clone()],
                    decoy: false,
                });
            }
        }
    }
    let mut options = vec![Choice {
        id: next_id(),
        text: truth.to_owned(),
        truth: true,
        authors: Vec::new(),
        decoy: false,
    }];
    options.extend(lied);
    for s in spares {
        if options.len() >= min {
            break;
        }
        let k = title_key(s);
        if k == truth_key || keys.iter().any(|(key, _)| *key == k) {
            continue;
        }
        keys.push((k, None));
        options.push(Choice {
            id: next_id(),
            text: s.clone(),
            truth: false,
            authors: Vec::new(),
            decoy: true,
        });
    }
    shuffle(&options, rng)
}

/// A Fisher–Yates shuffle (as `game/rules.ts` does it, so a seed gives
/// the same order in both).
pub fn shuffle<T: Clone>(items: &[T], rng: &mut Rng) -> Vec<T> {
    let mut a = items.to_vec();
    for i in (1..a.len()).rev() {
        let j = (rng.next_f64() * (i + 1) as f64).floor() as usize;
        a.swap(i, j.min(i));
    }
    a
}

/// Points for a voter who found the truth.
pub const POINTS_GUESS: i64 = 1000;
/// Points for the artist, per voter who found the truth.
pub const POINTS_ARTIST: i64 = 1000;
/// Points for a liar, per voter who fell for the lie.
pub const POINTS_FOOLED: i64 = 500;

/// The points of one drawing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scoring {
    /// Points per player id, in the order they were first given.
    pub points: Vec<(String, i64)>,
    /// Per option id (in the options' order): who picked it.
    pub voters: Vec<(String, Vec<String>)>,
}

impl Scoring {
    /// A player's points.
    pub fn points_of(&self, id: &str) -> i64 {
        self.points
            .iter()
            .find(|(p, _)| p == id)
            .map_or(0, |(_, n)| *n)
    }

    /// Who picked an option.
    pub fn voters_of(&self, option: &str) -> &[String] {
        self.voters
            .iter()
            .find(|(o, _)| o == option)
            .map_or(&[], |(_, v)| v.as_slice())
    }
}

/// Scores one drawing: `votes` are `(voter, option id)`; the artist's
/// votes, votes for one's own lie and unknown options count for nothing.
/// Round N counts N times.
pub fn score(artist: &str, options: &[Choice], votes: &[(String, String)], round: i64) -> Scoring {
    let mut s = Scoring {
        points: Vec::new(),
        voters: options.iter().map(|o| (o.id.clone(), Vec::new())).collect(),
    };
    let add = |points: &mut Vec<(String, i64)>, id: &str, n: i64| match points
        .iter_mut()
        .find(|(p, _)| p == id)
    {
        Some((_, total)) => *total += n * round,
        None => points.push((id.to_owned(), n * round)),
    };
    for (voter, option) in votes {
        let Some((i, o)) = options.iter().enumerate().find(|(_, o)| o.id == *option) else {
            continue;
        };
        if voter == artist || o.authors.contains(voter) {
            continue;
        }
        s.voters[i].1.push(voter.clone());
        if o.truth {
            add(&mut s.points, voter, POINTS_GUESS);
            add(&mut s.points, artist, POINTS_ARTIST);
        } else {
            for a in &o.authors {
                add(&mut s.points, a, POINTS_FOOLED);
            }
        }
    }
    s
}

/// The options a voter sees: all but the ones they wrote.
pub fn options_for<'a>(voter: &str, options: &'a [Choice]) -> Vec<&'a Choice> {
    options
        .iter()
        .filter(|o| !o.authors.iter().any(|a| a == voter))
        .collect()
}

/// The reveal order: lies nobody picked are skipped, the others from the
/// fewest to the most votes, the truth last.
pub fn reveal_order<'a>(options: &'a [Choice], scoring: &Scoring) -> Vec<&'a Choice> {
    let mut lies: Vec<&Choice> = options
        .iter()
        .filter(|o| !o.truth && !scoring.voters_of(&o.id).is_empty())
        .collect();
    lies.sort_by_key(|o| scoring.voters_of(&o.id).len());
    lies.extend(options.iter().filter(|o| o.truth));
    lies
}

/// A seeded generator (mulberry32), the same as `seeded()` in
/// `game/rules.ts`.
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    /// A generator from a seed.
    pub fn new(seed: u32) -> Rng {
        Rng(seed)
    }

    /// One seeded from the clock and the process (not for tests).
    pub fn from_entropy() -> Rng {
        use std::hash::{BuildHasher, RandomState};
        Rng(RandomState::new().hash_one(std::time::SystemTime::now()) as u32)
    }

    /// The next 32 random bits.
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }

    /// A number in `[0, 1)` (JavaScript's `rng()`).
    pub fn next_f64(&mut self) -> f64 {
        f64::from(self.next_u32()) / 4_294_967_296.0
    }

    /// An index below `n` (`Math.floor(rng() * n)`); 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_f64() * n as f64).floor() as usize).min(n.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_like_javascript() {
        for (x, r) in [
            (0.5, 1.0),
            (-0.5, -0.0),
            (-2.5, -2.0),
            (2.5, 3.0),
            (0.499_999_999_999_999_94, 0.0),
            (-1.2, -1.0),
        ] {
            assert_eq!(js_round(x), r, "{x}");
        }
    }

    #[test]
    fn keys() {
        assert_eq!(title_key("A Nervous  Cactus!"), title_key("nervous cactus"));
        assert_eq!(title_key("The café"), "cafe");
        assert_ne!(title_key("a cactus"), title_key("a cactuses"));
    }
}
