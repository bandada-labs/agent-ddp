//! `check timeline`: elements shown or absent at the expected times.
use crate::args::Args;
use crate::live::recording_for;
use crate::model::{element_alpha, element_key, read_text, rect, with_origin, Recording};
use crate::py::{self, f, truthy};
use crate::state::{failure, Result};
use crate::{fail, say};
use serde_json::{json, Value};

fn case_name(case: &Value) -> String {
    match case.get("id").filter(|v| truthy(Some(v))) {
        Some(id) => py::str(id),
        None => format!("text \"{}\"", py::str(&case["text"])),
    }
}

/// By id (element id or label, exact) or by text (part of the label, any case).
fn case_matches(case: &Value, element: &Value) -> bool {
    if let Some(id) = case.get("id") {
        return element_key(element) == py::str(id);
    }
    element
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase()
        .contains(&py::str(&case["text"]).to_lowercase())
}

/// Times (from the origin) of frames where the element is there, strong enough and in place,
/// plus why it never was when the list is empty.
fn shown_times(recording: &Recording, case: &Value, to_pt: f64) -> (Vec<f64>, Option<String>) {
    let min_alpha = case.get("minAlpha").map(f).unwrap_or(0.05);
    let tolerance_pt = case.get("tolerancePt").map(f).unwrap_or(4.0);
    let (mut times, mut present, mut strong) = (Vec::new(), false, false);
    for frame in &recording.frames {
        let element = frame
            .elements()
            .iter()
            .find(|e| case_matches(case, e) && f(&element_alpha(frame, &element_key(e))) >= min_alpha);
        let Some(element) = element else {
            present = present || frame.elements().iter().any(|e| case_matches(case, e));
            continue;
        };
        present = true;
        strong = true;
        if let Some(expected) = case.get("frame").and_then(Value::as_array) {
            let actual = rect(&element["frame"]);
            if actual.iter().zip(expected).any(|(a, b)| (a * to_pt - f(b)).abs() > tolerance_pt) {
                continue;
            }
        }
        times.push(frame.t - recording.origin);
    }
    if !times.is_empty() {
        return (times, None);
    }
    let why = if !present {
        "never in the recording".to_string()
    } else if !strong {
        format!("there but never at alpha {} or more", case.get("minAlpha").map(py::str).unwrap_or("0.05".into()))
    } else {
        format!(
            "there but never at {} (within {} pt)",
            py::str(&case["frame"]),
            case.get("tolerancePt").map(py::str).unwrap_or("4".into())
        )
    };
    (times, Some(why))
}

/// A case that must not be drawn between `at` and `until` (default: the end of the recording).
/// The window is narrowed by the tolerance at both ends, so a fade that ends on time is not counted.
fn absent_verdict(case: &Value, times: &[f64], tolerance: f64, recording_end: f64) -> (&'static str, String) {
    let start = f(&case["at"]) + tolerance;
    let end = case.get("until").map(f).unwrap_or(recording_end) - tolerance;
    let inside: Vec<f64> = times.iter().copied().filter(|t| start <= *t && *t <= end).collect();
    if inside.is_empty() {
        return ("ok", "not drawn".into());
    }
    ("SHOWN", format!("{} frames, {:.0}..{:.0}ms", inside.len(), inside[0], inside[inside.len() - 1]))
}

fn timeline_verdict(
    case: &Value,
    times: &[f64],
    why: Option<String>,
    tolerance: f64,
    recording_start: f64,
) -> (&'static str, String) {
    if times.is_empty() {
        return ("MISSING", why.unwrap_or_default());
    }
    let at = f(&case["at"]);
    let (first, last) = (times[0], times[times.len() - 1]);
    let mut problems = Vec::new();
    // Shown from the first recorded frame: it may have been there all along, which is on time for a case
    // expected from the action on, and early only for one expected later.
    let already_there = first <= recording_start && at <= tolerance;
    if first > at + tolerance {
        problems.push(format!("appears {:.0}ms late (at {first:.0}ms)", first - at));
    } else if first < at - tolerance && !already_there {
        problems.push(format!("appears {:.0}ms early (at {first:.0}ms)", at - first));
    }
    if let Some(until) = case.get("until").map(f) {
        if last < until - tolerance {
            problems.push(format!("gone {:.0}ms early (last at {last:.0}ms)", until - last));
        } else if last > until + tolerance {
            problems.push(format!("stays {:.0}ms too long (last at {last:.0}ms)", last - until));
        }
    }
    if problems.is_empty() {
        return ("ok", format!("{first:.0}..{last:.0}ms"));
    }
    let late = problems.iter().any(|p| p.contains("late") || p.contains("too long"));
    (if late { "LATE" } else { "EARLY" }, problems.join("; "))
}

pub fn check_timeline(args: &Args) -> Result<usize> {
    let Some(expect) = &args.expect else {
        fail!("check timeline needs --expect <cases.json>", "missing_option", "ddp skills get core --filter timeline");
    };
    let expected: Value = serde_json::from_str(&read_text(expect)?)
        .map_err(|e| failure(format!("{}: not JSON ({e})", expect.display()), "bad_expect", None))?;
    let mut recording = recording_for(args)?;
    let origin_name = expected.get("origin").map(py::str).unwrap_or_else(|| "first-change".into());
    if origin_name == "start" {
        recording.origin = recording.frames[0].t;
    } else {
        with_origin(&mut recording);
    }
    let to_pt = recording.to_pt();
    let tolerance_value = expected.get("toleranceMs").cloned().unwrap_or(json!(50));
    let tolerance = f(&tolerance_value);
    say!(
        "timeline: times from the recording's {origin_name} (t={:.0}ms), tolerance {}ms",
        recording.origin,
        py::str(&tolerance_value)
    );
    let mut failed = 0;
    let recording_start = recording.frames[0].t - recording.origin;
    let recording_end = recording.frames[recording.frames.len() - 1].t - recording.origin;
    for case in expected["expect"].as_array().cloned().unwrap_or_default() {
        let (times, why) = shown_times(&recording, &case, to_pt);
        let absent = truthy(case.get("absent"));
        let (verdict, mut detail) = if absent {
            absent_verdict(&case, &times, tolerance, recording_end)
        } else {
            timeline_verdict(&case, &times, why, tolerance, recording_start)
        };
        if !absent && verdict == "MISSING" && f(&case["at"]) > recording_end {
            detail += &format!(
                "; the recording ends at {recording_end:.0}ms, before this case (next: record longer with --for)"
            );
        }
        if verdict != "ok" {
            failed += 1;
        }
        let window = match case.get("until") {
            Some(until) => format!("{}..{}ms", py::str(&case["at"]), py::str(until)),
            None => format!("from {}ms", py::str(&case["at"])),
        };
        say!("{verdict} {} ({} {window}): {detail}", case_name(&case), if absent { "absent" } else { "expected" });
    }
    Ok(failed)
}
