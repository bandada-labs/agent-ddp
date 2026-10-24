//! Whether a touch reaches an element, from the app's own hit test (`screen.hit`). `tap` checks before it touches and
//! looks for a change after; `check tappable` reports every control on screen without touching anything.
use crate::args::Args;
use crate::device::touch;
use crate::live::{open, require_session, snapshot_live, snapshot_messages};
use crate::model::{element_key, element_state, from_lines, rect, Frame};
use crate::py::{self, f, truthy};
use crate::state::{failure, Result};
use crate::{fail, say};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long `tap` waits for an element to stop moving, become enabled or come out from under a cover.
const TAP_WAIT_MS: u64 = 2000;
/// How long `tap` looks for a change on screen after the touch.
const CHANGE_WAIT_MS: u64 = 1000;
const POLL_MS: u64 = 50;
const BUSY_SAMPLE_MS: u64 = 100;
/// Two frames closer than this (in the platform's unit) are the same place: the element is not moving.
const STILL: f64 = 0.5;

pub fn tap(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let key = args.id.clone().unwrap_or_default();
    let started = Instant::now();
    let before = snapshot_live(&session)?;
    let Some(element) = before.frames.last().unwrap().element(&key).cloned() else {
        fail!(format!("no element '{key}' on screen"), "unknown_element", "ddp tree -i");
    };
    let mut control = open(&session)?;
    if args.force {
        let [x, y, w, h] = rect(&element["frame"]);
        touch(&mut control, (x + w / 2.0, y + h / 2.0))?;
        control.close();
        say!("tapped {key} at ({:.0}, {:.0})", x + w / 2.0, y + h / 2.0);
        return Ok(0);
    }
    let (hit, moving) = wait_until_tappable(&mut control, &key, &element)?;
    let (x, y) = (f(&hit["point"][0]), f(&hit["point"][1]));
    // Two frames a little apart, so a clock that ticks every frame shows as busy.
    std::thread::sleep(Duration::from_millis(BUSY_SAMPLE_MS).saturating_sub(started.elapsed()));
    let settled = frame_now(&session)?;
    // What changes with no touch at all (a clock, a spinner) says nothing about the tap.
    let busy = changed_keys(before.frames.last().unwrap(), &settled);
    touch(&mut control, (x, y))?;
    control.close();
    let (changed, after) = wait_for_change(&session, &settled, &busy)?;
    let mut notes = Vec::new();
    if !text(&hit, "reason").is_empty() {
        notes.push(format!("its center is {}", detail(&hit)));
    }
    if moving {
        notes.push("it was still moving".to_string());
    }
    let outcome = if changed.is_empty() {
        // A slow action (a request) can still change the screen later; a second tap would run it twice.
        format!("nothing changed within {CHANGE_WAIT_MS}ms; look again before a second tap")
    } else {
        // A label is the name of an element without an id, so a new label reads as one element gone, one new.
        let (was, is) = (settled.keys(), after.keys());
        let group = |label: &str, keep: &dyn Fn(&String) -> bool| {
            let keys: Vec<&String> = changed.iter().filter(|k| keep(k)).take(5).collect();
            (!keys.is_empty())
                .then(|| format!("{label}: {}", keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")))
        };
        [
            group("changed", &|k| was.contains(k) && is.contains(k)),
            group("new", &|k| !was.contains(k)),
            group("gone", &|k| !is.contains(k)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ")
    };
    let notes = if notes.is_empty() { String::new() } else { format!(" ({})", notes.join(", ")) };
    say!("tapped {key} at ({x:.0}, {y:.0}){notes}; {outcome}");
    Ok(0)
}

/// Live: every control on screen, checked with the app's hit test. Nothing is touched.
pub fn check_tappable(args: &Args) -> Result<usize> {
    let session = require_session()?;
    let mut control = open(&session)?;
    let hits = control.call("screen.hit", json!({}))?;
    let hooks = crate::inspect::troubled_hooks(&mut control);
    control.close();
    crate::inspect::say_hooks(&hooks);
    let hits = hits["elements"].as_array().cloned().unwrap_or_default();
    let mut problems = 0;
    for hit in &hits {
        let key = element_key(hit);
        let line = match (text(hit, "reason"), hit["points"].as_u64().unwrap_or(0)) {
            ("", _) => continue,
            ("disabled", _) => format!("DISABLED {key}"),
            (reason, 0) => format!("{} {key}: {}, no point of it takes a touch", reason.to_uppercase(), detail(hit)),
            (_, points) => format!(
                "PARTLY {key}: its center is {}; {points}/9 points take a touch, ddp tap uses ({}, {})",
                detail(hit),
                py::str(&hit["point"][0]),
                py::str(&hit["point"][1])
            ),
        };
        problems += 1;
        say!("{line}");
    }
    if !args.shared {
        say!("{} of {} controls take a touch at their center", hits.len() - problems, hits.len());
    }
    Ok(problems)
}

/// Controls a touch does not reach now, keyed like the tree, with why. For `explore`, which skips them.
pub fn unreachable(session: &Value) -> Result<Vec<(String, String)>> {
    let mut control = open(session)?;
    let hits = control.call("screen.hit", json!({}))?;
    control.close();
    Ok(hits["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|hit| hit["reason"] == "disabled" || hit["points"].as_u64() == Some(0))
        .map(|hit| (element_key(hit), detail(hit)))
        .collect())
}

/// Why a touch misses, for a person: "covered by TapGesture", "outside its parent (UIView bounds)".
fn detail(hit: &Value) -> String {
    let by = text(hit, "by");
    match text(hit, "reason") {
        "disabled" => "disabled".into(),
        "covered" => format!("covered by {}", if by.is_empty() { "another view" } else { by }),
        "outside" => format!("outside its parent ({by})"),
        "intercepted" => format!("taken first by a parent's {by}"),
        _ if by.is_empty() => "taking no touch".into(),
        _ => format!("taking no touch ({by})"),
    }
}

/// Asks the app until the element holds still, is enabled and has a point a touch reaches, or the wait is over.
/// Returns the hit and whether the element was still moving when the wait ran out.
fn wait_until_tappable(control: &mut crate::control::Control, key: &str, element: &Value) -> Result<(Value, bool)> {
    let name = py::str(element.get("id").filter(|v| truthy(Some(*v))).unwrap_or(&element["label"]));
    let deadline = Instant::now() + Duration::from_millis(TAP_WAIT_MS);
    let mut last = rect(&element["frame"]);
    loop {
        let answer = control.call("screen.hit", json!({"id": name}))?;
        // Elements can share a name; the one nearest to where the element was is the one asked for.
        let Some(hit) = answer["elements"]
            .as_array()
            .and_then(|hits| py::min_first(hits.iter(), |h| distance(rect(&h["frame"]), last)).cloned())
        else {
            fail!(format!("no element '{key}' on screen"), "unknown_element", "ddp tree -i");
        };
        let now = rect(&hit["frame"]);
        let moving = distance(now, last) > STILL;
        last = now;
        let reachable = hit["reason"] != "disabled" && truthy(hit.get("point"));
        if reachable && !moving {
            return Ok((hit, false));
        }
        if Instant::now() >= deadline {
            if reachable {
                return Ok((hit, true));
            }
            let reason = text(&hit, "reason");
            let next = format!("ddp check tappable, or ddp tap {key} --force to touch it anyway");
            let message = if reason == "disabled" {
                format!("{key} is disabled")
            } else {
                format!("{key} is {}: no point of it takes a touch", detail(&hit))
            };
            return Err(failure(message, if reason.is_empty() { "untouchable" } else { reason }, Some(&next)));
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

/// A text field of the hit, empty when the app left it out.
fn text<'a>(hit: &'a Value, field: &str) -> &'a str {
    hit.get(field).and_then(Value::as_str).unwrap_or("")
}

fn distance(a: [f64; 4], b: [f64; 4]) -> f64 {
    a.iter().zip(b).map(|(p, q)| (p - q).abs()).fold(0.0, f64::max)
}

/// One frame of what is on screen, kept in memory: polling must not replace the last take on disk.
fn frame_now(session: &Value) -> Result<Frame> {
    let recording = from_lines(snapshot_messages(session)?, PathBuf::from("live"))?;
    Ok(recording.frames.into_iter().last().unwrap())
}

/// Elements that appeared, went away or changed state between two frames.
fn changed_keys(a: &Frame, b: &Frame) -> BTreeSet<String> {
    let keys: BTreeSet<String> = a.keys().into_iter().chain(b.keys()).collect();
    let state = |frame: &Frame, key: &str| element_state(frame, key).map(|s| py::dumps_sorted(&s));
    // "?" is an element with no name (the keyboard's keys): nothing an agent can act on.
    keys.into_iter().filter(|key| key != "?" && state(a, key) != state(b, key)).collect()
}

/// Polls until something other than the busy elements changes, or the wait is over: the changed keys and the frame.
fn wait_for_change(session: &Value, settled: &Frame, busy: &BTreeSet<String>) -> Result<(Vec<String>, Frame)> {
    let deadline = Instant::now() + Duration::from_millis(CHANGE_WAIT_MS);
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        let now = frame_now(session)?;
        let changed: Vec<String> = changed_keys(settled, &now).difference(busy).cloned().collect();
        if !changed.is_empty() || Instant::now() >= deadline {
            return Ok((changed, now));
        }
    }
}
