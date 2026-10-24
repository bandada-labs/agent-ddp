//! `read`: what changed per element over a take, sparklines, and `get --curve`: each move and its easing.
use crate::args::Args;
use crate::live::recording_for;
use crate::model::{element_alpha, element_keys, element_state, Frame, Recording};
use crate::py::{self, f};
use crate::state::{ctx, Result};
use crate::{fail, say};
use serde_json::{json, Value};

pub fn describe_track(name: &str, states: &[Option<Value>], times: &[Value]) -> Option<String> {
    // Frames where the element is absent are skipped; a value of None (no fill) is a real value.
    let present: Vec<(&Value, &Value)> =
        times.iter().zip(states).filter_map(|(t, s)| s.as_ref().map(|s| (t, &s[name]))).collect();
    let texts: Vec<String> = present.iter().map(|(_, v)| py::dumps(v)).collect();
    if texts.iter().collect::<std::collections::HashSet<_>>().len() <= 1 {
        return None;
    }
    let (first, last) = (present[0].1, present[present.len() - 1].1);
    let note = if texts[0] == texts[texts.len() - 1] {
        "  TRANSIENT (same at start and end, different in between)"
    } else {
        ""
    };
    let change_times: Vec<&Value> =
        (1..present.len()).filter(|&i| texts[i] != texts[i - 1]).map(|i| present[i].0).collect();
    let window = format!("changing {}..{}ms", py::str(change_times[0]), py::str(change_times[change_times.len() - 1]));
    if present.iter().all(|(_, v)| v.is_number()) {
        let (peak_t, peak) = py::max_first(present.iter().copied(), |p| f(p.1)).unwrap();
        let (low_t, low) = py::min_first(present.iter().copied(), |p| f(p.1)).unwrap();
        let points: Vec<(f64, f64)> = present.iter().map(|(t, v)| (f(t), f(v))).collect();
        return Some(format!(
            "  {name}: {} -> {} {}, {window}, min {} @{}ms, max {} @{}ms{note}",
            py::str(first),
            py::str(last),
            sparkline(&points, 12),
            py::str(low),
            py::str(low_t),
            py::str(peak),
            py::str(peak_t)
        ));
    }
    Some(format!(
        "  {name}: {} -> {}, {} values, {window}{note}",
        py::str(first),
        py::str(last),
        change_times.len() + 1
    ))
}

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The shape of a numeric track in a few characters: per time slice, the value farthest from where the
/// track started, so a dip shows as well as a peak.
pub fn sparkline(points: &[(f64, f64)], width: usize) -> String {
    let (start, end) = (points[0].0, points[points.len() - 1].0);
    let low = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let high = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let span = if end - start != 0.0 { end - start } else { 1.0 };
    let mut slices: Vec<Vec<f64>> = vec![Vec::new(); width];
    for &(t, value) in points {
        let index = (((t - start) / span * width as f64) as usize).min(width - 1);
        slices[index].push(value);
    }
    let range = if high - low != 0.0 { high - low } else { 1.0 };
    let mut last = points[0].1;
    slices
        .iter()
        .map(|values| {
            // An empty slice repeats the last.
            if let Some(&v) = py::max_first(values.iter(), |v| (**v - points[0].1).abs()) {
                last = v;
            }
            SPARK[((last - low) / range * (SPARK.len() - 1) as f64).round_ties_even() as usize]
        })
        .collect()
}

pub fn read(args: &mut Args) -> Result<i32> {
    let recording = recording_for(args)?;
    let frames = &recording.frames;
    let times: Vec<Value> = frames.iter().map(|f| f.t_value().clone()).collect();
    let known = element_keys(frames);
    if let Some(id) = &args.id {
        if !known.contains(id) {
            fail!(
                format!("no element '{id}'. Known: {}", known.join(", ")),
                "unknown_element",
                "ddp tree, or ddp find text <part of a label>"
            );
        }
    }
    let keys = args.id.clone().map(|id| vec![id]).unwrap_or(known);
    if ctx().json {
        for frame in frames {
            let mut states = serde_json::Map::new();
            for key in &keys {
                states.insert(key.clone(), element_state(frame, key).unwrap_or(Value::Null));
            }
            say!("{}", py::dumps(&json!({"i": frame.v["i"], "t": frame.v["t"], "elements": states})));
        }
        return Ok(0);
    }
    let header = &recording.header;
    say!(
        "{} {} frames over {} ms, unit {}",
        py::str(&header["platform"]),
        frames.len(),
        py::fixed(times.last().unwrap_or(&json!(0)), 0),
        py::str(&header["unit"])
    );
    for key in &keys {
        let states: Vec<Option<Value>> = frames.iter().map(|f| element_state(f, key)).collect();
        if args.id.is_some() {
            print_timeline(key, &frames.iter().collect::<Vec<_>>(), &states);
            continue;
        }
        let last = states.iter().rev().flatten().next().unwrap();
        let draws = if states.iter().flatten().any(|s| py::truthy(Some(&s["draws"]))) {
            format!(" draws {}", py::str(&last["draws"]))
        } else {
            String::new()
        };
        let kinds: Vec<String> = last["kinds"].as_array().unwrap().iter().map(py::str).collect();
        say!(
            "{key}: frame {} fill {} border {}{draws} kinds {}",
            py::str(&last["frame"]),
            py::str(&last["fill"]),
            py::str(&last["border"]),
            if kinds.is_empty() { "none".into() } else { kinds.join(",") }
        );
        for name in ["frame", "drawn", "fill", "border", "borderColor", "opacity", "draws", "pixels", "label", "kinds"]
        {
            if let Some(line) = describe_track(name, &states, &times) {
                say!("{line}");
            }
        }
    }
    Ok(0)
}

pub fn print_timeline(key: &str, frames: &[&Frame], states: &[Option<Value>]) {
    say!("{key}: t(ms) drawn alpha fill border borderColor opacity draws pixels (repeated rows skipped)");
    let mut previous: Option<String> = None;
    for (frame, state) in frames.iter().zip(states) {
        let row = match state {
            None => "absent".to_string(),
            Some(s) => {
                let pixels = if py::truthy(Some(&s["pixels"])) { py::str(&s["pixels"]) } else { String::new() };
                format!(
                    "{} {} {} {} {} {} {} {pixels}",
                    py::str(&s["drawn"]),
                    py::fixed(&element_alpha(frame, key), 2),
                    py::str(&s["fill"]),
                    py::str(&s["border"]),
                    py::str(&s["borderColor"]),
                    py::str(&s["opacity"]),
                    py::str(&s["draws"])
                )
                .trim_end()
                .to_string()
            }
        };
        if previous.as_ref() != Some(&row) {
            say!("  {:>8} {row}", py::str(frame.t_value()));
        }
        previous = Some(row);
    }
}

/// CSS / SwiftUI / Compose named timing curves as cubic Bezier control points.
pub const EASINGS: [(&str, [f64; 4]); 5] = [
    ("linear", [0.0, 0.0, 1.0, 1.0]),
    ("easeIn", [0.42, 0.0, 1.0, 1.0]),
    ("easeOut", [0.0, 0.0, 0.58, 1.0]),
    ("easeInOut", [0.42, 0.0, 0.58, 1.0]),
    ("fastOutSlowIn", [0.4, 0.0, 0.2, 1.0]),
];

/// y of a timing curve at time x (0..1), found by bisection on the curve's x.
pub fn bezier_progress(control: [f64; 4], x: f64) -> f64 {
    let [x1, y1, x2, y2] = control;
    let at = |a: f64, b: f64, u: f64| 3.0 * a * u * (1.0 - u).powi(2) + 3.0 * b * u * u * (1.0 - u) + u.powi(3);
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..30 {
        let middle = (low + high) / 2.0;
        if at(x1, x2, middle) < x {
            low = middle
        } else {
            high = middle
        }
    }
    at(y1, y2, (low + high) / 2.0)
}

type Point = (Value, Value);

/// Moves of a numeric track: runs of change between holds, split again at a turn (rise then fall).
fn segments(points: &[Point]) -> Vec<Vec<Point>> {
    let mut runs: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    for index in 1..points.len() {
        let moving = !py::equal(&points[index].1, &points[index - 1].1);
        if moving {
            if current.is_empty() {
                current.push(points[index - 1].clone());
            }
            current.push(points[index].clone());
        } else if !current.is_empty()
            && (index..(index + 3).min(points.len())).all(|j| py::equal(&points[j].1, &points[index].1))
        {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    let mut split = Vec::new();
    for run in runs {
        let (start, end) = (f(&run[0].1), f(&run[run.len() - 1].1));
        let extreme = py::max_first(0..run.len(), |&i| (f(&run[i].1) - start).abs()).unwrap();
        // Comes back to where it started (a flash): two moves, out and back.
        if 0 < extreme && extreme < run.len() - 1 && (end - start).abs() < (f(&run[extreme].1) - start).abs() / 2.0 {
            split.push(run[..=extreme].to_vec());
            split.push(run[extreme..].to_vec());
        } else {
            split.push(run);
        }
    }
    split
}

/// `0 -> 4 over 400ms from 550ms, matches easeOut (off by 1%)`, or its overshoot. A comparison of the recorded values
/// with named curves, not the curve the app asked for: SwiftUI and Compose do not say which one they run.
fn describe_move(run: &[Point]) -> String {
    let ((t0, v0), (t1, v1)) = (&run[0], &run[run.len() - 1]);
    let duration = f(t1) - f(t0);
    let distance = f(v1) - f(v0);
    let text = format!("{} -> {} over {:.0}ms from {:.0}ms", py::str(v0), py::str(v1), duration, f(t0));
    if distance == 0.0 || duration <= 0.0 || run.len() < 4 {
        return text;
    }
    let progress: Vec<(f64, f64)> =
        run.iter().map(|(t, v)| ((f(t) - f(t0)) / duration, (f(v) - f(v0)) / distance)).collect();
    let overshoot = progress.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) - 1.0;
    if overshoot > 0.02 {
        return format!(
            "{text}, overshoots by {} (a spring, or a curve that goes past its end)",
            py::percent(overshoot)
        );
    }
    let fits: Vec<(&str, f64)> = EASINGS
        .iter()
        .map(|(name, control)| {
            let error = progress.iter().map(|(x, y)| (bezier_progress(*control, *x) - y).powi(2)).sum::<f64>()
                / progress.len() as f64;
            (*name, error.sqrt())
        })
        .collect();
    let (best, fit) = py::min_first(fits, |p| p.1).unwrap();
    if fit < 0.05 {
        format!("{text}, matches {best} (off by {})", py::percent(fit))
    } else {
        format!("{text}, matches no named curve (nearest {best}, off by {})", py::percent(fit))
    }
}

pub const TRACKS: [&str; 7] = ["x", "y", "w", "h", "alpha", "border", "opacity"];

/// x, y, w, h, alpha, border, opacity of one element per frame, in the recording's unit.
pub fn numeric_tracks(frames: &[&Frame], key: &str) -> Vec<(&'static str, Vec<Point>)> {
    let mut tracks: Vec<(&str, Vec<Point>)> = TRACKS.iter().map(|n| (*n, Vec::new())).collect();
    for frame in frames {
        let Some(state) = element_state(frame, key) else { continue };
        let values = [
            state["frame"][0].clone(),
            state["frame"][1].clone(),
            state["frame"][2].clone(),
            state["frame"][3].clone(),
            py::round(&element_alpha(frame, key), 3),
            state["border"].clone(),
            state["opacity"].clone(),
        ];
        for ((_, track), value) in tracks.iter_mut().zip(values) {
            track.push((frame.t_value().clone(), value));
        }
    }
    tracks
}

pub fn print_curves(recording: &Recording, frames: &[&Frame], key: &str) {
    say!("{key} moves (unit {}):", recording.unit());
    let mut found = false;
    for (name, points) in numeric_tracks(frames, key) {
        for run in segments(&points) {
            if (f(&run[run.len() - 1].1) - f(&run[0].1)).abs() < 0.05 && run.len() < 4 {
                continue; // rounding noise, not a move
            }
            found = true;
            say!("  {name}: {}", describe_move(&run));
        }
    }
    if !found {
        say!("  no numeric change");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparklines_show_peaks_and_dips() {
        assert_eq!(sparkline(&[(0.0, 0.0), (50.0, 4.0), (100.0, 0.0)], 3), "▁█▁");
        assert_eq!(sparkline(&[(0.0, 1.0), (50.0, 0.2), (100.0, 1.0)], 3), "█▁█");
    }
}
