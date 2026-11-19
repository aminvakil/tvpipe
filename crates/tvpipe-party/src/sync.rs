//! Views and their diffs (docs/party-design.md §5 "State sync").
//!
//! A diff from view `a` to view `b` is `{ set, del }`: `set` is merged
//! into the old view (objects merge key by key; anything else, `null` and
//! arrays included, replaces), then the key paths in `del` are removed.
//! The same rules as `@tvpipe/party`'s `sync.ts` and the browser client's
//! `applyDiff`; `tests/vectors/diff.txt` checks them.

use crate::value::{Object, Value};

/// What changed from one view to the next.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Diff {
    pub set: Object,
    pub del: Vec<Vec<String>>,
}

#[cfg(test)]
impl Diff {
    /// The fields of a `state` message (`set`, `del`), into `msg`.
    pub fn write_into(self, msg: &mut Object) {
        if !self.set.is_empty() {
            msg.insert("set", Value::Object(self.set));
        }
        if !self.del.is_empty() {
            msg.insert(
                "del",
                Value::Array(
                    self.del
                        .into_iter()
                        .map(|p| Value::Array(p.into_iter().map(Value::from).collect()))
                        .collect(),
                ),
            );
        }
    }
}

/// The diff from `a` to `b`, `None` when they are equal.
#[cfg(test)]
pub(crate) fn diff(a: &Object, b: &Object) -> Option<Diff> {
    let mut d = Diff::default();
    let mut path = Vec::new();
    walk(a, b, &mut path, &mut d.set, &mut d.del);
    (!d.set.is_empty() || !d.del.is_empty()).then_some(d)
}

/// The diff from `a` to `b` as it appears inside a bigger view under
/// `key` (`del` paths start with `key`; `set` is relative to it). A
/// view's diff is its sections' diffs in key order (`Party::flush`
/// composes them).
pub(crate) fn diff_at(a: &Object, b: &Object, key: &str) -> Diff {
    let mut d = Diff::default();
    let mut path = vec![key.to_owned()];
    walk(a, b, &mut path, &mut d.set, &mut d.del);
    d
}

fn walk(
    a: &Object,
    b: &Object,
    path: &mut Vec<String>,
    set: &mut Object,
    del: &mut Vec<Vec<String>>,
) {
    for k in a.keys() {
        if !b.contains_key(k) {
            let mut p = path.clone();
            p.push(k.to_owned());
            del.push(p);
        }
    }
    for (k, nv) in b.iter() {
        let Some(ov) = a.get(k) else {
            set.insert(k, nv.clone());
            continue;
        };
        match (ov, nv) {
            (Value::Object(oo), Value::Object(no)) => {
                let mut sub = Object::new();
                path.push(k.to_owned());
                walk(oo, no, path, &mut sub, del);
                path.pop();
                if !sub.is_empty() {
                    set.insert(k, Value::Object(sub));
                }
            }
            _ if !json_equal(ov, nv) => set.insert(k, nv.clone()),
            _ => {}
        }
    }
}

/// Deep equality as JSON (objects without regard to key order).
pub(crate) fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_equal(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| json_equal(v, w))
        }
        _ => a == b,
    }
}

/// Applies a diff to `view` (the client's rules).
#[cfg(test)]
pub(crate) fn apply(view: &mut Object, d: &Diff) {
    merge(view, &d.set);
    for path in &d.del {
        let Some((last, parents)) = path.split_last() else {
            continue;
        };
        let mut o = Some(&mut *view);
        for k in parents {
            o = o.and_then(|o| o.get_mut(k)).and_then(Value::as_object_mut);
        }
        if let Some(o) = o {
            o.remove(last);
        }
    }
}

#[cfg(test)]
fn merge(target: &mut Object, patch: &Object) {
    for (k, pv) in patch.iter() {
        match (target.get_mut(k), pv) {
            (Some(Value::Object(tv)), Value::Object(po)) => merge(tv, po),
            _ => target.insert(k, pv.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::vectors;

    fn obj(s: &str) -> Object {
        match Value::parse(s).unwrap() {
            Value::Object(o) => o,
            v => panic!("not an object: {v}"),
        }
    }

    fn diff_value(d: Diff) -> Value {
        let mut o = Object::new();
        d.write_into(&mut o);
        Value::Object(o)
    }

    #[test]
    fn vectors_from_node() {
        let blocks = vectors("diff.txt");
        assert!(blocks.len() > 40);
        for b in blocks {
            let name = b.str("name");
            let (a, want) = (obj(b.str("a")), obj(b.str("b")));
            let got = diff(&a, &want);
            match b.str("diff") {
                "none" => assert!(got.is_none(), "{name}: {got:?}"),
                expected => {
                    let got = got.unwrap_or_else(|| panic!("{name}: no diff"));
                    let expected = Value::parse(expected).unwrap();
                    assert!(
                        json_equal(&diff_value(got.clone()), &expected),
                        "{name}: {} != {expected}",
                        diff_value(got)
                    );
                    let mut applied = a.clone();
                    apply(&mut applied, &got);
                    assert!(
                        json_equal(&Value::Object(applied), &Value::Object(want)),
                        "{name}: apply"
                    );
                }
            }
        }
    }
}
