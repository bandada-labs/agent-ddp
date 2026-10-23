//! Checks over a take. Each returns how many problems it found; `run_check` turns that into exit 1.
use crate::args::Args;
use crate::live::recording_for;
use crate::model::{area, color_alpha, contains, element_alpha, element_key, element_keys, element_state, frame_at, load,
                   node_alpha, paints_at, read_text, recording_path, rect, with_origin, Frame, Recording};
use crate::py::{self, f, obj, truthy};
use crate::state::{ctx, failure, write_file, Result, OWNER_TOLERANCE_PT};
use crate::summary::numeric_tracks;
use crate::{fail, say};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::f64::consts::PI;
use std::path::Path;

type Point = (f64, f64);

/// Which of 9 sample points inside the element are painted over by something it does not own.
/// Nodes of elements inside this one (a button's own label) count as its own.
fn coverage<'a>(frame: &'a Frame, element: &Value, tolerance: f64) -> Option<(usize, Vec<&'a Value>)> {
    let key = element_key(element);
    let outer = rect(&element["frame"]);
    let inner_keys: HashSet<String> = frame.elements().iter().filter(|e| contains(outer, rect(&e["frame"]), tolerance))
        .map(element_key).collect();
    let own_top = frame.nodes().iter().filter(|n| n["owner"].as_str() == Some(&key)).map(|n| f(&n["z"]))
        .fold(None, |best: Option<f64>, z| Some(best.map_or(z, |b| b.max(z))))?;
    let [x, y, w, h] = outer;
    let mut covered = 0;
    let mut coverers: Vec<(f64, &Value)> = Vec::new();
    for fy in [0.2, 0.5, 0.8] {
        for fx in [0.2, 0.5, 0.8] {
            let point = (x + w * fx, y + h * fy);
            let above: Vec<&Value> = frame.nodes().iter().filter(|n| {
                f(&n["z"]) > own_top && !n["owner"].as_str().is_some_and(|o| inner_keys.contains(o)) && paints_at(n, point)
            }).collect();
            if let Some(top) = py::max_first(above.iter().copied(), |n| f(&n["z"])) {
                covered += 1;
                let z = f(&top["z"]);
                match coverers.iter_mut().find(|(k, _)| *k == z) {
                    Some(slot) => slot.1 = top,
                    None => coverers.push((z, top)),
                }
            }
        }
    }
    Some((covered, coverers.into_iter().map(|(_, n)| n).collect()))
}

fn pick(node: &Value, fields: &[&str]) -> Value {
    let mut details = Map::new();
    for field in fields {
        if truthy(node.get(*field)) {
            details.insert(field.to_string(), node[*field].clone());
        }
    }
    Value::Object(details)
}

pub fn check_overlap(args: &Args, recording: &Recording) -> Result<usize> {
    let frames = if args.all_frames { &recording.frames[..] } else { &recording.frames[recording.frames.len().saturating_sub(1)..] };
    let mut reported: Vec<String> = Vec::new();
    for frame in frames {
        for element in frame.elements() {
            let key = element_key(element);
            let Some((covered, coverers)) = coverage(frame, element, recording.tolerance) else { continue };
            if covered == 0 || reported.contains(&key) {
                continue;
            }
            reported.push(key.clone());
            say!("COVERED {key} at t={}ms: {covered}/9 sample points, frame {}", py::str(frame.t_value()), py::str(&element["frame"]));
            for node in coverers {
                say!("  by z={} {}", py::str(&node["z"]), py::dumps(&pick(node, &["kind", "source", "fill", "frame", "owner"])));
            }
        }
    }
    if reported.is_empty() && !args.shared {
        say!("no covered elements");
    }
    Ok(reported.len())
}

/// Whether a point is inside a rounded rectangle, give or take the tolerance.
fn inside_rounded(point: Point, r: [f64; 4], radius: f64, tolerance: f64) -> bool {
    let [x, y, w, h] = r;
    let radius = radius.min(w / 2.0).min(h / 2.0);
    // Distance from the point to the inner rectangle the corner arcs are centered on.
    let dx = (x + radius - point.0).max(0.0).max(point.0 - (x + w - radius));
    let dy = (y + radius - point.1).max(0.0).max(point.1 - (y + h - radius));
    dx.hypot(dy) <= radius + tolerance
}

/// Points on the edge of what a node paints: line ends, 8 points round an oval, or a rect's corners.
fn outline_points(node: &Value) -> Vec<Point> {
    if let Some(points) = node.get("points").and_then(Value::as_array).filter(|p| !p.is_empty()) {
        return points.chunks(2).map(|p| (f(&p[0]), f(p.get(1).unwrap_or(&json!(0))))).collect();
    }
    let [x, y, w, h] = rect(&node["frame"]);
    if matches!(node.get("op").and_then(Value::as_str), Some("circle" | "oval" | "arc")) {
        return (0..8).map(|i| {
            let a = i as f64 * PI / 4.0;
            (x + w / 2.0 + w / 2.0 * a.cos(), y + h / 2.0 + h / 2.0 * a.sin())
        }).collect();
    }
    // A rounded node's own corner is cut too: take the point on its arc at 45 degrees.
    let inset = node.get("corner").map(f).unwrap_or(0.0).min(w / 2.0).min(h / 2.0) * (1.0 - 0.5f64.sqrt());
    vec![(x + inset, y + inset), (x + w - inset, y + inset), (x + inset, y + h - inset), (x + w - inset, y + h - inset)]
}

fn clamp(point: Point, r: [f64; 4]) -> Point {
    let [x, y, w, h] = r;
    (point.0.max(x).min(x + w), point.1.max(y).min(y + h))
}

/// Nodes drawn inside an element's rounded shape that reach past its corners and are not clipped to them.
fn corner_breaks<'a>(frame: &'a Frame, element: &Value, tolerance: f64) -> Vec<(&'a Value, Point)> {
    let key = element_key(element);
    let shapes = frame.nodes().iter().filter(|n| {
        n["owner"].as_str() == Some(&key) && truthy(n.get("corner")) && (truthy(n.get("fill")) || truthy(n.get("border")))
    });
    let Some(shape) = py::max_first(shapes, |n| area(rect(&n["frame"]))) else { return Vec::new() };
    let shape_rect = rect(&shape["frame"]);
    let [sx, sy, sw, sh] = shape_rect;
    let mut breaks = Vec::new();
    for node in frame.nodes() {
        let r = rect(node.get("visible").unwrap_or(&node["frame"]));
        let [x, y, w, h] = r;
        let center_inside = sx <= x + w / 2.0 && x + w / 2.0 <= sx + sw && sy <= y + h / 2.0 && y + h / 2.0 <= sy + sh;
        // The shape's own stroke, or anything as big as the shape (a page background), is not a draw inside it.
        if std::ptr::eq(node, shape) || node["kind"] == "bitmap" || !center_inside
            || contains(rect(&node["frame"]), shape_rect, tolerance) || area(rect(&node["frame"])) > area(shape_rect) * 1.5 {
            continue;
        }
        let clip = node.get("clipRound").filter(|c| truthy(Some(c))).and_then(Value::as_array);
        let outside: Vec<Point> = outline_points(node).into_iter().map(|p| clamp(p, r)).filter(|&p| {
            !inside_rounded(p, shape_rect, f(&shape["corner"]), tolerance)
                && !clip.is_some_and(|c| !inside_rounded(p, [f(&c[0]), f(&c[1]), f(&c[2]), f(&c[3])], f(&c[4]), tolerance))
        }).collect();
        if let Some(&first) = outside.first() {
            breaks.push((node, first));
        }
    }
    breaks
}

pub fn check_corners(args: &Args, recording: &Recording) -> Result<usize> {
    let frames = if args.all_frames { &recording.frames[..] } else { &recording.frames[recording.frames.len().saturating_sub(1)..] };
    let mut reported: Vec<String> = Vec::new();
    for frame in frames {
        for element in frame.elements() {
            let key = element_key(element);
            if reported.contains(&key) {
                continue;
            }
            let breaks = corner_breaks(frame, element, recording.tolerance / OWNER_TOLERANCE_PT);
            if breaks.is_empty() {
                continue;
            }
            reported.push(key.clone());
            let shape = py::max_first(frame.nodes().iter().filter(|n| n["owner"].as_str() == Some(&key) && truthy(n.get("corner"))),
                                      |n| area(rect(&n["frame"]))).unwrap();
            say!("OUTSIDE CORNERS {key} at t={}ms: {} nodes reach past corner radius {} of {}", py::str(frame.t_value()),
                 breaks.len(), py::str(&shape["corner"]), py::str(&shape["frame"]));
            for (node, point) in breaks.iter().take(5) {
                say!("  z={} at ({:.1}, {:.1}) {}", py::str(&node["z"]), point.0, point.1,
                     py::dumps(&pick(node, &["kind", "op", "fill", "border", "frame", "points"])));
            }
        }
    }
    if reported.is_empty() && !args.shared {
        say!("nothing outside rounded corners");
    }
    Ok(reported.len())
}

const PARITY_TIMING_MS: f64 = 50.0; // three frames at 60 Hz
const PARITY_ALPHA: f64 = 0.1;
const PARITY_PT: f64 = 1.0;

struct Profile {
    changes: Option<(f64, f64)>,
    fields: [f64; 4], // alpha, corner, shadow, shadowAlpha
}

/// Timing (from the recording's first change), peak alpha, corner and shadow of one element, in pt.
fn parity_profile(recording: &Recording, key: &str) -> Profile {
    let to_pt = recording.to_pt();
    let frames = &recording.frames;
    let states: Vec<String> = frames.iter().map(|f| py::dumps_sorted(&element_state(f, key).unwrap_or(Value::Null))).collect();
    let changes: Vec<f64> = (1..frames.len()).filter(|&i| states[i] != states[i - 1]).map(|i| frames[i].t).collect();
    let owned: Vec<&Value> = frames.iter().flat_map(|f| f.owned(Some(key))).collect();
    let shadows: Vec<&Value> = owned.iter().filter_map(|n| n.get("shadow").filter(|s| truthy(Some(s)))).collect();
    let max = |values: Vec<f64>| values.into_iter().fold(None, |b: Option<f64>, v| Some(b.map_or(v, |b| b.max(v)))).unwrap_or(0.0);
    Profile {
        changes: (!changes.is_empty()).then(|| (changes[0] - recording.origin, changes[changes.len() - 1] - recording.origin)),
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
    for (index, (label, tolerance, unit)) in [("peak alpha", PARITY_ALPHA, ""), ("corner", PARITY_PT, "pt"),
                                               ("shadow", PARITY_PT, "pt radius"), ("shadow alpha", PARITY_ALPHA, "")]
        .into_iter().enumerate() {
        if (a.fields[index] - b.fields[index]).abs() > tolerance {
            found.push(format!("{label}: {} {:.2}{unit}, {} {:.2}{unit}", names.0, a.fields[index], names.1, b.fields[index]));
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
    element.get("label").and_then(Value::as_str).unwrap_or("").to_lowercase().contains(&py::str(&case["text"]).to_lowercase())
}

/// Times (from the origin) of frames where the element is there, strong enough and in place,
/// plus why it never was when the list is empty.
fn shown_times(recording: &Recording, case: &Value, to_pt: f64) -> (Vec<f64>, Option<String>) {
    let min_alpha = case.get("minAlpha").map(f).unwrap_or(0.05);
    let tolerance_pt = case.get("tolerancePt").map(f).unwrap_or(4.0);
    let (mut times, mut present, mut strong) = (Vec::new(), false, false);
    for frame in &recording.frames {
        let element = frame.elements().iter()
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
        format!("there but never at {} (within {} pt)", py::str(&case["frame"]),
                case.get("tolerancePt").map(py::str).unwrap_or("4".into()))
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

fn timeline_verdict(case: &Value, times: &[f64], why: Option<String>, tolerance: f64, recording_start: f64) -> (&'static str, String) {
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
    say!("timeline: times from the recording's {origin_name} (t={:.0}ms), tolerance {}ms", recording.origin, py::str(&tolerance_value));
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
            detail += &format!("; the recording ends at {recording_end:.0}ms, before this case (next: record longer with --for)");
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

fn channel(color: &str, at: usize) -> f64 {
    u8::from_str_radix(color.get(at..at + 2).unwrap_or("00"), 16).unwrap_or(0) as f64
}

fn luminance(color: &str) -> f64 {
    let linear: Vec<f64> = [1, 3, 5].iter().map(|&i| {
        let c = channel(color, i) / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }).collect();
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

/// A translucent color composited over an opaque background.
fn over(color: &str, background: &str) -> String {
    let alpha = channel(color, 7) / 255.0;
    let mixed: String = [1, 3, 5].iter()
        .map(|&i| format!("{:02X}", (channel(color, i) * alpha + channel(background, i) * (1.0 - alpha)).round_ties_even() as u8))
        .collect();
    format!("#{mixed}FF")
}

fn contrast(text_color: &str, background: &str) -> f64 {
    let (a, b) = (luminance(text_color), luminance(background));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The opaque color behind a node: the fills below it that hold its center, blended upwards.
/// None when what is right behind it is not a plain fill.
fn background_under(frame: &Frame, node: &Value) -> Option<String> {
    let [x, y, w, h] = rect(&node["frame"]);
    let center = (x + w / 2.0, y + h / 2.0);
    let z = f(&node["z"]);
    let under: Vec<&Value> = frame.nodes().iter().filter(|n| f(&n["z"]) < z && paints_at(n, center)).collect();
    if let Some(top) = py::max_first(under.iter().copied(), |n| f(&n["z"])) {
        if !truthy(top.get("fill")) {
            return None; // text over a gradient, an image or a drawing: no single color to compare with
        }
    }
    let mut below: Vec<&Value> = under.into_iter().filter(|n| truthy(n.get("fill"))).collect();
    below.sort_by(|a, b| f(&a["z"]).partial_cmp(&f(&b["z"])).unwrap());
    let mut color: Option<String> = None;
    for fill in below {
        let fill = py::str(&fill["fill"]);
        color = match color {
            None => fill.ends_with("FF").then_some(fill),
            Some(base) => Some(over(&fill, &base)),
        };
    }
    color
}

/// Small touch targets, controls nobody can name, and text without enough contrast (WCAG AA).
pub fn check_a11y(args: &Args, recording: &Recording) -> Result<usize> {
    let to_pt = recording.to_pt();
    let minimum = if recording.header["platform"] == "android" { 48.0 } else { 44.0 }; // pt / dp: Apple HIG and Material
    let mut problems: Vec<(String, String)> = Vec::new();
    let set = |problems: &mut Vec<(String, String)>, key: String, line: String| {
        match problems.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = line,
            None => problems.push((key, line)),
        }
    };
    let last = recording.frames.last().unwrap();
    for element in last.elements() {
        if !truthy(element.get("tap")) {
            continue;
        }
        let key = element_key(element);
        let size = rect(element.get("touch").unwrap_or(&element["frame"]));
        let (w, h) = (size[2] * to_pt, size[3] * to_pt);
        if w < minimum - 0.5 || h < minimum - 0.5 {
            set(&mut problems, key.clone(), format!("SMALL TARGET {key}: {w:.0}x{h:.0}, under {minimum}x{minimum}"));
        } else if !truthy(element.get("label")) {
            set(&mut problems, key.clone(), format!("NO LABEL {key}: a control a screen reader can't name"));
        }
    }
    let mut worst: BTreeMap<String, f64> = BTreeMap::new(); // per element, the frame where its text is hardest to read
    let mut unjudged: HashSet<String> = HashSet::new();
    for frame in &recording.frames {
        for node in frame.nodes() {
            let text = node.get("text").filter(|t| t.is_object()).cloned().unwrap_or(json!({}));
            let key = node["owner"].as_str().filter(|o| !o.is_empty()).map(String::from)
                .unwrap_or_else(|| format!("text at {}", py::str(&node["frame"])));
            let has_problem = problems.iter().any(|(k, _)| *k == key);
            if !truthy(text.get("color")) || (has_problem && !worst.contains_key(&key)) {
                continue;
            }
            let Some(background) = background_under(frame, node) else {
                unjudged.insert(key);
                continue;
            };
            let color = py::str(&text["color"]);
            let ratio = contrast(&over(&color, &background), &background);
            let size_pt = text.get("size").map(f).unwrap_or(0.0) * to_pt;
            let large = size_pt >= 18.0 || (size_pt >= 14.0 && text.get("weight").map(f).unwrap_or(400.0) >= 700.0);
            let needed = if large { 3.0 } else { 4.5 };
            if ratio < needed && ratio < *worst.get(&key).unwrap_or(&needed) {
                worst.insert(key.clone(), ratio);
                let estimated = if truthy(text.get("estimated")) { " (color read from pixels)" } else { "" };
                set(&mut problems, key.clone(), format!("LOW CONTRAST {key}: {ratio:.1}:1 at t={:.0}ms, {color} on {background}, needs {}:1{estimated}",
                                                        frame.t, if large { "3" } else { "4.5" }));
            }
        }
    }
    for (_, line) in &problems {
        say!("{line}");
    }
    for (key, _) in &problems {
        unjudged.remove(key);
    }
    if !args.shared {
        if problems.is_empty() {
            say!("no accessibility problems found");
        }
        if !unjudged.is_empty() {
            let mut sorted: Vec<String> = unjudged.into_iter().collect();
            sorted.sort();
            say!("contrast not judged (text over a gradient, image or drawing): {}", sorted.join(", "));
        }
    }
    Ok(problems.len())
}

const JANK_FACTOR: f64 = 3.0; // a frame interval 3x the usual one means at least two missed refreshes
const SETTLE_WINDOW_MS: f64 = 500.0;
const JITTER_PT: f64 = 1.0;

/// Missed refreshes from the frame timestamps the recorder already writes.
pub fn check_jank(args: &Args, recording: &Recording) -> Result<usize> {
    if truthy(recording.header.get("history")) {
        if !args.shared {
            say!("jank not judged: a history take keeps only the frames that changed; record live for jank");
        }
        return Ok(0);
    }
    let times: Vec<f64> = recording.frames.iter().map(|f| f.t).collect();
    let mut intervals: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    if intervals.is_empty() {
        return Ok(0);
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let usual = intervals[intervals.len() / 2];
    let hitches: Vec<(f64, f64)> = times.windows(2).filter(|w| w[1] - w[0] > usual * JANK_FACTOR).map(|w| (w[0], w[1] - w[0])).collect();
    if let Some(worst) = py::max_first(hitches.iter(), |h| h.1) {
        let listed: Vec<String> = hitches.iter().take(8).map(|(t, gap)| format!("{t:.0}ms+{gap:.0}")).collect();
        say!("JANK {} hitches over {:.0}ms (usual frame {usual:.1}ms), worst {:.0}ms at t={:.0}ms: {}{}", hitches.len(),
             times[times.len() - 1] - times[0], worst.1, worst.0, listed.join(", "), if hitches.len() > 8 { " ..." } else { "" });
        if let Some(average) = recording.end.get("averageCaptureMs").filter(|v| !v.is_null()) {
            // Live takes end without the recorder's numbers.
            say!("  capture cost per frame: average {} ms, slowest {} ms (the recorder's own share)", py::str(average),
                 py::str(recording.end.get("slowestCaptureMs").unwrap_or(&Value::Null)));
        }
    } else if !args.shared {
        say!("no jank: {} frames, usual frame {usual:.1}ms", times.len());
    }
    Ok(hitches.len())
}

/// Elements still moving at the end of the take (an animation that never stops),
/// or shaking back and forth by about a point (jitter).
pub fn check_settle(args: &Args, recording: &Recording) -> Result<usize> {
    let frames: Vec<&Frame> = recording.frames.iter().collect();
    let to_pt = recording.to_pt();
    if frames[frames.len() - 1].t - frames[0].t < 2.0 * SETTLE_WINDOW_MS {
        if !args.shared {
            say!("take too short to tell: settle needs {}ms or more", 2.0 * SETTLE_WINDOW_MS);
        }
        return Ok(0);
    }
    let window_start = frames[frames.len() - 1].t - SETTLE_WINDOW_MS;
    let mut problems = 0;
    for key in element_keys(&recording.frames) {
        for (name, points) in numeric_tracks(&frames, &key) {
            let tail: Vec<&Value> = points.iter().filter(|(t, _)| f(t) >= window_start).map(|(_, v)| v).collect();
            let steps: Vec<f64> = tail.windows(2).filter(|w| !py::equal(w[0], w[1])).map(|w| f(w[1]) - f(w[0])).collect();
            if steps.len() < 3 {
                continue;
            }
            let turns = steps.windows(2).filter(|w| (w[0] > 0.0) != (w[1] > 0.0)).count();
            let values: Vec<f64> = tail.iter().map(|v| f(v)).collect();
            let spread = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - values.iter().cloned().fold(f64::INFINITY, f64::min);
            let amplitude = spread * if ["x", "y", "w", "h", "border"].contains(&name) { to_pt } else { 1.0 };
            if turns >= 2 && amplitude <= JITTER_PT && ["x", "y", "w", "h"].contains(&name) {
                say!("JITTER {key}: {name} shakes by {amplitude:.2}pt, {turns} turns in the last {}ms", SETTLE_WINDOW_MS);
            } else {
                say!("NOT SETTLED {key}: {name} still changes in the last {}ms ({} changes, {} -> {}). A loop, or record longer to tell",
                     SETTLE_WINDOW_MS, steps.len(), py::str(tail[0]), py::str(tail[tail.len() - 1]));
            }
            problems += 1;
            break; // one line per element
        }
    }
    if problems == 0 && !args.shared {
        say!("everything settled in the last {}ms", SETTLE_WINDOW_MS);
    }
    Ok(problems)
}

const BASELINE_DIR: &str = "ddp-baselines";

/// Keep the last take as a baseline next to the code, so it can be committed and checked in CI.
pub fn baseline(args: &mut Args) -> Result<i32> {
    let source = match &args.file {
        Some(file) => recording_path(&file.to_string_lossy()),
        None => ctx().last,
    };
    if !source.exists() {
        fail!(format!("{} not found", source.display()), "file_not_found", "ddp record first");
    }
    let name = args.name.clone().unwrap_or_default();
    let target = Path::new(BASELINE_DIR).join(format!("{name}.jsonl.gz"));
    write_file(&target, crate::model::gzip(read_text(&source)?.as_bytes()))?;
    let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    say!("baseline {name} -> {} ({size} bytes)", target.display());
    Ok(0)
}

pub fn check_baseline(args: &Args) -> Result<usize> {
    let Some(against) = &args.against else {
        fail!("check baseline needs --against <baseline name>", "missing_option", "ddp baseline save NAME, then --against NAME");
    };
    let name = against.display().to_string();
    let stored = Path::new(BASELINE_DIR).join(format!("{name}.jsonl.gz"));
    let path = if stored.exists() { stored } else { recording_path(&name) };
    let mut expected = load(&path)?;
    let mut actual = recording_for(args)?;
    with_origin(&mut expected);
    with_origin(&mut actual);
    let mut keys = element_keys(&expected.frames);
    for key in element_keys(&actual.frames) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    type FirstChange = Option<(f64, Vec<String>)>;
    let mut results: Vec<(String, FirstChange)> = Vec::new();
    for key in keys {
        let mut first = None;
        for frame in &actual.frames {
            let t = frame.t - actual.origin;
            let other = frame_at(&expected.frames, t + expected.origin);
            let found = state_differences(&comparable_state(&expected, other, &key), &comparable_state(&actual, frame, &key), &[]);
            if !found.is_empty() {
                first = Some((t, found));
                break;
            }
        }
        if let Some((t, found)) = &first {
            say!("CHANGED {key} from t={t:.0}ms: {} (baseline vs now)", found.iter().take(3).cloned().collect::<Vec<_>>().join("; "));
        }
        results.push((key, first));
    }
    let changed = results.iter().filter(|(_, first)| first.is_some()).count();
    say!("baseline {name}: {changed} of {} elements changed", results.len());
    if let Some(junit) = &args.junit {
        let cases: Vec<(String, Option<String>)> = results.iter().map(|(key, first)| {
            (key.clone(), first.as_ref().map(|(t, found)| format!("from t={t:.0}ms: {}", found.join("; "))))
        }).collect();
        write_junit(junit, &format!("ddp baseline {name}"), &cases)?;
    }
    Ok(changed)
}

fn quote_attribute(text: &str) -> String {
    let escaped = text.replace('&', "&amp;").replace('>', "&gt;").replace('<', "&lt;")
        .replace('\n', "&#10;").replace('\r', "&#13;").replace('\t', "&#9;");
    if escaped.contains('"') {
        if escaped.contains('\'') { format!("\"{}\"", escaped.replace('"', "&quot;")) } else { format!("'{escaped}'") }
    } else {
        format!("\"{escaped}\"")
    }
}

/// cases: (name, failure message or None).
fn write_junit(path: &Path, suite: &str, cases: &[(String, Option<String>)]) -> Result<()> {
    let failures = cases.iter().filter(|(_, m)| m.is_some()).count();
    let mut lines = vec![format!("<testsuite name={} tests=\"{}\" failures=\"{failures}\">", quote_attribute(suite), cases.len())];
    for (name, message) in cases {
        lines.push(match message {
            Some(message) => format!("  <testcase name={}><failure message={}/></testcase>", quote_attribute(name), quote_attribute(message)),
            None => format!("  <testcase name={}/>", quote_attribute(name)),
        });
    }
    lines.push("</testsuite>".into());
    write_file(path, lines.join("\n") + "\n")
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
    say!("check all over {} frames: {} (ran {})", recording.frames.len(),
         if found.is_empty() { "no problems".to_string() } else { found.iter().map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ") },
         ran.join(", "));
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
    obj(vec![("present", json!(true)), ("frame", json!(frame_pt)), ("alpha", py::round(&element_alpha(frame, key), 2)),
             ("fill", state["fill"].clone()), ("border", in_pt(&state["border"])),
             ("borderColor", state["borderColor"].clone()), ("opacity", state["opacity"].clone()), ("draws", state["draws"].clone())])
        .as_object().unwrap().clone()
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
            (Value::Number(_), Value::Number(_)) => (f(left) - f(right)).abs() > if field == "draws" { 0.0 } else { tolerance },
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

pub const RECORDING_CHECKS: [(&str, RecordingCheck); 5] = [("overlap", check_overlap), ("corners", check_corners),
    ("jank", check_jank), ("settle", check_settle), ("a11y", check_a11y)];

pub const CHECK_NAMES: [&str; 9] = ["overlap", "corners", "jank", "settle", "a11y", "parity", "timeline", "baseline", "all"];

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
        _ => check_all(args),
    }
}
