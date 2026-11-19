//! Replays `test/vectors/rules.txt` (written by `node test/vectors.ts
//! --bless` from `game/rules.ts`, the reference) against `src/rules.rs`.

use drawbluff::rules::{
    build_options, clean_title, deal, options_for, pick_liars, plan, reveal_order, rotate, score,
    share, shuffle, title_key, validate_drawing, Rng, PHONE_LIES,
};
use tvpipe_party::Value;

struct Block {
    line: usize,
    entries: Vec<(String, String)>,
}

impl Block {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn str(&self, key: &str) -> &str {
        self.get(key)
            .unwrap_or_else(|| panic!("rules.txt:{}: no {key}", self.line))
    }

    fn json(&self, key: &str) -> Value {
        Value::parse(self.str(key)).unwrap_or_else(|e| panic!("rules.txt:{}: {e}", self.line))
    }

    fn num(&self, key: &str) -> u64 {
        self.str(key).parse().unwrap()
    }
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut open = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            open = false;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once(" = ").expect("key = value");
        if !open {
            out.push(Block {
                line: i + 1,
                entries: Vec::new(),
            });
            open = true;
        }
        let b = out.last_mut().unwrap();
        b.entries.push((k.trim().to_owned(), v.to_owned()));
    }
    out
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let p = strings(p);
            (p[0].clone(), p[1].clone())
        })
        .collect()
}

/// Objects compared without key order.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        _ => a == b,
    }
}

/// A `drawing` block's input, with `repeat` and `pad` applied.
fn drawing_input(b: &Block) -> Value {
    let mut input = b.json("input");
    let Some(Value::Array(strokes)) = input.as_object_mut().and_then(|o| o.get_mut("strokes"))
    else {
        return input;
    };
    if let Some(n) = b.get("repeat") {
        let first = strokes[0].clone();
        *strokes = vec![first; n.parse().unwrap()];
    }
    if let Some(n) = b.get("pad") {
        if let Value::Array(s) = &mut strokes[0] {
            s.extend(std::iter::repeat_n(Value::from(1), n.parse().unwrap()));
        }
    }
    input
}

#[test]
fn rules_vectors() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test/vectors/rules.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    for b in blocks(&text) {
        let kind = b.str("test");
        *seen.entry(kind.to_owned()).or_default() += 1;
        let at = format!("rules.txt:{}", b.line);
        match kind {
            "rng" => {
                let mut r = Rng::new(b.num("seed") as u32);
                for v in b.json("values").as_array().unwrap() {
                    assert_eq!(u64::from(r.next_u32()), v.as_u64().unwrap(), "{at}");
                }
            }
            "shuffle" => {
                let items: Vec<u64> = (0..b.num("n")).collect();
                let got = shuffle(&items, &mut Rng::new(b.num("seed") as u32));
                let want: Vec<u64> = b
                    .json("order")
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap())
                    .collect();
                assert_eq!(got, want, "{at}");
            }
            "title" => {
                let input = b.json("input");
                let input = input.as_str().unwrap();
                assert_eq!(
                    clean_title(input),
                    b.json("clean").as_str().unwrap(),
                    "{at}"
                );
                assert_eq!(title_key(input), b.json("key").as_str().unwrap(), "{at}");
            }
            "drawing" => {
                let input = drawing_input(&b);
                match (validate_drawing(&input), b.get("error")) {
                    (Err(e), Some(code)) => assert_eq!(e.code, code, "{at}"),
                    (Ok(d), None) => {
                        let want = match b.str("output") {
                            "input" => input.clone(),
                            _ => b.json("output"),
                        };
                        assert!(same(&d.to_value(), &want), "{at}: {}", d.to_value());
                    }
                    (got, want) => panic!("{at}: got {got:?}, want error {want:?}"),
                }
            }
            "plan" => {
                for row in b.json("table").as_array().unwrap() {
                    let n = row[0].as_u64().unwrap() as usize;
                    let p = plan(n);
                    assert_eq!(
                        (p.spotlights as u64, p.liars as u64, p.pipelined),
                        (
                            row[1].as_u64().unwrap(),
                            row[2].as_u64().unwrap(),
                            row[3].as_bool().unwrap()
                        ),
                        "{at}: {n}"
                    );
                }
            }
            "share" => {
                for c in b.json("cases").as_array().unwrap() {
                    let n: Vec<i64> = c
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_i64().unwrap())
                        .collect();
                    assert_eq!(share(n[0], n[1], n[2]), n[3], "{at}: {c}");
                }
            }
            "liars" => {
                let lied = b.json("lied");
                let first = b.json("first");
                let got = pick_liars(
                    &strings(&b.json("pool")),
                    |id| lied.get(id).and_then(Value::as_i64).unwrap_or(0),
                    b.num("n") as usize,
                    &mut Rng::new(b.num("seed") as u32),
                    first.as_str(),
                );
                assert_eq!(got, strings(&b.json("liars")), "{at}");
            }
            "rotate" => {
                let keys = b.json("keys");
                let got = rotate(
                    &strings(&b.json("pool")),
                    |id| keys.get(id).and_then(Value::as_i64).unwrap(),
                    b.num("n") as usize,
                    &mut Rng::new(b.num("seed") as u32),
                );
                assert_eq!(got, strings(&b.json("picked")), "{at}");
            }
            "turn" => {
                let truth = b.json("truth");
                let options = build_options(
                    truth.as_str().unwrap(),
                    &pairs(&b.json("lies")),
                    &strings(&b.json("spares")),
                    &mut Rng::new(b.num("seed") as u32),
                    b.num("min") as usize,
                );
                let got = Value::Array(options.iter().map(|o| o.to_value()).collect());
                assert!(same(&got, &b.json("options")), "{at}: {got}");
                let own = b.json("own");
                for (voter, ids) in own.as_object().unwrap().iter() {
                    let got: Vec<&str> = options_for(voter, &options)
                        .iter()
                        .map(|o| o.id.as_str())
                        .collect();
                    assert_eq!(got, strings(ids), "{at}: {voter}");
                }
                let shown = deal(
                    &options,
                    &strings(&b.json("voting")),
                    PHONE_LIES,
                    &mut Rng::new(b.num("dealSeed") as u32),
                );
                let got = Value::Object(
                    shown
                        .iter()
                        .map(|(v, ids)| (v.clone(), Value::from(ids.clone())))
                        .collect(),
                );
                assert!(same(&got, &b.json("shown")), "{at}: {got}");
                let artist = b.json("artist");
                let scoring = score(
                    artist.as_str().unwrap(),
                    &options,
                    &shown,
                    &pairs(&b.json("votes")),
                    b.num("round") as i64,
                );
                let points = Value::Object(
                    scoring
                        .points
                        .iter()
                        .map(|(p, n)| (p.clone(), Value::from(*n)))
                        .collect(),
                );
                assert!(same(&points, &b.json("points")), "{at}: {points}");
                let voters = Value::Object(
                    scoring
                        .voters
                        .iter()
                        .map(|(o, v)| (o.clone(), Value::from(v.clone())))
                        .collect(),
                );
                assert!(same(&voters, &b.json("voters")), "{at}: {voters}");
                let seen = Value::Object(
                    scoring
                        .seen
                        .iter()
                        .map(|(o, n)| (o.clone(), Value::from(*n)))
                        .collect(),
                );
                assert!(same(&seen, &b.json("seen")), "{at}: {seen}");
                let reveal: Vec<String> = reveal_order(&options, &scoring)
                    .iter()
                    .map(|o| o.id.clone())
                    .collect();
                assert_eq!(reveal, strings(&b.json("reveal")), "{at}");
            }
            other => panic!("{at}: unknown test {other}"),
        }
    }
    for kind in [
        "rng", "shuffle", "title", "drawing", "plan", "share", "liars", "rotate", "turn",
    ] {
        assert!(
            seen.get(kind).copied().unwrap_or(0) > 0,
            "no {kind} vectors"
        );
    }
}

#[test]
fn prompts_are_distinct() {
    let prompts = drawbluff::prompts();
    assert!(prompts.len() > 100);
    let mut keys: Vec<String> = prompts.iter().map(|p| title_key(p)).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), prompts.len());
}
