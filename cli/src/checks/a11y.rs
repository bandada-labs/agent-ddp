//! `check a11y`: touch targets, labels and text contrast.
use crate::args::Args;
use crate::model::{element_key, paints_at, rect, Frame, Recording};
use crate::py::{self, f, truthy};
use crate::say;
use crate::state::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

fn channel(color: &str, at: usize) -> f64 {
    u8::from_str_radix(color.get(at..at + 2).unwrap_or("00"), 16).unwrap_or(0) as f64
}

fn luminance(color: &str) -> f64 {
    let linear: Vec<f64> = [1, 3, 5]
        .iter()
        .map(|&i| {
            let c = channel(color, i) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
        .collect();
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

/// A translucent color composited over an opaque background.
fn over(color: &str, background: &str) -> String {
    let alpha = channel(color, 7) / 255.0;
    let mixed: String = [1, 3, 5]
        .iter()
        .map(|&i| {
            format!(
                "{:02X}",
                (channel(color, i) * alpha + channel(background, i) * (1.0 - alpha)).round_ties_even() as u8
            )
        })
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
    let set = |problems: &mut Vec<(String, String)>, key: String, line: String| match problems
        .iter_mut()
        .find(|(k, _)| *k == key)
    {
        Some(slot) => slot.1 = line,
        None => problems.push((key, line)),
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
            let key = node["owner"]
                .as_str()
                .filter(|o| !o.is_empty())
                .map(String::from)
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
                set(
                    &mut problems,
                    key.clone(),
                    format!(
                        "LOW CONTRAST {key}: {ratio:.1}:1 at t={:.0}ms, {color} on {background}, needs {}:1{estimated}",
                        frame.t,
                        if large { "3" } else { "4.5" }
                    ),
                );
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
