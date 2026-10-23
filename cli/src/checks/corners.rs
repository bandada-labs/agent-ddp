//! `check corners`: what reaches past an element's rounded corners.
use super::{pick, Point};
use crate::args::Args;
use crate::model::{area, contains, element_key, rect, Frame, Recording};
use crate::py::{self, f, truthy};
use crate::say;
use crate::state::{Result, OWNER_TOLERANCE_PT};
use serde_json::{json, Value};
use std::f64::consts::PI;

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
        return (0..8)
            .map(|i| {
                let a = i as f64 * PI / 4.0;
                (x + w / 2.0 + w / 2.0 * a.cos(), y + h / 2.0 + h / 2.0 * a.sin())
            })
            .collect();
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
        n["owner"].as_str() == Some(&key)
            && truthy(n.get("corner"))
            && (truthy(n.get("fill")) || truthy(n.get("border")))
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
        if std::ptr::eq(node, shape)
            || node["kind"] == "bitmap"
            || !center_inside
            || contains(rect(&node["frame"]), shape_rect, tolerance)
            || area(rect(&node["frame"])) > area(shape_rect) * 1.5
        {
            continue;
        }
        let clip = node.get("clipRound").filter(|c| truthy(Some(c))).and_then(Value::as_array);
        let outside: Vec<Point> = outline_points(node)
            .into_iter()
            .map(|p| clamp(p, r))
            .filter(|&p| {
                !inside_rounded(p, shape_rect, f(&shape["corner"]), tolerance)
                    && !clip.is_some_and(|c| {
                        !inside_rounded(p, [f(&c[0]), f(&c[1]), f(&c[2]), f(&c[3])], f(&c[4]), tolerance)
                    })
            })
            .collect();
        if let Some(&first) = outside.first() {
            breaks.push((node, first));
        }
    }
    breaks
}

pub fn check_corners(args: &Args, recording: &Recording) -> Result<usize> {
    let frames = if args.all_frames {
        &recording.frames[..]
    } else {
        &recording.frames[recording.frames.len().saturating_sub(1)..]
    };
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
            let shape = py::max_first(
                frame.nodes().iter().filter(|n| n["owner"].as_str() == Some(&key) && truthy(n.get("corner"))),
                |n| area(rect(&n["frame"])),
            )
            .unwrap();
            say!(
                "OUTSIDE CORNERS {key} at t={}ms: {} nodes reach past corner radius {} of {}",
                py::str(frame.t_value()),
                breaks.len(),
                py::str(&shape["corner"]),
                py::str(&shape["frame"])
            );
            for (node, point) in breaks.iter().take(5) {
                say!(
                    "  z={} at ({:.1}, {:.1}) {}",
                    py::str(&node["z"]),
                    point.0,
                    point.1,
                    py::dumps(&pick(node, &["kind", "op", "fill", "border", "frame", "points"]))
                );
            }
        }
    }
    if reported.is_empty() && !args.shared {
        say!("nothing outside rounded corners");
    }
    Ok(reported.len())
}
