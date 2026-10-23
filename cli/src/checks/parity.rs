//! `check parity`: the same screen on iOS and Android.
use super::{PARITY_ALPHA, PARITY_PT};
use crate::args::Args;
use crate::model::{color_alpha, element_keys, element_state, load, node_alpha, with_origin, Recording};
use crate::py::{self, f, truthy};
use crate::state::{ctx, Result};
use crate::{fail, say};
use serde_json::Value;

const PARITY_TIMING_MS: f64 = 50.0; // three frames at 60 Hz

struct Profile {
    changes: Option<(f64, f64)>,
    fields: [f64; 4], // alpha, corner, shadow, shadowAlpha
}

/// Timing (from the recording's first change), peak alpha, corner and shadow of one element, in pt.
fn parity_profile(recording: &Recording, key: &str) -> Profile {
    let to_pt = recording.to_pt();
    let frames = &recording.frames;
    let states: Vec<String> =
        frames.iter().map(|f| py::dumps_sorted(&element_state(f, key).unwrap_or(Value::Null))).collect();
    let changes: Vec<f64> = (1..frames.len()).filter(|&i| states[i] != states[i - 1]).map(|i| frames[i].t).collect();
    let owned: Vec<&Value> = frames.iter().flat_map(|f| f.owned(Some(key))).collect();
    let shadows: Vec<&Value> = owned.iter().filter_map(|n| n.get("shadow").filter(|s| truthy(Some(s)))).collect();
    let max = |values: Vec<f64>| {
        values.into_iter().fold(None, |b: Option<f64>, v| Some(b.map_or(v, |b| b.max(v)))).unwrap_or(0.0)
    };
    Profile {
        changes: (!changes.is_empty())
            .then(|| (changes[0] - recording.origin, changes[changes.len() - 1] - recording.origin)),
        fields: [
            max(owned.iter().map(|n| f(&node_alpha(n))).collect()),
            max(owned.iter().map(|n| n.get("corner").map(f).unwrap_or(0.0)).collect()) * to_pt,
            max(shadows.iter().map(|s| f(&s["radius"])).collect()) * to_pt,
            max(shadows.iter().map(|s| f(&s["opacity"]) * f(&color_alpha(&s["color"]))).collect()),
        ],
    }
}

fn parity_differences(a: &Profile, b: &Profile, names: (&str, &str)) -> Vec<String> {
    let mut found = Vec::new();
    match (a.changes, b.changes) {
        (Some(_), None) => found.push(format!("timing: changes in {} only", names.0)),
        (None, Some(_)) => found.push(format!("timing: changes in {} only", names.1)),
        (Some(x), Some(y)) if (x.0 - y.0).abs() > PARITY_TIMING_MS || (x.1 - y.1).abs() > PARITY_TIMING_MS => {
            found.push(format!("timing: {} {:.0}..{:.0}ms, {} {:.0}..{:.0}ms", names.0, x.0, x.1, names.1, y.0, y.1));
        }
        _ => {}
    }
    for (index, (label, tolerance, unit)) in [
        ("peak alpha", PARITY_ALPHA, ""),
        ("corner", PARITY_PT, "pt"),
        ("shadow", PARITY_PT, "pt radius"),
        ("shadow alpha", PARITY_ALPHA, ""),
    ]
    .into_iter()
    .enumerate()
    {
        if (a.fields[index] - b.fields[index]).abs() > tolerance {
            found.push(format!(
                "{label}: {} {:.2}{unit}, {} {:.2}{unit}",
                names.0, a.fields[index], names.1, b.fields[index]
            ));
        }
    }
    found
}

pub fn check_parity(args: &Args) -> Result<usize> {
    let Some(against) = &args.against else {
        fail!("check parity needs --against <other recording>", "missing_option");
    };
    let file = args.file.clone().unwrap_or_else(|| ctx().last);
    let (mut left, mut right) = (load(&file)?, load(against)?);
    with_origin(&mut left);
    with_origin(&mut right);
    let mut names = (py::str(&left.header["platform"]), py::str(&right.header["platform"]));
    if names.0 == names.1 {
        names = (file.display().to_string(), against.display().to_string());
    }
    let (left_keys, right_keys) = (element_keys(&left.frames), element_keys(&right.frames));
    say!("parity {} vs {}: times from each recording's first change, sizes in pt", names.0, names.1);
    for key in &left_keys {
        if !right_keys.contains(key) {
            continue;
        }
        let found = parity_differences(&parity_profile(&left, key), &parity_profile(&right, key), (&names.0, &names.1));
        say!("{} {key}", if found.is_empty() { "same" } else { "DIFF" });
        for line in found {
            say!("  {line}");
        }
    }
    for key in left_keys.iter().filter(|k| !right_keys.contains(k)) {
        say!("only in {}: {key}", names.0);
    }
    for key in right_keys.iter().filter(|k| !left_keys.contains(k)) {
        say!("only in {}: {key}", names.1);
    }
    Ok(0)
}
