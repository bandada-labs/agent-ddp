//! `check overlap`: a text something is drawn over. The app's own pixels decide (inspect). iOS: the screen shows the
//! same color where the text has ink as where it has none, so its shape cannot be seen. Android: the text's colors
//! show nowhere in its frame. Partly covered text is not reported; the pixels cannot tell it for sure.
use crate::args::Args;
use crate::inspect::VISIBLE;
use crate::model::Recording;
use crate::py;
use crate::say;
use crate::state::Result;
use serde_json::Value;

/// A text is covered when this share of its ink points shows the color of its clear points.
pub const COVERED_INK: f64 = 0.9;

/// Why a check that needs the platform's answers says nothing about a take.
pub fn not_inspected(recording: &Recording) -> Option<&'static str> {
    if recording.inspect.is_null() {
        Some("the take has no screen check (history, or a file recorded before 0.3.0); run it live, or record it again")
    } else if recording.inspect.get("pixels").is_some() {
        Some("the app did not answer screen.pixels")
    } else {
        None
    }
}

pub fn check_overlap(args: &Args, recording: &Recording) -> Result<usize> {
    if let Some(why) = not_inspected(recording) {
        if !args.shared {
            say!("overlap not judged: {why}");
        }
        return Ok(0);
    }
    let mut problems = 0;
    for text in recording.inspect["texts"].as_array().into_iter().flatten() {
        if text.get("ink").is_some() {
            if text["inkLikeClear"].as_f64().unwrap_or(0.0) < COVERED_INK {
                continue;
            }
            problems += 1;
            say!(
                "COVERED {}: the screen shows {} where its text has ink and around it, so the text cannot be seen in {}",
                py::str(&text["key"]),
                py::str(&text["clear"]),
                py::str(&text["rect"])
            );
            continue;
        }
        let seen: Vec<i64> = text["seen"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
        if seen.is_empty() || seen.iter().any(|d| *d <= VISIBLE as i64) {
            continue;
        }
        problems += 1;
        let colors: Vec<String> = text["colors"].as_array().into_iter().flatten().map(py::str).collect();
        say!(
            "COVERED {}: its text ({}) shows nowhere in its frame {}; the screen there is {}",
            py::str(&text["key"]),
            colors.join(", "),
            py::str(&text["rect"]),
            py::str(&text["screen"])
        );
    }
    if problems == 0 && !args.shared {
        say!("no covered text");
    }
    Ok(problems)
}
