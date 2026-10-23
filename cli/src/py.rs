//! Values print the way the first ddp (Python) printed them, so output, docs and tests stay the same:
//! `383` vs `381.0`, `None`, `['bitmap']`, and JSON with `", "` and `": "` between items.
use serde_json::{Map, Value};
use std::cmp::Ordering;

/// Python's repr of a float: shortest round-trip digits, `.0` on whole numbers, exponent outside 1e-4..1e16.
pub fn float(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    if (-4..16).contains(&exponent) {
        let plain = format!("{value}");
        if plain.contains('.') {
            plain
        } else {
            format!("{plain}.0")
        }
    } else {
        format!("{mantissa}e{}{:02}", if exponent < 0 { '-' } else { '+' }, exponent.abs())
    }
}

pub fn number(number: &serde_json::Number) -> String {
    match number.as_i64() {
        Some(whole) => whole.to_string(),
        None => match number.as_u64() {
            Some(whole) => whole.to_string(),
            None => float(number.as_f64().unwrap_or(0.0)),
        },
    }
}

/// Python's str(): what an f-string prints for a value.
pub fn str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => repr(other),
    }
}

/// Python's repr(): strings quoted, as inside a list or dict.
pub fn repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => number(n),
        Value::String(text) => quote(text),
        Value::Array(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter().map(|(k, v)| format!("{}: {}", quote(k), repr(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn quote(text: &str) -> String {
    let delimiter = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::from(delimiter);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == delimiter => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(delimiter);
    out
}

/// json.dumps(value): `", "` and `": "` separators, non-ASCII escaped.
pub fn dumps(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value, None, 0, false);
    out
}

/// json.dumps(value, indent=2, sort_keys=True)
pub fn dumps_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value, Some(2), 0, true);
    out
}

/// json.dumps(value, sort_keys=True): a stable text to compare states by.
pub fn dumps_sorted(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value, None, 0, true);
    out
}

fn write_json(out: &mut String, value: &Value, indent: Option<usize>, depth: usize, sort: bool) {
    let newline = |out: &mut String, depth: usize| {
        if let Some(step) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(step * depth));
        }
    };
    let separator = if indent.is_some() { "," } else { ", " };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&match n.as_f64() {
            Some(f) if f.is_nan() => "NaN".into(),
            _ => number(n),
        }),
        Value::String(text) => json_string(out, text),
        Value::Array(items) => {
            if items.is_empty() {
                return out.push_str("[]");
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(separator);
                }
                newline(out, depth + 1);
                write_json(out, item, indent, depth + 1, sort);
            }
            newline(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                return out.push_str("{}");
            }
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            if sort {
                keys.sort();
            }
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(separator);
                }
                newline(out, depth + 1);
                json_string(out, key);
                out.push_str(": ");
                write_json(out, &map[key], indent, depth + 1, sort);
            }
            newline(out, depth);
            out.push('}');
        }
    }
}

fn json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7f => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A value as a number, for arithmetic (bools and others count as nothing).
pub fn f(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

/// Python's round(x, digits): an int stays an int, a float is rounded on its decimal digits.
pub fn round(value: &Value, digits: usize) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => fnum(round_f(n.as_f64().unwrap(), digits)),
        other => other.clone(),
    }
}

pub fn round_f(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// A float as a JSON number (NaN and infinities have none: they become null).
pub fn fnum(value: f64) -> Value {
    serde_json::Number::from_f64(value).map(Value::Number).unwrap_or(Value::Null)
}

/// A number that keeps Python's type: an int when both sides were ints.
pub fn add(a: &Value, b: &Value) -> Value {
    match (a.as_i64(), b.as_i64()) {
        (Some(x), Some(y)) if a.is_i64() && b.is_i64() => Value::from(x + y),
        _ => fnum(f(a) + f(b)),
    }
}

pub fn sub(a: &Value, b: &Value) -> Value {
    match (a.as_i64(), b.as_i64()) {
        (Some(x), Some(y)) if a.is_i64() && b.is_i64() => Value::from(x - y),
        _ => fnum(f(a) - f(b)),
    }
}

pub fn mul(a: &Value, b: &Value) -> Value {
    match (a.as_i64(), b.as_i64()) {
        (Some(x), Some(y)) if a.is_i64() && b.is_i64() => Value::from(x * y),
        _ => fnum(f(a) * f(b)),
    }
}

/// Python comparison of two JSON values of one kind (numbers by value, strings by text).
pub fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => f(a).partial_cmp(&f(b)).unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// Python equality: 1 == 1.0, lists and dicts by their items.
pub fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => f(a) == f(b),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| equal(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equal(v, w)))
        }
        _ => a == b,
    }
}

/// The first item with the largest key, as Python's max() picks it (Rust's max_by keeps the last).
pub fn max_first<T, K: PartialOrd>(items: impl IntoIterator<Item = T>, key: impl Fn(&T) -> K) -> Option<T> {
    let mut best: Option<(T, K)> = None;
    for item in items {
        let k = key(&item);
        if best.as_ref().is_none_or(|(_, b)| k > *b) {
            best = Some((item, k));
        }
    }
    best.map(|(item, _)| item)
}

/// The first item with the smallest key, as Python's min() picks it.
pub fn min_first<T, K: PartialOrd>(items: impl IntoIterator<Item = T>, key: impl Fn(&T) -> K) -> Option<T> {
    let mut best: Option<(T, K)> = None;
    for item in items {
        let k = key(&item);
        if best.as_ref().is_none_or(|(_, b)| k < *b) {
            best = Some((item, k));
        }
    }
    best.map(|(item, _)| item)
}

/// Python truthiness of a JSON value.
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(_)) => f(value.unwrap()) != 0.0,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

/// `{:.Nf}` of a JSON number.
pub fn fixed(value: &Value, digits: usize) -> String {
    format!("{:.digits$}", f(value))
}

/// `{:.0%}`
pub fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prints_like_python() {
        assert_eq!(float(381.0), "381.0");
        assert_eq!(float(0.1), "0.1");
        assert_eq!(float(1e-5), "1e-05");
        assert_eq!(float(1.5e16), "1.5e+16");
        assert_eq!(repr(&json!([383, 1.5, "a", null, {"k": true}])), "[383, 1.5, 'a', None, {'k': True}]");
        assert_eq!(dumps(&json!({"a": [1, 2.0], "b": "\u{e9}"})), "{\"a\": [1, 2.0], \"b\": \"\\u00e9\"}");
        assert_eq!(dumps_pretty(&json!({"b": 1, "a": [1]})), "{\n  \"a\": [\n    1\n  ],\n  \"b\": 1\n}");
        assert_eq!(round(&json!(2.675), 2), json!(2.67));
        assert_eq!(round(&json!(3), 2), json!(3));
        assert_eq!(max_first(vec![(1, 'a'), (2, 'b'), (2, 'c')], |p| p.0), Some((2, 'b')));
    }
}
