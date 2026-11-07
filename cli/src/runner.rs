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
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use crate::simulator::{Simulator, BACKSPACE, END, ENTER, MOVE, START, TAB};
#[cfg(not(target_os = "macos"))]
use mac_only::{Simulator, BACKSPACE, END, ENTER, MOVE, START, TAB};

const SOCKET: &str = "localabstract:agentddp.runner.control";
const PACKAGE: &str = "dev.agentddp.runner";
const APK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runner.apk"));
/// `am instrument` to the socket open took 2 s on a warm emulator, 21 s on one booted a minute before.
const START_PATIENCE: Duration = Duration::from_secs(40);
/// The runner's `VERSION` (Runner.kt) this ddp speaks.
const RUNNER_VERSION: u64 = 5;
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
            Err(_) if Instant::now() >= deadline => fail!(
                format!("ddp's runner did not start on {device} in {}s", START_PATIENCE.as_secs()),
                "unreachable",
                format!("adb -s {device} logcat -d | grep -i agentddp.runner, then the command again")
            ),
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
        let mut elements = frame["elements"].as_array().cloned().unwrap_or_default();
        key_unnamed(&mut elements);
        Ok(elements)
    }

    /// Where a touch on a named element lands. On iOS the app answers it (the visible point VoiceOver touches): a bar
    /// or a view over the element's middle moves the touch to a part that takes it, and over all of it refuses the
    /// touch, since it would land on the cover and "nothing changed" would read as the app's answer. A container's
    /// point is a child's, so a container keeps its middle; so does any element the app gives no point for.
    fn reach(&mut self, elements: &[Value], mut place: Place, device: &str) -> Result<Place> {
        let (Screen::Ios(simulator), Some(seen)) = (&mut *self, &place.seen) else { return Ok(place) };
        let key = element_key(seen);
        if crate::hit::point(&place.given).is_some() || elements.iter().any(|e| e["parent"].as_str() == Some(&key)) {
            return Ok(place);
        }
        let Some(index) = elements.iter().position(|e| e == seen) else { return Ok(place) };
        match simulator.visible_point(index)? {
            Some(Some(at)) if inside(at, seen) => place.at = at,
            Some(None) => fail!(
                format!(
                    "'{}' is covered at {}: the app's hit test reaches no point of it (a bar or a view over it); swipe it into view first",
                    place.given,
                    bracket(&frame_of(seen))
                ),
                "covered",
                format!("ddp swipe <from> <to> --device {device}, then ddp tree --device {device}; tap --force touches it anyway")
            ),
            _ => {}
        }
        Ok(place)
    }

    /// What the screen shows, for a scroll with no `--in`: Android's display; on iOS the elements, which the
    /// simulator already cuts to the screen.
    fn area(&mut self, elements: &[Value]) -> Result<[f64; 4]> {
        if let Screen::Android(control) = self {
            let size = crate::model::rect(&control.call("app.info", json!({}))?["screen"]);
            return Ok([0.0, 0.0, size[0], size[1]]);
        }
        let rects: Vec<[f64; 4]> = on_screen(elements).iter().map(|e| crate::model::rect(&e["frame"])).collect();
        let (left, top) = rects.iter().fold((f64::MAX, f64::MAX), |(l, t), [x, y, _, _]| (l.min(*x), t.min(*y)));
        let (right, bottom) =
            rects.iter().fold((0.0_f64, 0.0_f64), |(r, b), [x, y, w, h]| (r.max(x + w), b.max(y + h)));
        if rects.is_empty() {
            fail!("nothing on screen to scroll", "unknown_element", "ddp tree --device <device>");
        }
        Ok([left, top, right - left, bottom - top])
    }

    /// `; in front: com.google.android.permissioncontroller, not dev.example`: another app over the one this session
    /// drives (a permission dialog, the launcher, the notification shade). Android only: the runner reads the
    /// package of the window that takes input; iOS says nothing.
    fn front_note(&mut self, app: Option<&str>) -> String {
        let (Screen::Android(control), Some(app)) = (self, app) else { return String::new() };
        match control.call("app.info", json!({})).ok().and_then(|info| info["app"].as_str().map(String::from)) {
            Some(front) if front != app => format!("; in front: {front}, not {app}"),
            _ => String::new(),
        }
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
            Action::Drag { from, to, ms, hold, still } => {
                let mut params = json!({"from": point(*from), "to": point(*to), "ms": ms, "hold": hold});
                if *still > 0 {
                    params["still"] = json!(still);
                }
                self.android("input.drag", params)
            }
            Action::Type { text, clear } => {
                if *clear {
                    self.android("input.clear", json!({}))?;
                }
                if text.is_empty() {
                    return Ok(());
                }
                self.android("input.type", json!({"text": text}))
            }
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
        Action::Drag { from, to, ms, hold, still } => {
            simulator.finger(START, *from)?;
            pause(Duration::from_millis(*hold));
            let steps = (*ms / FRAME.as_millis() as f64).round().max(1.0) as u32;
            for step in 1..=steps {
                pause(FRAME);
                let share = step as f64 / steps as f64;
                simulator.finger(MOVE, (from.0 + (to.0 - from.0) * share, from.1 + (to.1 - from.1) * share))?;
            }
            for _ in 0..*still / FRAME.as_millis() as u64 {
                pause(FRAME);
                simulator.finger(MOVE, *to)?;
            }
            simulator.finger(END, *to)?;
        }
        Action::Type { text, clear } => {
            if *clear {
                // Select all, then delete: the keys a person with a keyboard presses.
                simulator.key(LEFT_COMMAND, true)?;
                press_key(simulator, KEY_A)?;
                simulator.key(LEFT_COMMAND, false)?;
                press_key(simulator, BACKSPACE)?;
            }
            if text.is_empty() {
                return simulator.delivered();
            }
            return simulator.type_text(text);
        }
        Action::Key(key) => {
            let usage = match key.as_str() {
                "Enter" => ENTER,
                "Tab" => TAB,
                "Backspace" => BACKSPACE,
                "Escape" => ESCAPE,
                "Right" => RIGHT,
                "Left" => LEFT,
                "Down" => DOWN,
                "Up" => UP,
                _ => fail!(
                    format!("no key {key} on iOS: Home, Recents and Notifications are Android's"),
                    "bad_value",
                    "ddp background --for 2s leaves the app and comes back"
                ),
            };
            press_key(simulator, usage)?;
        }
    }
    simulator.delivered()
}

fn press_key(simulator: &mut Simulator, usage: u64) -> Result<()> {
    simulator.key(usage, true)?;
    simulator.key(usage, false)
}

/// USB HID keyboard usages (page 7), as the simulator's keyboard takes them.
const KEY_A: u64 = 4;
const ESCAPE: u64 = 41;
const RIGHT: u64 = 79;
const LEFT: u64 = 80;
const DOWN: u64 = 81;
const UP: u64 = 82;
const LEFT_COMMAND: u64 = 227;

/// What one command does to the app, in the platform's unit (points on iOS, pixels on Android).
enum Action {
    Tap((f64, f64)),
    DoubleTap((f64, f64)),
    LongPress((f64, f64), f64),
    /// `still`: ms held at the end before letting go, so a scroll view sees no speed and does not fling.
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        ms: f64,
        hold: u64,
        still: u64,
    },
    Type {
        text: String,
        clear: bool,
    },
    Key(String),
}

/// A place as the agent gave it, where it is now, and the element seen there: a walk keeps what was seen, so a
/// replay finds the element again instead of touching the same pixels.
struct Place {
    given: String,
    at: (f64, f64),
    seen: Option<Value>,
}

fn inside((px, py): (f64, f64), element: &Value) -> bool {
    let [x, y, w, h] = crate::model::rect(&element["frame"]);
    px >= x && px <= x + w && py >= y && py <= y + h
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
        [] if !off.is_empty() => {
            let frame = frame_of(off[0]);
            let direction = if frame[1] < 0 { "up" } else { "down" };
            fail!(
                format!("'{text}' is off screen at {}: scroll it into view first", bracket(&frame)),
                "off_screen",
                format!("ddp scroll {direction} --to '{text}' --device {device}")
            )
        }
        [] if from_ref => fail!(
            format!("no element '{text}' on screen now: it went away, or it has no label or id and moved (such an element is known by its place)"),
            "unknown_element",
            format!("ddp tree --device {device}")
        ),
        [] => fail!(
            format!("no element '{text}' in the accessibility tree (not in it: a list's rows scrolled out of view, what the iOS keyboard covers, an Android Compose testTag without testTagsAsResourceId)"),
            "unknown_element",
            format!("ddp scroll down --to '{text}' --device {device} (or up), or ddp tree --device {device}")
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

/// Element key (id, else label, else type and place) to what the screen shows of it.
fn keyed(elements: &[Value]) -> BTreeMap<String, &Value> {
    let mut keyed = BTreeMap::new();
    for element in elements {
        keyed.entry(element_key(element)).or_insert(element);
    }
    keyed
}

fn unnamed(element: &Value) -> bool {
    ["label", "id"].iter().all(|field| element[field].as_str().is_none_or(str::is_empty))
}

/// An element with no label and no id is known by its type and place (`ImageButton [24,96 48x48]`): its ref fails
/// once it moves, instead of following a `? [2]` numbering onto another unnamed element.
fn key_unnamed(elements: &mut [Value]) {
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for element in elements.iter_mut().filter(|e| unnamed(e)) {
        let place = format!("{} {}", element["type"].as_str().unwrap_or("element"), bracket(&frame_of(element)));
        let count = seen.entry(place.clone()).or_insert(0);
        *count += 1;
        let key = if *count == 1 { place } else { format!("{place} [{count}]") };
        renamed.insert(element_key(element), key.clone());
        element["key"] = key.into();
    }
    for element in elements.iter_mut() {
        if let Some(key) = element["parent"].as_str().and_then(|parent| renamed.get(parent)) {
            element["parent"] = key.clone().into();
        }
    }
}

/// An unnamed element is known by its place, so one that moved has a new key: matched to a lost one of the same type
/// and size, it moved (or changed, when what it shows did). Only what has no match is new or gone.
fn pair_unnamed(change: &mut Change, mut lost: Vec<&Value>, found: Vec<&Value>) {
    let size = |element: &Value| frame_of(element)[2..].to_vec();
    for element in found {
        let twin = lost.iter().position(|old| old["type"] == element["type"] && size(old) == size(element));
        match twin.map(|at| lost.remove(at)) {
            Some(old) if off_screen(old) && off_screen(element) => {}
            Some(old) if !off_screen(element) && state(old) != state(element) => change.changed.push(element.clone()),
            Some(_) => change.moved += 1,
            None if off_screen(element) => {}
            None => change.new.push(element.clone()),
        }
    }
    change.gone.extend(lost.into_iter().filter(|e| !off_screen(e)).cloned());
}

/// What the agent reads about an element: label, value, enabled, its marks. Its place is compared apart (a scroll
/// moves all).
fn state(element: &Value) -> (String, String, bool, String) {
    (py::str(&element["label"]), py::str(&element["value"]), element["enabled"] != false, marks(element, false))
}

/// The state the platform gives an element, as `tree` prints it after the frame: ` checked`, ` selected`,
/// ` collapsed`, ` error "Required"`. The focus only where asked: it moves with every tap and would fill change lines.
fn marks(element: &Value, with_focus: bool) -> String {
    let mut marks = String::new();
    let mut mark = |on: bool, name: &str| {
        if on {
            marks += " ";
            marks += name;
        }
    };
    let selected = element["traits"].as_array().is_some_and(|traits| traits.contains(&json!("selected")));
    mark(element["checked"] == true, "checked");
    mark(element["checked"] == false, "unchecked");
    mark(element["selected"] == true || selected, "selected");
    mark(element["expanded"] == true, "expanded");
    mark(element["expanded"] == false, "collapsed");
    mark(with_focus && element["focused"] == true, "focused");
    mark(element["password"] == true, "password");
    if let Some(error) = element["error"].as_str() {
        marks += &format!(" error {}", py::dumps(&json!(error)));
    }
    marks
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
    let mut found = Vec::new();
    for (key, element) in &is {
        if busy.contains(key) {
            continue;
        }
        match was.get(key) {
            None if unnamed(element) => found.push(*element),
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
    let (lost, gone): (Vec<&Value>, Vec<&Value>) = was
        .iter()
        .filter(|(key, _)| !busy.contains(*key) && !is.contains_key(*key))
        .map(|(_, element)| *element)
        .partition(|element| unnamed(element));
    change.gone = gone.into_iter().filter(|e| !off_screen(e)).cloned().collect();
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
    pair_unnamed(&mut change, lost, found);
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

/// `@e7 "Saved"`, `@e7 #save` or `@e7 ImageButton [24,96 48x48]`: the ref to act on and what a person sees.
fn named(element: &Value, refs: &BTreeMap<String, String>) -> String {
    let key = element_key(element);
    let shown = match element["label"].as_str() {
        Some(label) => py::dumps(&json!(label)),
        None if unnamed(element) => key.clone(),
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
                    let marks = if with_value { marks(e, false) } else { String::new() };
                    format!("{}{}{marks}", named(e, refs), value.unwrap_or_default())
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
        None if unnamed(element) => element_key(element),
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
    build: impl Fn(&[Value], &str, &mut Screen) -> Result<(Action, Vec<Place>)>,
) -> Result<i32> {
    let (platform, device, app) = crate::device::target(args)?;
    let mut screen = Screen::open(&platform, &device)?;
    let looking = Instant::now();
    let mut before = screen.look()?;
    let (action, places) = loop {
        match build(&before, &device, &mut screen) {
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
    if let (Action::Type { .. }, Some(field)) = (&action, places.first()) {
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
    let front = screen.front_note(app.as_deref());
    say!("{verb} on {device} in {}ms; {outcome}{front}; {NO_CHECKS}", took.round() as i64);
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
    perform(args, &verb, given, |elements, device, screen| {
        let from = place(elements, &from_text, args.id_from_ref, device)?;
        let from = if args.force { from } else { screen.reach(elements, from, device)? };
        let to = to_text.as_deref().map(|to| place(elements, to, to_from_ref, device)).transpose()?;
        let action = match (&gesture[..], &to) {
            ("tap", _) => Action::Tap(from.at),
            ("double-tap", _) => Action::DoubleTap(from.at),
            ("long-press", _) => Action::LongPress(from.at, ms),
            ("swipe", Some(to)) => Action::Drag { from: from.at, to: to.at, ms, hold: 0, still: 0 },
            (_, Some(to)) => Action::Drag { from: from.at, to: to.at, ms, hold: crate::hit::DRAG_HOLD_MS, still: 0 },
            _ => unreachable!("swipe and drag have a place to go"),
        };
        Ok((action, [Some(from), to].into_iter().flatten().collect()))
    })
}

/// `type` without the library: into the focused field, or into `--into` after tapping it.
pub fn type_text(args: &Args) -> Result<i32> {
    let text = args.value.clone().unwrap_or_default();
    let into = args.id.as_ref().map(|id| format!(" into {id}")).unwrap_or_default();
    let emptied = if args.clear { " after emptying it" } else { "" };
    let given = if args.clear { json!({"text": text, "clear": true}) } else { json!({"text": text}) };
    perform(
        args,
        &format!("typed {} characters{into}{emptied}", text.chars().count()),
        given,
        |elements, device, screen| {
            let field = args
                .id
                .as_deref()
                .map(|id| screen.reach(elements, place(elements, id, args.id_from_ref, device)?, device))
                .transpose()?;
            Ok((Action::Type { text: text.clone(), clear: args.clear }, field.into_iter().collect()))
        },
    )
}

/// iOS without the library: the edge swipe a thumb makes, from the very edge (UIKit's screen-edge recognizer starts
/// only there) to most of the way across.
pub fn back(args: &Args) -> Result<i32> {
    perform(args, "back (edge swipe)", json!({}), |elements, _, screen| {
        let [_, y, w, h] = screen.area(elements)?;
        let from = (1.0, y + h / 2.0);
        Ok((Action::Drag { from, to: (w * 0.7, from.1), ms: 250.0, hold: 0, still: 0 }, Vec::new()))
    })
}

pub fn press(args: &Args) -> Result<i32> {
    let key = args.value.clone().unwrap_or_default();
    perform(args, &format!("pressed {key}"), json!({"key": key}), |_, _, _| Ok((Action::Key(key.clone()), Vec::new())))
}

/// One scroll moves the content by this share of its area, slowly, and holds still before letting go: nothing flings,
/// so each scroll moves about as far and `--to` does not fly past what it looks for.
const SCROLL_SHARE: f64 = 0.4;
const SCROLL_MS: f64 = 500.0;
const SCROLL_STILL_MS: u64 = 100;
/// `scroll --to` gives up after this many scrolls.
const MAX_SCROLLS: usize = 15;

/// The finger's way for one scroll across the middle of `area`: `down` shows what is below, so the finger goes up.
fn stroke(direction: &str, [x, y, w, h]: [f64; 4]) -> Action {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (dx, dy) = (w * SCROLL_SHARE / 2.0, h * SCROLL_SHARE / 2.0);
    let (from, to) = match direction {
        "down" => ((cx, cy + dy), (cx, cy - dy)),
        "up" => ((cx, cy - dy), (cx, cy + dy)),
        "right" => ((cx + dx, cy), (cx - dx, cy)),
        _ => ((cx - dx, cy), (cx + dx, cy)),
    };
    Action::Drag { from, to, ms: SCROLL_MS, hold: 0, still: SCROLL_STILL_MS }
}

/// `scroll up|down|left|right [--in <element>] [--to <text>]` without the library. `--to` scrolls until an element
/// whose label holds the text (or with that id) is on screen, and stops when a scroll moves nothing (the end).
pub fn scroll(args: &Args) -> Result<i32> {
    let direction = args.value.clone().unwrap_or_default();
    let within = args.within.as_deref().map(crate::queries::resolve_ref).transpose()?;
    let within_from_ref = args.within.as_deref().is_some_and(|text| text.starts_with("@e"));
    let target = args.end.as_deref().map(crate::queries::resolve_ref).transpose()?;
    let area = |screen: &mut Screen, elements: &[Value], device: &str| -> Result<[f64; 4]> {
        match &within {
            Some(within) => {
                let place = place(elements, within, within_from_ref, device)?;
                Ok(crate::model::rect(&place.seen.map(|seen| seen["frame"].clone()).unwrap_or_default()))
            }
            None => screen.area(elements),
        }
    };
    let mut given = json!({"direction": direction});
    for (field, text) in [("to", &target), ("in", &within)] {
        if let Some(text) = text {
            given[field] = json!(text);
        }
    }
    let Some(target) = target else {
        return perform(args, &format!("scroll {direction}"), given, |elements, device, screen| {
            Ok((stroke(&direction, area(screen, elements, device)?), Vec::new()))
        });
    };
    let (platform, device, _) = crate::device::target(args)?;
    let mut screen = Screen::open(&platform, &device)?;
    let started = Instant::now();
    // A screen still moving (the last command's transition) would give a place that is gone a moment later.
    let first = screen.look()?;
    pause(BUSY_SAMPLE);
    let mut elements = screen.look()?;
    if !differing(&first, &elements).is_empty() {
        elements = settle(&mut screen, &elements, &HashSet::new())?.0;
    }
    for scrolls in 0..=MAX_SCROLLS {
        let shown = on_screen(&elements);
        let found: Vec<&Value> =
            shown.iter().filter(|e| matches(e, "id", &target) || matches(e, "text", &target)).collect();
        if let Some(first) = found.first() {
            let refs = refs_for(&[element_key(first)])?;
            let more = if found.len() > 1 { format!(" (+{} more)", found.len() - 1) } else { String::new() };
            let ms = started.elapsed().as_millis();
            say!(
                "scroll {direction} --to {target} on {device}: {}{more} on screen after {} in {ms}ms; {NO_CHECKS}",
                tree_line(first, &refs),
                count(scrolls, "scroll")
            );
            let mut note = json!({"mode": "device", "platform": platform, "places": [], "new": [], "appeared": [],
                                  "gone": [], "changed": [], "ms": ms});
            if let (Some(note), Some(given)) = (note.as_object_mut(), given.as_object()) {
                note.extend(given.clone());
            }
            crate::walk::note(note);
            return Ok(0);
        }
        if scrolls == MAX_SCROLLS {
            break;
        }
        let swipe = stroke(&direction, area(&mut screen, &elements, &device)?);
        screen.act(&swipe)?;
        let (after, _) = settle(&mut screen, &elements, &HashSet::new())?;
        if differing(&elements, &after).is_empty() {
            fail!(
                format!(
                    "no '{target}' after {} {direction}: the last one moved nothing (the end, or nothing scrolls there)",
                    count(scrolls + 1, "scroll")
                ),
                "not_found",
                format!("ddp scroll {} --to '{target}' --device {device}, or ddp tree --device {device}", opposite(&direction))
            );
        }
        elements = after;
    }
    fail!(
        format!("no '{target}' after {MAX_SCROLLS} scrolls {direction}"),
        "not_found",
        format!("ddp tree --device {device}")
    )
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn opposite(direction: &str) -> &'static str {
    match direction {
        "down" => "up",
        "up" => "down",
        "left" => "right",
        _ => "left",
    }
}

/// `tree` without the library: one line per element of the platform's accessibility tree,
/// `@eN type "label" #id = "value" [x,y wxh]` in the platform's unit (pixels on Android, points on iOS); `tap` marks
/// what Android says takes a click. The ref, label or id is what `tap` takes.
pub fn tree(args: &Args) -> Result<i32> {
    let (platform, device, app) = crate::device::target(args)?;
    let mut screen = Screen::open(&platform, &device)?;
    if let Screen::Ios(simulator) = &mut screen {
        simulator.list_containers();
    }
    let elements = screen.elements()?;
    if args.json {
        say!("{}", py::dumps(&json!({"elements": elements, "checks": false})));
        return Ok(0);
    }
    let refs = refs_for(&elements.iter().map(element_key).collect::<Vec<_>>())?;
    for element in &elements {
        say!("{}", tree_line(element, &refs));
    }
    let front = screen.front_note(app.as_deref());
    say!("{} elements on {device}{front}; {NO_CHECKS}", elements.len());
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
    if args.gone && args.by.is_none() {
        fail!("wait --gone needs `id <id>` or `text <label>`", "missing_option", "ddp wait text Loading --gone");
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
            match (elements.iter().find(|e| matches(e, by, &value)), args.gone) {
                (Some(found), false) => {
                    let refs = refs_for(&[element_key(found)])?;
                    say!("{} on screen after {ms}ms; {NO_CHECKS}", tree_line(found, &refs));
                    return Ok(0);
                }
                (None, true) => {
                    say!("no {by} {} on screen after {ms}ms; {NO_CHECKS}", py::dumps(&json!(value)));
                    return Ok(0);
                }
                _ => {}
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
                Some(by) if args.gone => format!("{by} {value} to go"),
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
    line + &marks(element, true)
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
/// The process id of the app in front of an iOS simulator, from its accessibility service; None when it does not say.
pub fn ios_front_pid(device: &str) -> Option<i32> {
    Simulator::open(device).and_then(|mut simulator| simulator.front_pid()).ok()
}

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
        pub fn front_pid(&mut self) -> Result<i32> {
            unreachable!()
        }
        pub fn list_containers(&mut self) {}
        pub fn visible_point(&mut self, _: usize) -> Result<Option<Option<(f64, f64)>>> {
            unreachable!()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looked(elements: Value) -> Vec<Value> {
        let mut frame = json!({ "elements": elements });
        crate::model::name_elements(&mut frame);
        let mut elements = frame["elements"].as_array().cloned().unwrap();
        key_unnamed(&mut elements);
        elements
    }

    #[test]
    fn an_unnamed_element_is_known_by_its_place_and_its_move_is_not_new() {
        let before = looked(json!([
            {"type": "Button", "label": "Save", "frame": [0, 0, 100, 40]},
            {"type": "ImageButton", "frame": [0, 100, 48, 48]},
            {"type": "ImageButton", "frame": [0, 200, 48, 48], "parent": 1},
        ]));
        assert_eq!(element_key(&before[1]), "ImageButton [0,100 48x48]");
        assert_eq!(before[2]["parent"], "ImageButton [0,100 48x48]");
        let scrolled = looked(json!([
            {"type": "Button", "label": "Save", "frame": [0, 0, 100, 40]},
            {"type": "ImageButton", "frame": [0, 50, 48, 48]},
            {"type": "ImageButton", "frame": [0, 150, 48, 48]},
            {"type": "ImageView", "frame": [0, 300, 20, 20]},
        ]));
        let change = change(&before, &scrolled, &HashSet::new());
        assert_eq!((change.moved, change.gone.len()), (2, 0));
        assert_eq!(describe(&change, &BTreeMap::new()), "new: ImageView [0,300 20x20]; 2 moved");
        // The keyboard pans a field up while it takes the text, and pushes a button out of view.
        let field = |y: i64, value: Option<&str>| json!({"type": "EditText", "frame": [0, y, 300, 60], "value": value});
        let before = looked(json!([field(900, None), {"type": "View", "frame": [0, 0, 50, 50]}]));
        let typed =
            looked(json!([field(500, Some("milk")), {"type": "View", "frame": [0, -400, 50, 50], "shown": false}]));
        let typing = super::change(&before, &typed, &HashSet::new());
        assert_eq!(describe(&typing, &BTreeMap::new()), "changed: EditText [0,500 300x60] = \"milk\"; 1 moved");
    }
}
