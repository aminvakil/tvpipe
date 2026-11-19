//! JSON values: tvpipe's ordered [`Value`] and the [`json!`](crate::json)
//! macro to build them.

pub use tvpipe::__web::json::{Number, Object, Value};

/// Builds a [`Value`] from JSON-like syntax: `null`, `true`, arrays,
/// objects with string-literal (or parenthesised expression) keys, and
/// any Rust expression that converts into a `Value`.
///
/// ```
/// use tvpipe_party::json;
/// let name = "Ann";
/// let v = json!({ "phase": "lobby", "players": [name, null], "round": 1 + 1 });
/// assert_eq!(v.to_string(), r#"{"phase":"lobby","players":["Ann",null],"round":2}"#);
/// ```
#[macro_export]
macro_rules! json {
    (null) => { $crate::Value::Null };
    ([ $($tt:tt)* ]) => { $crate::Value::Array($crate::__json_array!([] $($tt)*)) };
    ({ $($tt:tt)* }) => {{
        #[allow(unused_mut)]
        let mut object = $crate::Object::new();
        $crate::__json_object!(object ($($tt)*));
        $crate::Value::Object(object)
    }};
    ($other:expr) => { $crate::Value::from($other) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __json_array {
    ([$($done:expr,)*]) => { vec![$($done,)*] };
    ([$($done:expr,)*] null $(, $($rest:tt)*)?) => {
        $crate::__json_array!([$($done,)* $crate::Value::Null,] $($($rest)*)?)
    };
    ([$($done:expr,)*] [$($inner:tt)*] $(, $($rest:tt)*)?) => {
        $crate::__json_array!([$($done,)* $crate::json!([$($inner)*]),] $($($rest)*)?)
    };
    ([$($done:expr,)*] {$($inner:tt)*} $(, $($rest:tt)*)?) => {
        $crate::__json_array!([$($done,)* $crate::json!({$($inner)*}),] $($($rest)*)?)
    };
    ([$($done:expr,)*] $next:expr $(, $($rest:tt)*)?) => {
        $crate::__json_array!([$($done,)* $crate::json!($next),] $($($rest)*)?)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __json_object {
    ($o:ident ()) => {};
    ($o:ident ($key:literal : null $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::Value::Null);
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident ($key:literal : [$($inner:tt)*] $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::json!([$($inner)*]));
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident ($key:literal : {$($inner:tt)*} $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::json!({$($inner)*}));
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident ($key:literal : $value:expr $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::json!($value));
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident (($key:expr) : null $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::Value::Null);
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident (($key:expr) : [$($inner:tt)*] $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::json!([$($inner)*]));
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident (($key:expr) : {$($inner:tt)*} $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::json!({$($inner)*}));
        $crate::__json_object!($o ($($($rest)*)?));
    };
    ($o:ident (($key:expr) : $value:expr $(, $($rest:tt)*)?)) => {
        $o.insert($key, $crate::json!($value));
        $crate::__json_object!($o ($($($rest)*)?));
    };
}

/// A value from a phone made like `JSON.parse` makes it: a key that
/// repeats keeps its first position and its last value.
pub(crate) fn dedupe(v: Value) -> Value {
    match v {
        Value::Array(items) => Value::Array(items.into_iter().map(dedupe).collect()),
        Value::Object(o) => {
            let mut out = Object::new();
            for (k, v) in o.iter() {
                // `insert` replaces in place: first position, last value.
                out.insert(k, dedupe(v.clone()));
            }
            Value::Object(out)
        }
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_macro() {
        let x = 3;
        let key = String::from("dyn");
        let v = json!({
            "a": null,
            "b": [1, "two", null, [true], {"c": x * 2}],
            "nested": {"empty": {}, "list": []},
            (key): -1.5,
            "last": "s"
        });
        assert_eq!(
            v.to_string(),
            r#"{"a":null,"b":[1,"two",null,[true],{"c":6}],"nested":{"empty":{},"list":[]},"dyn":-1.5,"last":"s"}"#
        );
        assert_eq!(json!(null), Value::Null);
        assert_eq!(json!([]), Value::Array(vec![]));
        assert_eq!(json!({}).to_string(), "{}");
        assert_eq!(json!("x"), Value::from("x"));
        assert_eq!(json!([1, 2,]).to_string(), "[1,2]");
    }

    #[test]
    fn dedupe_like_json_parse() {
        let v = Value::parse(r#"{"a":1,"b":{"x":1,"x":2},"a":3}"#).unwrap();
        assert_eq!(dedupe(v).to_string(), r#"{"a":3,"b":{"x":2}}"#);
    }
}
