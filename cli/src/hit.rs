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
/// After the first change, the screen is settled when this many polls in a row bring nothing new.
const QUIET_POLLS: u32 = 3;
/// The longest `tap` waits for the screen to settle.
const SETTLE_WAIT_MS: u64 = 3000;
const BUSY_SAMPLE_MS: u64 = 100;
/// Two frames closer than this (in the platform's unit) are the same place: the element is not moving.
const STILL: f64 = 0.5;

pub fn tap(args: &mut Args) -> Result<i32> {
    if crate::runner::device_mode(args) {
        return crate::runner::gesture(args);
    }
    let session = require_session()?;
    let key = args.id.clone().unwrap_or_default();
    if let Some(at) = point(&key) {
        return tap_point(&session, at);
    }
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
    let (changed, after, still_changing) = wait_for_change(&session, &settled, &busy)?;
    let mut notes = Vec::new();
    if !text(&hit, "reason").is_empty() {
        notes.push(format!("its center is {}", detail(&hit)));
    }
    if moving {
        notes.push("it was still moving".to_string());
    }
    let (outcome, new) = outcome(&settled, &after, &changed, still_changing)?;
    let shown = json!({"appeared": appeared(&session, &settled, &after, &busy)?});
    note(&py::str(&session["platform"]), vec![(key.clone(), (x, y), Some(seen(&element)))], &new, shown);
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
pub fn frame_now(session: &Value) -> Result<Frame> {
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

/// Polls until the screen settles: the first change other than the busy elements within 1 s, then until three polls
/// in a row bring nothing new, at most 3 s. The changed keys against `settled`, the last frame, whether it was still
/// changing when the wait ran out.
fn wait_for_change(session: &Value, settled: &Frame, busy: &BTreeSet<String>) -> Result<(Vec<String>, Frame, bool)> {
    let started = Instant::now();
    let mut last: Option<Frame> = None;
    let mut quiet = 0;
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        let now = frame_now(session)?;
        let changed_since = |frame: &Frame| changed_keys(frame, &now).difference(busy).count() > 0;
        match &last {
            None if !changed_since(settled) => {
                if started.elapsed() >= Duration::from_millis(CHANGE_WAIT_MS) {
                    return Ok((Vec::new(), now, false));
                }
            }
            Some(previous) if !changed_since(previous) => {
                quiet += 1;
                if quiet >= QUIET_POLLS {
                    let changed = changed_keys(settled, &now).difference(busy).cloned().collect();
                    return Ok((changed, now, false));
                }
            }
            _ => {
                quiet = 0;
                if started.elapsed() >= Duration::from_millis(SETTLE_WAIT_MS) {
                    let changed = changed_keys(settled, &now).difference(busy).cloned().collect();
                    return Ok((changed, now, true));
                }
                last = Some(now);
            }
        }
    }
}

/// The change as the agent reads it: changed, new and gone elements, each with the `@eN` to act on next.
fn outcome(settled: &Frame, after: &Frame, changed: &[String], still_changing: bool) -> Result<(String, Vec<String>)> {
    if changed.is_empty() {
        return Ok((format!("nothing changed within {CHANGE_WAIT_MS}ms; look again before a second tap"), Vec::new()));
    }
    let refs = crate::runner::refs_for(changed)?;
    let (was, is) = (settled.keys(), after.keys());
    let named = |key: &String| refs.get(key).map(|r| format!("{r} {key}")).unwrap_or_else(|| key.clone());
    let group = |label: &str, keep: &dyn Fn(&String) -> bool| {
        let keys: Vec<&String> = changed.iter().filter(|k| keep(k)).collect();
        (!keys.is_empty()).then(|| {
            let mut shown: Vec<String> = keys.iter().take(5).map(|k| named(k)).collect();
            if keys.len() > 5 {
                shown.push(format!("+{}", keys.len() - 5));
            }
            format!("{label}: {}", shown.join(", "))
        })
    };
    // A label is the name of an element without an id, so a new label reads as one element gone, one new.
    let mut parts: Vec<String> = [
        group("changed", &|k| was.contains(k) && is.contains(k)),
        group("new", &|k| !was.contains(k)),
        group("gone", &|k| !is.contains(k)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if still_changing {
        parts.push("still changing".into());
    }
    let new = changed.iter().filter(|k| !was.contains(k)).cloned().collect();
    Ok((parts.join("; "), new))
}

/// What a walk keeps of an element: enough to find it again on another run.
pub fn seen(element: &Value) -> Value {
    json!({"key": element_key(element), "id": element.get("id"), "label": element.get("label"), "frame": element["frame"]})
}

/// The smallest element under a point: what a finger there lands on.
fn under(frame: &Frame, (x, y): (f64, f64)) -> Option<Value> {
    let area = |e: &Value| {
        let [_, _, w, h] = rect(&e["frame"]);
        w * h
    };
    frame
        .elements()
        .iter()
        .filter(|e| {
            let [ex, ey, w, h] = rect(&e["frame"]);
            x >= ex && x <= ex + w && y >= ey && y <= ey + h
        })
        .min_by(|a, b| area(a).total_cmp(&area(b)))
        .map(seen)
}

/// A walk step with the library: the places as given with what was there, and the keys that changed.
/// Labels on screen after the action that were not before it, whatever element holds them (see walk parity).
/// During a replay, labels must still be there one more look later (see `runner::confirmed`).
fn appeared(session: &Value, settled: &Frame, after: &Frame, busy: &BTreeSet<String>) -> Result<Vec<String>> {
    let labels = |frame: &Frame| -> Vec<String> {
        frame
            .elements()
            .iter()
            .filter(|e| !busy.contains(&element_key(e)))
            .filter_map(|e| e["label"].as_str().filter(|l| !l.is_empty()).map(String::from))
            .collect()
    };
    let was = labels(settled);
    let appeared: Vec<String> = labels(after).into_iter().filter(|l| !was.contains(l)).collect();
    if !crate::walk::replaying() || appeared.is_empty() {
        return Ok(appeared);
    }
    std::thread::sleep(crate::runner::CONFIRM_LOOK);
    let later = labels(&frame_now(session)?);
    Ok(appeared.into_iter().filter(|l| later.contains(l)).collect())
}

pub fn note(platform: &str, places: Vec<(String, (f64, f64), Option<Value>)>, new: &[String], given: Value) {
    let mut note = json!({
        "mode": "sdk",
        "platform": platform,
        "places": places.into_iter().map(|(given, at, seen)| json!({"given": given, "at": [at.0, at.1], "seen": seen})).collect::<Vec<_>>(),
        "new": new,
    });
    if let (Some(note), Some(given)) = (note.as_object_mut(), given.as_object()) {
        note.extend(given.clone());
    }
    crate::walk::note(note);
}

/// A point has no element to ask the hit test about: touch it, then say what changed.
fn tap_point(session: &Value, at: (f64, f64)) -> Result<i32> {
    let before = frame_now(session)?;
    std::thread::sleep(Duration::from_millis(BUSY_SAMPLE_MS));
    let settled = frame_now(session)?;
    let busy = changed_keys(&before, &settled);
    let mut control = open(session)?;
    touch(&mut control, at)?;
    control.close();
    let (changed, after, still_changing) = wait_for_change(session, &settled, &busy)?;
    let (outcome, new) = outcome(&settled, &after, &changed, still_changing)?;
    note(
        &py::str(&session["platform"]),
        vec![(format!("{},{}", at.0, at.1), at, under(&settled, at))],
        &new,
        json!({"appeared": appeared(session, &settled, &after, &busy)?}),
    );
    say!("tapped ({:.0}, {:.0}); {outcome}", at.0, at.1);
    Ok(0)
}

/// Moves in a swipe or a drag, spread over its time: enough for a gesture recognizer to see a velocity.
const GESTURE_STEPS: u32 = 12;
/// A drag holds still this long first, so the app sees a long press that picks the item up.
pub const DRAG_HOLD_MS: u64 = 500;
/// The pause between the two taps of a double tap, under both platforms' double-tap timeout (300 ms).
pub const DOUBLE_TAP_GAP_MS: u64 = 100;

/// `swipe`, `drag`, `double-tap`, `long-press`: one finger on `input.touch`, the real finger the libraries give, then
/// what changed. A place is an element (its center) or `x,y` in the frames' unit (pt on iOS, px on Android).
pub fn gesture(args: &mut Args) -> Result<i32> {
    if crate::runner::device_mode(args) {
        return crate::runner::gesture(args);
    }
    let session = require_session()?;
    let gesture = args.command.clone();
    let from_text = args.id.clone().unwrap_or_default();
    let before = snapshot_live(&session)?;
    let frame = before.frames.last().unwrap();
    let from = place(frame, &from_text)?;
    let to = match (&gesture[..], &args.value) {
        ("swipe" | "drag", Some(text)) => Some(place(frame, &crate::queries::resolve_ref(text)?)?),
        ("swipe" | "drag", None) => {
            fail!(format!("{gesture} needs where to: FROM TO"), "missing_option", format!("ddp {gesture} list 0,400"))
        }
        _ => None,
    };
    let default_ms = match &gesture[..] {
        "swipe" => 300.0,
        "drag" => 600.0,
        _ => 800.0,
    };
    let ms = args.duration.as_deref().map(crate::state::parse_ms).transpose()?.unwrap_or(default_ms);
    // Two frames a little apart, so a clock that ticks every frame shows as busy.
    std::thread::sleep(Duration::from_millis(BUSY_SAMPLE_MS));
    let settled = frame_now(&session)?;
    let busy = changed_keys(frame, &settled);
    let mut control = open(&session)?;
    let mut finger = |phase: &str, (x, y): (f64, f64)| {
        control.call("input.touch", json!({"phase": phase, "x": x, "y": y})).map(|_| ())
    };
    let pause = |ms: f64| std::thread::sleep(Duration::from_millis(ms.max(0.0) as u64));
    match (&gesture[..], to) {
        ("double-tap", _) => {
            for second in [false, true] {
                if second {
                    pause(DOUBLE_TAP_GAP_MS as f64);
                }
                finger("down", from)?;
                finger("up", from)?;
            }
        }
        ("long-press", _) => {
            finger("down", from)?;
            pause(ms);
            finger("up", from)?;
        }
        (_, Some(to)) => {
            finger("down", from)?;
            if gesture == "drag" {
                pause(DRAG_HOLD_MS as f64);
            }
            for step in 1..=GESTURE_STEPS {
                pause(ms / GESTURE_STEPS as f64);
                let share = step as f64 / GESTURE_STEPS as f64;
                finger("move", (from.0 + (to.0 - from.0) * share, from.1 + (to.1 - from.1) * share))?;
            }
            finger("up", to)?;
        }
        _ => unreachable!("swipe and drag have a place to go"),
    }
    control.close();
    let (changed, after, still_changing) = wait_for_change(&session, &settled, &busy)?;
    let (outcome, new) = outcome(&settled, &after, &changed, still_changing)?;
    let outcome = outcome.replace("; look again before a second tap", "");
    let to_text = args.value.clone().map(|text| crate::queries::resolve_ref(&text)).transpose()?;
    let mut places = vec![(from_text.clone(), from, place_seen(frame, &from_text, from))];
    if let (Some(text), Some(to)) = (&to_text, to) {
        places.push((text.clone(), to, place_seen(frame, text, to)));
    }
    let given = if gesture == "double-tap" {
        json!({"appeared": appeared(&session, &settled, &after, &busy)?})
    } else {
        json!({"appeared": appeared(&session, &settled, &after, &busy)?, "ms_given": args.duration.as_deref().map(crate::state::parse_ms).transpose()?})
    };
    note(&py::str(&session["platform"]), places, &new, given);
    let places = match to {
        Some(to) => format!("({:.0}, {:.0}) -> ({:.0}, {:.0})", from.0, from.1, to.0, to.1),
        None => format!("({:.0}, {:.0})", from.0, from.1),
    };
    say!("{gesture} {from_text} {places}; {outcome}");
    Ok(0)
}

/// A box over the live app for a while, for a person watching the phone (`screen.highlight`).
pub fn highlight(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let key = args.id.clone().unwrap_or_default();
    let recording = snapshot_live(&session)?;
    let Some(element) = recording.frames.last().unwrap().element(&key).cloned() else {
        fail!(format!("no element '{key}' on screen"), "unknown_element", "ddp tree -i");
    };
    let ms = args.duration.as_deref().map(crate::state::parse_ms).transpose()?.unwrap_or(3000.0);
    open(&session)?.call("screen.highlight", json!({"rect": element["frame"], "ms": ms}))?;
    say!("highlighted {key} {} for {}ms", py::str(&element["frame"]), ms as u64);
    Ok(0)
}

/// What was at a place a gesture was given: the element named, or the one under the point.
fn place_seen(frame: &Frame, text: &str, at: (f64, f64)) -> Option<Value> {
    match point(text) {
        Some(_) => under(frame, at),
        None => frame.element(text).map(seen),
    }
}

/// An element's center, or `x,y`.
fn place(frame: &Frame, text: &str) -> Result<(f64, f64)> {
    if let Some(point) = point(text) {
        return Ok(point);
    }
    let Some(element) = frame.element(text) else {
        fail!(format!("no element '{text}' on screen"), "unknown_element", "ddp tree -i, or give x,y");
    };
    let [x, y, w, h] = rect(&element["frame"]);
    Ok((x + w / 2.0, y + h / 2.0))
}

/// `120,300` as a point; anything else is not one.
pub fn point(text: &str) -> Option<(f64, f64)> {
    let (x, y) = text.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}
