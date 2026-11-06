//! Apps without the library (`--device`). Android: a runner (android/runner, an instrumentation inside the ddp binary)
//! serves the control protocol on the device, touches go in as the system's own input and elements come from the
//! platform's accessibility tree. iOS: the simulator's own HID and accessibility services, from the Mac
//! (simulator.rs). Neither knows the app's engine, so ddp says what it did and checks nothing.
use crate::args::Args;
use crate::control::Control;
use crate::model::element_key;
use crate::py;
use crate::state::{adb_path, capture, ctx, failure, read_json, Result};
use crate::{fail, say};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use crate::simulator::{Simulator, BACKSPACE, END, ENTER, MOVE, START, TAB};
#[cfg(not(target_os = "macos"))]
use mac_only::{Simulator, BACKSPACE, END, ENTER, MOVE, START, TAB};

const SOCKET: &str = "localabstract:agentddp.runner.control";
const PACKAGE: &str = "dev.agentddp.runner";
const APK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runner.apk"));
/// `am instrument` to the socket open took 2 s on an emulator; a cold device can take longer.
const START_PATIENCE: Duration = Duration::from_secs(10);
/// The runner's `VERSION` (Runner.kt) this ddp speaks.
const RUNNER_VERSION: u64 = 4;
const POLL: Duration = Duration::from_millis(100);

/// What every runner answer ends with: the runner did it, nothing was checked.
pub const NO_CHECKS: &str = "no checks (no SDK)";

/// A connection to the runner on this Android device, started (and installed) when it is not running. A runner
/// another ddp left running answers differently: it is stopped and replaced by the one this ddp carries.
pub fn open(device: &str) -> Result<Control> {
    let session = json!({"device": device, "app": PACKAGE, "port": forward(device)?});
    if let Ok(mut control) = Control::open(&session) {
        if control.call("app.info", json!({}))?["runner"] == RUNNER_VERSION {
            return Ok(control);
        }
        control.call("runner.stop", json!({}))?;
        control.close();
        stop(device)?;
    }
    start(device)?;
    let deadline = Instant::now() + START_PATIENCE;
    loop {
        match Control::open(&session) {
            Ok(control) => return Ok(control),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(POLL),
        }
    }
}

/// Ends ddp's runner on this device. It holds UiAutomation, which one process at a time may have: the app's own UI
/// tests on the device fail with "UiAutomationService already registered" while it runs.
/// Says whether it was running.
pub fn stop(device: &str) -> Result<bool> {
    let running = adb(device, &["shell", "pidof", PACKAGE])?.code == 0;
    adb(device, &["shell", "am", "force-stop", PACKAGE])?;
    Ok(running)
}

fn adb(device: &str, command: &[&str]) -> Result<crate::state::Output> {
    let mut all = vec!["-s", device];
    all.extend_from_slice(command);
    Ok(capture(&adb_path()?, &all))
}

/// The host port forwarded to the runner's socket, reusing one ddp made before.
fn forward(device: &str) -> Result<u16> {
    let listed = adb(device, &["forward", "--list"])?.stdout;
    let existing = listed.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        (fields.len() == 3 && fields[0] == device && fields[2] == SOCKET)
            .then(|| fields[1].strip_prefix("tcp:")?.parse().ok())
            .flatten()
    });
    if let Some(port) = existing {
        return Ok(port);
    }
    let made = adb(device, &["forward", "tcp:0", SOCKET])?;
    match made.stdout.trim().parse() {
        Ok(port) if made.code == 0 => Ok(port),
        _ => fail!(format!("adb forward failed on {device}: {}", made.stderr.trim()), "command_failed", "adb devices"),
    }
}

/// Installs the runner this ddp carries (so an old one from another ddp is replaced) and starts it in the background:
/// `-w` keeps `am` alive, and UiAutomation's connection lives in `am`.
fn start(device: &str) -> Result<()> {
    if APK.is_empty() {
        fail!(
            "this ddp was built without the Android runner",
            "not_supported",
            "cd android && ./gradlew :runner:assembleDebug, then build the CLI again"
        );
    }
    let path = std::env::temp_dir().join(format!("ddp-runner-{}.apk", std::process::id()));
    std::fs::write(&path, APK).map_err(|e| failure(format!("can't write the runner APK: {e}"), "io_error", None))?;
    let installed = adb(device, &["install", "-r", "-t", &path.to_string_lossy()]);
    let _ = std::fs::remove_file(&path);
    let installed = installed?;
    if installed.code != 0 {
        fail!(format!("installing the runner failed: {}", installed.stderr.trim()), "command_failed", "adb devices");
    }
    let started = adb(device, &["shell", &format!("nohup am instrument -w {PACKAGE}/.Runner >/dev/null 2>&1 &")])?;
    if started.code != 0 {
        fail!(format!("starting the runner failed: {}", started.stderr.trim()), "command_failed", "adb devices");
    }
    Ok(())
}

/// Between two looks at the screen while it settles. A look costs 16 ms (Android) to 40 ms (iOS).
const LOOK_EVERY: Duration = Duration::from_millis(100);
/// Two looks this far apart before the action show what changes by itself (a clock, a spinner).
const BUSY_SAMPLE: Duration = Duration::from_millis(100);
/// How long a change may take to start; after it, how long the screen may keep changing.
const FIRST_CHANGE_WAIT: Duration = Duration::from_millis(1000);
const SETTLE_WAIT: Duration = Duration::from_millis(3000);
/// The screen is settled when this many looks in a row found nothing new.
const QUIET_LOOKS: u32 = 2;
/// One move per display frame, so scroll views and flings see a finger's speed.
const FRAME: Duration = Duration::from_millis(16);
/// The field takes the focus on a later turn of the app's main loop; keys before that go nowhere.
const FOCUS_WAIT: Duration = Duration::from_millis(200);
/// How long after a replayed action ddp looks again before it keeps what appeared.
pub const CONFIRM_LOOK: Duration = Duration::from_millis(500);
/// Elements listed per group in an action's line; the rest are counted.
const SHOWN: usize = 5;
/// How long an action waits for its element to show, as `tap` with the library does: a menu or sheet still opening
/// rebuilds its part of the tree, and an element listed a moment ago is missing for a few looks.
const ELEMENT_WAIT: Duration = Duration::from_millis(2000);

/// Whether this command drives an app without the library: `--device`, or a session `ddp up` made for one.
pub fn device_mode(args: &Args) -> bool {
    args.file.is_none()
        && (args.device.is_some() || read_json(&ctx().session_file).is_some_and(|s| s["mode"] == "device"))
}

/// One app without the library, held for one command.
enum Screen {
    Android(Control),
    Ios(Simulator),
}

impl Screen {
    fn open(platform: &str, device: &str) -> Result<Screen> {
        Ok(if platform == "ios" { Screen::Ios(Simulator::open(device)?) } else { Screen::Android(open(device)?) })
    }

    /// What is on screen: `type`, `label`, `id`, `value`, `frame`, and `tap`/`enabled` where the platform says.
    /// Named as the library names them: elements that share a label are `Delete`, `Delete [2]` in reading order, so
    /// a ref or a name points at one element, never at the first of several.
    /// What a person sees: the elements on screen.
    fn elements(&mut self) -> Result<Vec<Value>> {
        Ok(on_screen(&self.look()?))
    }

    /// Every element the platform lists, with the ones the app holds off screen (Android: `shown: false`), so an
    /// action can tell "scrolled out of view" from "gone". Names come from all of them, so a scroll keeps them.
    fn look(&mut self) -> Result<Vec<Value>> {
        let elements = match self {
            Screen::Ios(simulator) => simulator.elements()?,
            Screen::Android(control) => {
                control.call("screen.elements", json!({}))?["elements"].as_array().cloned().unwrap_or_default()
            }
        };
        let mut frame = json!({"elements": elements});
        crate::model::name_elements(&mut frame);
        Ok(frame["elements"].as_array().cloned().unwrap_or_default())
    }

    fn android(&mut self, method: &str, params: Value) -> Result<()> {
        if let Screen::Android(control) = self {
            control.call(method, params)?;
        }
        Ok(())
    }

    fn act(&mut self, action: &Action) -> Result<()> {
        let point = |(x, y): (f64, f64)| json!({"x": x, "y": y});
        if let Screen::Ios(simulator) = self {
            return ios_act(simulator, action);
        }
        match action {
            Action::Tap(at) => self.android("input.tap", point(*at)),
            Action::DoubleTap(at) => self.android("input.doubleTap", point(*at)),
            Action::LongPress(at, ms) => {
                let mut params = point(*at);
                params["ms"] = json!(ms);
                self.android("input.longPress", params)
            }
            Action::Drag { from, to, ms, hold } => {
                self.android("input.drag", json!({"from": point(*from), "to": point(*to), "ms": ms, "hold": hold}))
            }
            Action::Type(text) => self.android("input.type", json!({"text": text})),
            Action::Key(key) => self.android("input.key", json!({"key": key})),
        }
    }
}

fn pause(duration: Duration) {
    std::thread::sleep(duration);
}

fn ios_act(simulator: &mut Simulator, action: &Action) -> Result<()> {
    let tap = |simulator: &mut Simulator, at: (f64, f64)| -> Result<()> {
        simulator.finger(START, at)?;
        simulator.finger(END, at)
    };
    match action {
        Action::Tap(at) => tap(simulator, *at)?,
        Action::DoubleTap(at) => {
            tap(simulator, *at)?;
            pause(Duration::from_millis(crate::hit::DOUBLE_TAP_GAP_MS));
            tap(simulator, *at)?;
        }
        Action::LongPress(at, ms) => {
            simulator.finger(START, *at)?;
            pause(Duration::from_millis(*ms as u64));
            simulator.finger(END, *at)?;
        }
        Action::Drag { from, to, ms, hold } => {
            simulator.finger(START, *from)?;
            pause(Duration::from_millis(*hold));
            let steps = (*ms / FRAME.as_millis() as f64).round().max(1.0) as u32;
            for step in 1..=steps {
                pause(FRAME);
                let share = step as f64 / steps as f64;
                simulator.finger(MOVE, (from.0 + (to.0 - from.0) * share, from.1 + (to.1 - from.1) * share))?;
            }
            simulator.finger(END, *to)?;
        }
        Action::Type(text) => return simulator.type_text(text),
        Action::Key(key) => {
            let usage = match key.as_str() {
                "Enter" => ENTER,
                "Tab" => TAB,
                "Backspace" => BACKSPACE,
                _ => fail!(format!("no key {key} on iOS"), "bad_value"),
            };
            simulator.key(usage, true)?;
            simulator.key(usage, false)?;
        }
    }
    simulator.delivered()
}

/// What one command does to the app, in the platform's unit (points on iOS, pixels on Android).
enum Action {
    Tap((f64, f64)),
    DoubleTap((f64, f64)),
    LongPress((f64, f64), f64),
    Drag { from: (f64, f64), to: (f64, f64), ms: f64, hold: u64 },
    Type(String),
    Key(String),
}

/// A place as the agent gave it, where it is now, and the element seen there: a walk keeps what was seen, so a
/// replay finds the element again instead of touching the same pixels.
struct Place {
    given: String,
    at: (f64, f64),
    seen: Option<Value>,
}

fn center(element: &Value) -> (f64, f64) {
    let [x, y, w, h] = crate::model::rect(&element["frame"]);
    (x + w / 2.0, y + h / 2.0)
}

/// `x,y`, or the first element whose label or id is `text` (reading order, as a person scans the screen).
fn place(elements: &[Value], text: &str, from_ref: bool, device: &str) -> Result<Place> {
    if let Some(at) = crate::hit::point(text) {
        // The smallest element under the point is what the finger lands on.
        let seen = elements
            .iter()
            .filter(|e| {
                let [x, y, w, h] = crate::model::rect(&e["frame"]);
                at.0 >= x && at.0 <= x + w && at.1 >= y && at.1 <= y + h
            })
            .min_by(|a, b| area(a).total_cmp(&area(b)))
            .cloned();
        return Ok(Place { given: text.to_string(), at, seen });
    }
    // A ref or a numbered name (`Delete [2]`) names one element. A plain label or id may name several (a tab and
    // the bar titled like it): then no guess, a touch on the wrong one would read as the app's doing.
    let exact = from_ref || text.ends_with(']');
    let named = elements.iter().find(|e| element_key(e) == text).filter(|_| exact);
    let candidates: Vec<&Value> = match named {
        Some(element) => vec![element],
        None => elements.iter().filter(|e| element_key(e) == text || e["label"] == text || e["id"] == text).collect(),
    };
    // Off screen (scrolled away, below the fold): a touch at its middle lands on nothing, and "nothing changed"
    // would read as the app's answer. The agent scrolls first.
    let (candidates, off): (Vec<&Value>, Vec<&Value>) = candidates.into_iter().partition(|e| !off_screen(e));
    match candidates[..] {
        [element] => Ok(Place { given: text.to_string(), at: center(element), seen: Some(element.clone()) }),
        [] if !off.is_empty() => fail!(
            format!("'{text}' is off screen at {}: swipe it into view first", bracket(&frame_of(off[0]))),
            "off_screen",
            format!("ddp swipe <from> <to> --device {device}, then ddp tree --device {device}")
        ),
        [] => fail!(
            format!("no element '{text}' in the accessibility tree (not in it: what the iOS keyboard covers, an Android Compose testTag without testTagsAsResourceId)"),
            "unknown_element",
            format!("ddp tree --device {device}")
        ),
        _ => {
            let refs = refs_for(&candidates.iter().map(|e| element_key(e)).collect::<Vec<_>>())?;
            let listed: Vec<String> = candidates.iter().map(|element| tree_line(element, &refs)).collect();
            fail!(
                format!("'{text}' names {} elements: {}", candidates.len(), listed.join(", ")),
                "ambiguous_element",
                "act on one by its ref"
            )
        }
    }
}

fn area(element: &Value) -> f64 {
    let [_, _, w, h] = crate::model::rect(&element["frame"]);
    w * h
}

/// Element key (id, else label) to what the screen shows of it; elements with neither are left out.
fn keyed(elements: &[Value]) -> BTreeMap<String, &Value> {
    let mut keyed = BTreeMap::new();
    for element in elements {
        let key = element_key(element);
        if key != "?" {
            keyed.entry(key).or_insert(element);
        }
    }
    keyed
}

/// What the agent reads about an element: label, value, enabled. Its place is compared apart (a scroll moves all).
fn state(element: &Value) -> (String, String, bool) {
    (py::str(&element["label"]), py::str(&element["value"]), element["enabled"] != false)
}

fn frame_of(element: &Value) -> Vec<i64> {
    crate::model::rect(&element["frame"]).iter().map(|v| v.round() as i64).collect()
}

/// Keys whose element appeared, went away, changed or moved between two looks.
fn differing(a: &[Value], b: &[Value]) -> HashSet<String> {
    let (a, b) = (keyed(a), keyed(b));
    let keys: HashSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|key| match (a.get(*key), b.get(*key)) {
            (Some(x), Some(y)) => state(x) != state(y) || frame_of(x) != frame_of(y),
            _ => true,
        })
        .cloned()
        .collect()
}

/// What an action changed on screen, by element key, leaving out what changes by itself.
#[derive(Default)]
pub struct Change {
    pub new: Vec<Value>,
    pub gone: Vec<Value>,
    pub changed: Vec<Value>,
    pub moved: usize,
    pub still_changing: bool,
}

fn off_screen(element: &Value) -> bool {
    element["shown"] == false
}

fn on_screen(elements: &[Value]) -> Vec<Value> {
    elements.iter().filter(|e| !off_screen(e)).cloned().collect()
}

fn change(before: &[Value], after: &[Value], busy: &HashSet<String>) -> Change {
    let (was, is) = (keyed(before), keyed(after));
    let mut change = Change::default();
    for (key, element) in &is {
        if busy.contains(key) {
            continue;
        }
        match was.get(key) {
            // Off screen, after and maybe before: nothing a person sees.
            None if off_screen(element) => {}
            Some(old) if off_screen(old) && off_screen(element) => {}
            // Scrolled into or out of view: the app still holds it, as the library would say.
            Some(old) if off_screen(old) != off_screen(element) => change.moved += 1,
            None => change.new.push((*element).clone()),
            Some(old) if state(old) != state(element) => change.changed.push((*element).clone()),
            Some(old) if frame_of(old) != frame_of(element) => change.moved += 1,
            Some(_) => {}
        }
    }
    change.gone = was
        .iter()
        .filter(|(key, element)| !busy.contains(*key) && !is.contains_key(*key) && !off_screen(element))
        .map(|(_, e)| (*e).clone())
        .collect();
    // An id that shows up or goes away between looks (SwiftUI names tab buttons a moment after launch) changes the
    // key, not what a person sees: the same label, type and place on both sides is one element.
    let same_place = |element: &Value, others: &[Value]| {
        others
            .iter()
            .any(|o| o["label"] == element["label"] && o["type"] == element["type"] && frame_of(o) == frame_of(element))
    };
    let (new, gone) = (change.new.clone(), change.gone.clone());
    change.new.retain(|element| !same_place(element, &gone));
    change.gone.retain(|element| !same_place(element, &new));
    change
}

/// Looks until the screen stops changing: the first change within 1 s, then two quiet looks, at most 3 s.
fn settle(screen: &mut Screen, before: &[Value], busy: &HashSet<String>) -> Result<(Vec<Value>, bool)> {
    let started = Instant::now();
    let mut last: Option<Vec<Value>> = None;
    let mut quiet = 0;
    loop {
        pause(LOOK_EVERY);
        let now = screen.look()?;
        match &last {
            None if differing(before, &now).is_subset(busy) => {
                if started.elapsed() >= FIRST_CHANGE_WAIT {
                    return Ok((now, false));
                }
            }
            Some(previous) if differing(previous, &now).is_subset(busy) => {
                quiet += 1;
                if quiet >= QUIET_LOOKS {
                    return Ok((now, false));
                }
            }
            _ => {
                quiet = 0;
                last = Some(now);
                if started.elapsed() >= SETTLE_WAIT {
                    return Ok((last.unwrap(), true));
                }
            }
        }
    }
}

/// `@eN` for each key, keeping every number already given: an agent may still hold a ref from an earlier line, and
/// renumbering would send its next touch somewhere else.
pub fn refs_for(keys: &[String]) -> Result<BTreeMap<String, String>> {
    let path = ctx().dir.join("refs.json");
    let mut refs = read_json(&path).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut by_key: BTreeMap<String, String> = refs.iter().map(|(r, k)| (py::str(k), r.clone())).collect();
    let mut next = refs.keys().filter_map(|r| r.strip_prefix("@e")?.parse::<u64>().ok()).max().unwrap_or(0);
    for key in keys {
        if !by_key.contains_key(key) {
            next += 1;
            let reference = format!("@e{next}");
            refs.insert(reference.clone(), json!(key));
            by_key.insert(key.clone(), reference);
        }
    }
    crate::state::write_file(&path, py::dumps(&Value::Object(refs)))?;
    Ok(by_key)
}

/// `@e7 "Saved"` or `@e7 #save`: the ref to act on and what a person sees.
fn named(element: &Value, refs: &BTreeMap<String, String>) -> String {
    let key = element_key(element);
    let shown = match element["label"].as_str() {
        Some(label) => py::dumps(&json!(label)),
        None => format!("#{key}"),
    };
    match refs.get(&key) {
        Some(reference) => format!("{reference} {shown}"),
        None => shown,
    }
}

/// The change as one line: new, gone, changed (with its value), moved; or that nothing changed.
pub fn describe(change: &Change, refs: &BTreeMap<String, String>) -> String {
    let group = |name: &str, elements: &[Value], with_value: bool| {
        (!elements.is_empty()).then(|| {
            let mut shown: Vec<String> = elements
                .iter()
                .take(SHOWN)
                .map(|e| {
                    let value =
                        e["value"].as_str().filter(|_| with_value).map(|v| format!(" = {}", py::dumps(&json!(v))));
                    format!("{}{}", named(e, refs), value.unwrap_or_default())
                })
                .collect();
            if elements.len() > SHOWN {
                shown.push(format!("+{}", elements.len() - SHOWN));
            }
            format!("{name}: {}", shown.join(", "))
        })
    };
    let mut parts: Vec<String> =
        [group("new", &change.new, true), group("gone", &change.gone, false), group("changed", &change.changed, true)]
            .into_iter()
            .flatten()
            .collect();
    if change.moved > 0 {
        parts.push(format!("{} moved", change.moved));
    }
    if parts.is_empty() {
        return format!("nothing changed within {}ms", FIRST_CHANGE_WAIT.as_millis());
    }
    if change.still_changing {
        parts.push("still changing".into());
    }
    parts.join("; ")
}

/// An element as a person names it: its label, else `#id`. Walks keep this, so parity compares across platforms.
pub fn display(element: &Value) -> String {
    match element["label"].as_str() {
        Some(label) => label.to_string(),
        None => format!("#{}", element_key(element)),
    }
}

/// Labels on screen after an action that were not before it, whatever element holds them: what a person sees appear.
/// Parity compares these, since one platform keys an element by id and the other by label.
fn appeared(before: &[Value], after: &[Value], busy: &HashSet<String>) -> Vec<String> {
    let labels = |elements: &[Value]| -> Vec<String> {
        elements
            .iter()
            .filter(|e| !busy.contains(&element_key(e)))
            .filter_map(|e| e["label"].as_str().map(String::from))
            .collect()
    };
    let was = labels(before);
    let mut shown: Vec<String> = labels(after).into_iter().filter(|l| !was.contains(l)).collect();
    shown.dedup();
    shown
}

/// During a replay, one more look: a label that came and went (a toast) or changes by itself (a countdown) between
/// two runs would read as a difference between platforms. Only labels still there are kept.
fn confirmed(screen: &mut Screen, appeared: Vec<String>) -> Result<Vec<String>> {
    if !crate::walk::replaying() || appeared.is_empty() {
        return Ok(appeared);
    }
    pause(CONFIRM_LOOK);
    let later: Vec<String> = screen.elements()?.iter().filter_map(|e| e["label"].as_str().map(String::from)).collect();
    Ok(appeared.into_iter().filter(|label| later.contains(label)).collect())
}

/// The elements on screen now, for walks: the platform's accessibility tree.
pub fn elements_now(platform: &str, device: &str) -> Result<Vec<Value>> {
    Screen::open(platform, device)?.elements()
}

/// The keys of every element a change names, for refs.
fn named_keys(change: &Change) -> Vec<String> {
    change.new.iter().chain(&change.changed).chain(&change.gone).map(element_key).collect()
}

/// One action on an app without the library: look, act, wait for the screen to settle, say what changed. The
/// change comes from the platform's accessibility tree, the same one `tree --device` lists; nothing is checked.
fn perform(
    args: &Args,
    verb: &str,
    given: Value,
    build: impl Fn(&[Value], &str) -> Result<(Action, Vec<Place>)>,
) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let mut screen = Screen::open(&platform, &device)?;
    let looking = Instant::now();
    let mut before = screen.look()?;
    let (action, places) = loop {
        match build(&before, &device) {
            Err(missing) if missing.code == "unknown_element" && looking.elapsed() < ELEMENT_WAIT => {
                pause(LOOK_EVERY);
                before = screen.look()?;
            }
            built => break built?,
        }
    };
    pause(BUSY_SAMPLE);
    let settled = screen.look()?;
    let busy = differing(&before, &settled);
    let started = Instant::now();
    if let (Action::Type(_), Some(field)) = (&action, places.first()) {
        screen.act(&Action::Tap(field.at))?;
        pause(FOCUS_WAIT);
    }
    screen.act(&action)?;
    let took = started.elapsed().as_secs_f64() * 1000.0;
    let (after, still_changing) = settle(&mut screen, &settled, &busy)?;
    let mut change = change(&settled, &after, &busy);
    change.still_changing = still_changing;
    let refs = refs_for(&named_keys(&change))?;
    let outcome = describe(&change, &refs);
    say!("{verb} on {device} in {}ms; {outcome}; {NO_CHECKS}", took.round() as i64);
    let mut note = json!({
        "mode": "device",
        "platform": platform,
        "places": places.iter().map(|p| json!({"given": p.given, "at": [p.at.0, p.at.1], "seen": p.seen})).collect::<Vec<_>>(),
        "new": change.new.iter().map(display).collect::<Vec<_>>(),
        "appeared": confirmed(&mut screen, appeared(&on_screen(&settled), &on_screen(&after), &busy))?,
        "gone": change.gone.iter().map(display).collect::<Vec<_>>(),
        "changed": change.changed.iter().map(display).collect::<Vec<_>>(),
        "ms": took.round(),
    });
    if let (Some(note), Some(given)) = (note.as_object_mut(), given.as_object()) {
        note.extend(given.clone());
    }
    crate::walk::note(note);
    Ok(0)
}

fn duration(args: &Args, default_ms: f64) -> Result<f64> {
    Ok(args.duration.as_deref().map(crate::state::parse_ms).transpose()?.unwrap_or(default_ms))
}

/// `tap`, `double-tap`, `long-press`, `swipe`, `drag` without the library. A place is `x,y` (pixels on Android,
/// points on iOS), a label or id from the platform's accessibility tree, or an `@eN` from `tree` or an earlier line.
pub fn gesture(args: &Args) -> Result<i32> {
    let gesture = args.command.clone();
    let from_text = args.id.clone().unwrap_or_default();
    let to_from_ref = args.value.as_deref().is_some_and(|text| text.starts_with("@e"));
    let to_text = match (&gesture[..], &args.value) {
        ("swipe" | "drag", Some(text)) => Some(crate::queries::resolve_ref(text)?),
        ("swipe" | "drag", None) => fail!(
            format!("{gesture} needs where to: FROM TO"),
            "missing_option",
            format!("ddp {gesture} 200,600 200,200")
        ),
        _ => None,
    };
    let ms = match &gesture[..] {
        "swipe" => duration(args, 300.0)?,
        "drag" => duration(args, 600.0)?,
        "long-press" => duration(args, 800.0)?,
        _ => 0.0,
    };
    let verb = match &to_text {
        Some(to) => format!("{gesture} {from_text} -> {to}"),
        None => format!("{gesture} {from_text}"),
    };
    let given = match &gesture[..] {
        "tap" | "double-tap" => json!({}),
        _ => json!({"ms_given": args.duration.as_deref().map(crate::state::parse_ms).transpose()?}),
    };
    perform(args, &verb, given, |elements, device| {
        let from = place(elements, &from_text, args.id_from_ref, device)?;
        let to = to_text.as_deref().map(|to| place(elements, to, to_from_ref, device)).transpose()?;
        let action = match (&gesture[..], &to) {
            ("tap", _) => Action::Tap(from.at),
            ("double-tap", _) => Action::DoubleTap(from.at),
            ("long-press", _) => Action::LongPress(from.at, ms),
            ("swipe", Some(to)) => Action::Drag { from: from.at, to: to.at, ms, hold: 0 },
            (_, Some(to)) => Action::Drag { from: from.at, to: to.at, ms, hold: crate::hit::DRAG_HOLD_MS },
            _ => unreachable!("swipe and drag have a place to go"),
        };
        Ok((action, [Some(from), to].into_iter().flatten().collect()))
    })
}

/// `type` without the library: into the focused field, or into `--into` after tapping it.
pub fn type_text(args: &Args) -> Result<i32> {
    let text = args.value.clone().unwrap_or_default();
    let into = args.id.as_ref().map(|id| format!(" into {id}")).unwrap_or_default();
    perform(
        args,
        &format!("typed {} characters{into}", text.chars().count()),
        json!({"text": text}),
        |elements, device| {
            let field = args.id.as_deref().map(|id| place(elements, id, args.id_from_ref, device)).transpose()?;
            Ok((Action::Type(text.clone()), field.into_iter().collect()))
        },
    )
}

pub fn press(args: &Args) -> Result<i32> {
    let key = args.value.clone().unwrap_or_default();
    perform(args, &format!("pressed {key}"), json!({"key": key}), |_, _| Ok((Action::Key(key.clone()), Vec::new())))
}

/// `tree` without the library: one line per element of the platform's accessibility tree,
/// `@eN type "label" #id = "value" [x,y wxh]` in the platform's unit (pixels on Android, points on iOS); `tap` marks
/// what Android says takes a click. The ref, label or id is what `tap` takes.
pub fn tree(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let elements = Screen::open(&platform, &device)?.elements()?;
    if args.json {
        say!("{}", py::dumps(&json!({"elements": elements, "checks": false})));
        return Ok(0);
    }
    let refs = refs_for(&elements.iter().map(element_key).filter(|k| k != "?").collect::<Vec<_>>())?;
    for element in &elements {
        say!("{}", tree_line(element, &refs));
    }
    say!("{} elements on {device}; {NO_CHECKS}", elements.len());
    Ok(0)
}

/// An element `wait` and `find` look for: `id` an id (or `Delete [2]`), `text` part of a label, any case.
fn matches(element: &Value, by: &str, value: &str) -> bool {
    if by == "id" {
        element_key(element) == value || element["id"] == value
    } else {
        let wanted = crate::model::normalize_label(value).to_lowercase();
        element["label"].as_str().is_some_and(|label| label.to_lowercase().contains(&wanted))
    }
}

/// `wait` without the library: until an element is in the accessibility tree, or the tree stops changing.
pub fn wait(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let settled_ms = args.settled.as_deref().map(crate::state::parse_ms).transpose()?;
    if args.by.is_none() && settled_ms.is_none() {
        fail!("wait needs `id <id>`, `text <label>` or --settled 300ms", "missing_option");
    }
    let timeout = crate::state::parse_ms(&args.timeout)?;
    let value = args.value.clone().unwrap_or_default();
    let ignored: HashSet<String> = args.ignore.iter().cloned().collect();
    let mut screen = Screen::open(&platform, &device)?;
    let started = Instant::now();
    let (mut previous, mut still_since): (Option<Vec<Value>>, Instant) = (None, started);
    loop {
        let elements = screen.elements()?;
        let ms = started.elapsed().as_millis();
        if let Some(by) = &args.by {
            if let Some(found) = elements.iter().find(|e| matches(e, by, &value)) {
                let refs = refs_for(&[element_key(found)])?;
                say!("{} on screen after {ms}ms; {NO_CHECKS}", tree_line(found, &refs));
                return Ok(0);
            }
        } else {
            let changed = previous
                .as_ref()
                .is_none_or(|before| differing(before, &elements).iter().any(|key| !ignored.contains(key)));
            if changed {
                still_since = Instant::now();
            } else if still_since.elapsed().as_secs_f64() * 1000.0 >= settled_ms.unwrap() {
                say!(
                    "settled: nothing changed for {}ms (after {ms}ms); {NO_CHECKS}",
                    still_since.elapsed().as_millis()
                );
                return Ok(0);
            }
            previous = Some(elements);
        }
        if ms as f64 >= timeout {
            let what = match &args.by {
                Some(by) => format!("{by} {value}"),
                None => format!("{} without change", args.settled.clone().unwrap_or_default()),
            };
            fail!(
                format!("timed out after {} waiting for {what}", args.timeout),
                "timeout",
                format!("ddp tree --device {device} to see what is there, or a longer --timeout")
            );
        }
        pause(LOOK_EVERY);
    }
}

/// `find text|id` without the library: the matching elements of the accessibility tree, as `tree` lists them.
pub fn find(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let (Some(by), Some(value)) = (args.by.as_deref().filter(|by| *by == "text" || *by == "id"), &args.value) else {
        fail!("find without the library takes `text <part of a label>` or `id <id>`", "bad_find");
    };
    let elements = Screen::open(&platform, &device)?.elements()?;
    let found: Vec<&Value> = elements.iter().filter(|e| matches(e, by, value)).collect();
    if args.json {
        say!("{}", py::dumps(&json!({"elements": found, "checks": false})));
        return Ok(0);
    }
    if found.is_empty() {
        say!("no match");
        return Ok(0);
    }
    let refs = refs_for(&found.iter().map(|e| element_key(e)).collect::<Vec<_>>())?;
    for element in &found {
        say!("{}", tree_line(element, &refs));
    }
    say!("{} of {} elements on {device}; {NO_CHECKS}", found.len(), elements.len());
    Ok(0)
}

/// `@e3 Button "Save" #save = "value" [x,y wxh] tap disabled`
/// `[x,y wxh]`, as `tree` prints a frame.
fn bracket(frame: &[i64]) -> String {
    format!("[{},{} {}x{}]", frame[0], frame[1], frame[2], frame[3])
}

fn tree_line(element: &Value, refs: &BTreeMap<String, String>) -> String {
    let frame = frame_of(element);
    let mut line = refs.get(&element_key(element)).map(|r| format!("{r} ")).unwrap_or_default();
    line += &py::str(&element["type"]);
    if let Some(label) = element["label"].as_str() {
        line += &format!(" {}", py::dumps(&json!(label)));
    }
    if let Some(id) = element["id"].as_str() {
        line += &format!(" #{id}");
    }
    if let Some(value) = element["value"].as_str() {
        line += &format!(" = {}", py::dumps(&json!(value)));
    }
    line += &format!(" {}", bracket(&frame));
    if element["tap"] == true {
        line += " tap";
    }
    if element["enabled"] == false {
        line += " disabled";
    }
    line
}

/// `ddp doctor`: the Mac-side hooks for iOS apps without the library, each asked once (an empty key release, the
/// app in front).
pub fn ios_hook_steps(device: &str) -> Vec<(bool, String, String)> {
    let step = |hook: &str, answer: Result<()>, feeds: &str| match answer {
        Ok(()) => (true, format!("hook {hook} (private): answered"), String::new()),
        Err(failure) => (
            false,
            format!("hook {hook} (private): failing ({})", failure.message),
            format!("update agent-ddp, or report it with this Xcode version; until then: {feeds} on iOS fail"),
        ),
    };
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    match Simulator::open(device) {
        Ok(mut simulator) => vec![
            step("simulator.hid", simulator.hid_answers(), "tap, type and press without the library"),
            step("simulator.accessibility", simulator.front_app().map(|_| ()), "tree and places without the library"),
        ],
        Err(failure) => vec![step("simulator.hid", Err(failure), "everything without the library")],
    }
}

/// Simulators exist only on a Mac: the same calls, refused.
#[cfg(not(target_os = "macos"))]
mod mac_only {
    use crate::fail;
    use crate::state::Result;
    use serde_json::Value;

    pub const START: u64 = 0;
    pub const MOVE: u64 = 1;
    pub const END: u64 = 2;
    pub const ENTER: u64 = 40;
    pub const BACKSPACE: u64 = 42;
    pub const TAB: u64 = 43;

    pub struct Simulator;

    impl Simulator {
        pub fn open(_: &str) -> Result<Simulator> {
            fail!("iOS simulators run on a Mac only", "not_supported");
        }
        pub fn elements(&mut self) -> Result<Vec<Value>> {
            unreachable!()
        }
        pub fn finger(&mut self, _: u64, _: (f64, f64)) -> Result<()> {
            unreachable!()
        }
        pub fn key(&mut self, _: u64, _: bool) -> Result<()> {
            unreachable!()
        }
        pub fn delivered(&mut self) -> Result<()> {
            unreachable!()
        }
        pub fn type_text(&mut self, _: &str) -> Result<()> {
            unreachable!()
        }
        pub fn hid_answers(&mut self) -> Result<()> {
            unreachable!()
        }
        pub fn front_app(&mut self) -> Result<()> {
            unreachable!()
        }
    }
}
