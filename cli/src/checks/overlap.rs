//! `check overlap`: what paints over an element it does not own.
use super::pick;
use crate::args::Args;
use crate::model::{contains, element_key, paints_at, rect, Frame, Recording};
use crate::py::{self, f};
use crate::say;
use crate::state::Result;
use serde_json::Value;
use std::collections::HashSet;

/// Which of 9 sample points inside the element are painted over by something it does not own.
/// Nodes of elements inside this one (a button's own label) count as its own.
fn coverage<'a>(frame: &'a Frame, element: &Value, tolerance: f64) -> Option<(usize, Vec<&'a Value>)> {
    let key = element_key(element);
    let outer = rect(&element["frame"]);
    let inner_keys: HashSet<String> =
        frame.elements().iter().filter(|e| contains(outer, rect(&e["frame"]), tolerance)).map(element_key).collect();
    let own_top = frame
        .nodes()
        .iter()
        .filter(|n| n["owner"].as_str() == Some(&key))
        .map(|n| f(&n["z"]))
        .fold(None, |best: Option<f64>, z| Some(best.map_or(z, |b| b.max(z))))?;
    let [x, y, w, h] = outer;
    let mut covered = 0;
    let mut coverers: Vec<(f64, &Value)> = Vec::new();
    for fy in [0.2, 0.5, 0.8] {
        for fx in [0.2, 0.5, 0.8] {
            let point = (x + w * fx, y + h * fy);
            let above: Vec<&Value> = frame
                .nodes()
                .iter()
                .filter(|n| {
                    f(&n["z"]) > own_top
                        && !n["owner"].as_str().is_some_and(|o| inner_keys.contains(o))
                        && paints_at(n, point)
                })
                .collect();
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

pub fn check_overlap(args: &Args, recording: &Recording) -> Result<usize> {
    let frames = if args.all_frames {
        &recording.frames[..]
    } else {
        &recording.frames[recording.frames.len().saturating_sub(1)..]
    };
    let mut reported: Vec<String> = Vec::new();
    for frame in frames {
        for element in frame.elements() {
            let key = element_key(element);
            let Some((covered, coverers)) = coverage(frame, element, recording.tolerance) else { continue };
            if covered == 0 || reported.contains(&key) {
                continue;
            }
            reported.push(key.clone());
            say!(
                "COVERED {key} at t={}ms: {covered}/9 sample points, frame {}",
                py::str(frame.t_value()),
                py::str(&element["frame"])
            );
            for node in coverers {
                say!(
                    "  by z={} {}",
                    py::str(&node["z"]),
                    py::dumps(&pick(node, &["kind", "source", "fill", "frame", "owner"]))
                );
            }
        }
    }
    if reported.is_empty() && !args.shared {
        say!("no covered elements");
    }
    Ok(reported.len())
}
