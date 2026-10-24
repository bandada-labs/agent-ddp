//! Checks over a take. Each returns how many problems it found; `run_check` turns that into exit 1.
mod a11y;
mod baseline;
mod corners;
mod motion;
mod overlap;
mod parity;
mod timeline;

pub use a11y::check_a11y;
pub use baseline::{baseline, check_baseline};
pub use corners::check_corners;
pub use motion::{check_jank, check_settle};
pub use overlap::check_overlap;
pub use parity::check_parity;
pub use timeline::check_timeline;

use crate::args::Args;
use crate::live::recording_for;
use crate::model::{element_alpha, element_state, Frame, Recording};
use crate::py::{self, f, obj, truthy};
use crate::say;
use crate::state::Result;
use serde_json::{json, Map, Value};

type Point = (f64, f64);

const PARITY_ALPHA: f64 = 0.1;
const PARITY_PT: f64 = 1.0;

fn pick(node: &Value, fields: &[&str]) -> Value {
    let mut details = Map::new();
    for field in fields {
        if truthy(node.get(*field)) {
            details.insert(field.to_string(), node[*field].clone());
        }
    }
    Value::Object(details)
}

/// Every check that needs only this recording, in one call: only problems are printed, exit 1 on any.
pub fn check_all(args: &Args) -> Result<usize> {
    let recording = recording_for(args)?;
    let mut shared = args.clone();
    shared.shared = true;
    let mut found: Vec<(&str, usize)> = Vec::new();
    for (name, check) in RECORDING_CHECKS {
        let count = check(&shared, &recording)?;
        if count > 0 {
            found.push((name, count));
        }
    }
    let ran: Vec<&str> = RECORDING_CHECKS.iter().map(|(n, _)| *n).collect();
    say!(
        "check all over {} frames: {} (ran {})",
        recording.frames.len(),
        if found.is_empty() {
            "no problems".to_string()
        } else {
            found.iter().map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ")
        },
        ran.join(", ")
    );
    Ok(found.iter().map(|(_, c)| c).sum())
}

/// An element's state in pt, so two platforms compare.
pub fn comparable_state(recording: &Recording, frame: &Frame, key: &str) -> Map<String, Value> {
    let Some(state) = element_state(frame, key) else {
        return obj(vec![("present", json!(false))]).as_object().unwrap().clone();
    };
    // In pt a value keeps its type (Python multiplied by the int 1); from px it becomes a float.
    let px = recording.header["unit"] == "px";
    let to_pt = recording.to_pt();
    let in_pt = |v: &Value| if px { py::fnum(py::round_f(f(v) * to_pt, 1)) } else { py::round(v, 1) };
    let frame_pt: Vec<Value> = state["frame"].as_array().unwrap().iter().map(in_pt).collect();
    obj(vec![
        ("present", json!(true)),
        ("frame", json!(frame_pt)),
        ("alpha", py::round(&element_alpha(frame, key), 2)),
        ("fill", state["fill"].clone()),
        ("border", in_pt(&state["border"])),
        ("borderColor", state["borderColor"].clone()),
        ("opacity", state["opacity"].clone()),
        ("draws", state["draws"].clone()),
    ])
    .as_object()
    .unwrap()
    .clone()
}

pub fn state_differences(a: &Map<String, Value>, b: &Map<String, Value>, ignored: &[String]) -> Vec<String> {
    let mut fields: Vec<&String> = a.keys().chain(b.keys()).collect();
    fields.sort();
    fields.dedup();
    let mut found = Vec::new();
    for field in fields {
        if ignored.contains(field) {
            continue;
        }
        let (left, right) = (a.get(field).unwrap_or(&Value::Null), b.get(field).unwrap_or(&Value::Null));
        let tolerance = if field == "alpha" || field == "opacity" { PARITY_ALPHA } else { PARITY_PT };
        let differs = match (left, right) {
            (Value::Array(x), Value::Array(y)) => x.iter().zip(y).any(|(p, q)| (f(p) - f(q)).abs() > tolerance),
            (Value::Number(_), Value::Number(_)) => {
                (f(left) - f(right)).abs() > if field == "draws" { 0.0 } else { tolerance }
            }
            _ => !py::equal(left, right),
        };
        if differs {
            found.push(format!("{field} {} vs {}", py::str(left), py::str(right)));
        }
    }
    found.sort();
    found
}

type RecordingCheck = fn(&Args, &Recording) -> Result<usize>;

pub const RECORDING_CHECKS: [(&str, RecordingCheck); 5] = [
    ("overlap", check_overlap),
    ("corners", check_corners),
    ("jank", check_jank),
    ("settle", check_settle),
    ("a11y", check_a11y),
];

pub const CHECK_NAMES: [&str; 10] =
    ["overlap", "corners", "jank", "settle", "a11y", "parity", "timeline", "baseline", "tappable", "all"];

/// One check by name: how many problems it found.
pub fn run_named(args: &Args) -> Result<usize> {
    let what = args.what.clone().unwrap_or_default();
    if let Some((_, check)) = RECORDING_CHECKS.iter().find(|(name, _)| *name == what) {
        let recording = recording_for(args)?;
        return check(args, &recording);
    }
    match what.as_str() {
        "parity" => check_parity(args),
        "timeline" => check_timeline(args),
        "baseline" => check_baseline(args),
        "tappable" => crate::hit::check_tappable(args),
        _ => check_all(args),
    }
}
