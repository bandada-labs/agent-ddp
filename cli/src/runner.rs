//! Apps without the library (`--device`). Android: a runner (android/runner, an instrumentation inside the ddp binary)
//! serves the control protocol on the device, touches go in as the system's own input and elements come from the
//! platform's accessibility tree. iOS: the simulator's own HID and accessibility services, from the Mac
//! (simulator.rs). Neither knows the app's engine, so ddp says what it did and checks nothing.
use crate::args::Args;
use crate::control::Control;
use crate::py;
use crate::state::{adb_path, capture, failure, Result};
use crate::{fail, say};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const SOCKET: &str = "localabstract:agentddp.runner.control";
const PACKAGE: &str = "dev.agentddp.runner";
const APK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runner.apk"));
/// `am instrument` to the socket open took 2 s on an emulator; a cold device can take longer.
const START_PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// What every runner answer ends with: the runner did it, nothing was checked.
pub const NO_CHECKS: &str = "no checks (no SDK)";

/// A connection to the runner on this Android device, started (and installed) when it is not running.
pub fn open(device: &str) -> Result<Control> {
    let session = json!({"device": device, "app": PACKAGE, "port": forward(device)?});
    if let Ok(control) = Control::open(&session) {
        return Ok(control);
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

/// One call to the runner on its own connection; the answer's `ms` is the time spent on the device.
pub fn call(device: &str, method: &str, params: Value) -> Result<Value> {
    let mut control = open(device)?;
    let answer = control.call(method, params);
    control.close();
    // The runner names what it could not find; the agent's next step is to look at what is there.
    answer.map_err(|error| match error.message.strip_prefix("no element ") {
        Some(name) => failure(
            format!("no element '{name}' in the accessibility tree (a Compose testTag shows only with testTagsAsResourceId)"),
            "unknown_element",
            Some(&format!("ddp tree --device {device}")),
        ),
        None => error,
    })
}

/// `x,y` in screen pixels, or a label / resource id the runner looks up.
pub fn place(text: &str) -> Value {
    match crate::hit::point(text) {
        Some((x, y)) => json!({"x": x, "y": y}),
        None => json!({"label": text}),
    }
}

/// Default durations, as with the library: a swipe is quick, a drag holds first, a long press holds.
fn duration(args: &Args, default_ms: f64) -> Result<f64> {
    Ok(args.duration.as_deref().map(crate::state::parse_ms).transpose()?.unwrap_or(default_ms))
}

/// `tap`, `double-tap`, `long-press`, `swipe`, `drag` with `--device`. A place is `x,y` (pixels on Android, points on
/// iOS) or a label or id from the platform's accessibility tree. Nothing is checked: the app's engine is not there.
pub fn gesture(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let gesture = args.command.clone();
    let from_text = args.id.clone().unwrap_or_default();
    let to_text = match (&gesture[..], &args.value) {
        ("swipe" | "drag", Some(text)) => Some(text.clone()),
        ("swipe" | "drag", None) => fail!(
            format!("{gesture} needs where to: FROM TO"),
            "missing_option",
            format!("ddp {gesture} 200,600 200,200 --device {device}")
        ),
        _ => None,
    };
    let ms = match &gesture[..] {
        "swipe" => duration(args, 300.0)?,
        "drag" => duration(args, 600.0)?,
        "long-press" => duration(args, 800.0)?,
        _ => 0.0,
    };
    let took = if platform == "ios" {
        ios::gesture(&device, &gesture, &from_text, to_text.as_deref(), ms)?
    } else {
        let from = place(&from_text);
        let (method, params) = match &gesture[..] {
            "tap" => ("input.tap", from),
            "double-tap" => ("input.doubleTap", from),
            "long-press" => ("input.longPress", {
                let mut at = from;
                at["ms"] = json!(ms);
                at
            }),
            "swipe" => ("input.drag", json!({"from": from, "to": place(to_text.as_deref().unwrap_or("")), "ms": ms})),
            _ => (
                "input.drag",
                json!({"from": from, "to": place(to_text.as_deref().unwrap_or("")), "ms": ms, "hold": crate::hit::DRAG_HOLD_MS}),
            ),
        };
        call(&device, method, params)?["ms"].as_f64().unwrap_or(0.0)
    };
    say!(
        "{gesture} {from_text}{} on {device} in {}ms; {NO_CHECKS}",
        to_text.map(|to| format!(" -> {to}")).unwrap_or_default(),
        took.round() as i64
    );
    Ok(0)
}

/// `type --device`: into the focused field, or into `--into` after tapping it.
pub fn type_text(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let text = args.value.clone().unwrap_or_default();
    if platform == "ios" {
        ios::type_text(&device, args.id.as_deref(), &text)?;
    } else {
        if let Some(into) = &args.id {
            call(&device, "input.tap", place(into))?;
        }
        call(&device, "input.type", json!({"text": text}))?;
    }
    let into = args.id.as_ref().map(|id| format!(" into {id}")).unwrap_or_default();
    say!("typed {} characters{into} on {device}; {NO_CHECKS}", text.chars().count());
    Ok(0)
}

pub fn press(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let key = args.value.clone().unwrap_or_default();
    if platform == "ios" {
        ios::press(&device, &key)?;
    } else {
        call(&device, "input.key", json!({"key": key}))?;
    }
    say!("pressed {key} on {device}; {NO_CHECKS}");
    Ok(0)
}

/// `tree --device`: one line per element of the platform's accessibility tree, `type "label" #id [x,y wxh]` in the
/// platform's unit (pixels on Android, points on iOS); `tap` marks what Android says takes a click. The label or id
/// is what `tap --device` takes.
pub fn tree(args: &Args) -> Result<i32> {
    let (platform, device, _) = crate::device::target(args)?;
    let elements = if platform == "ios" {
        ios::elements(&device)?
    } else {
        call(&device, "screen.elements", json!({}))?["elements"].as_array().cloned().unwrap_or_default()
    };
    if args.json {
        say!("{}", py::dumps(&json!({"elements": elements, "checks": false})));
        return Ok(0);
    }
    for element in &elements {
        let frame: Vec<i64> = (0..4).map(|i| element["frame"][i].as_f64().unwrap_or(0.0).round() as i64).collect();
        let mut line = py::str(&element["type"]);
        if let Some(label) = element["label"].as_str() {
            line += &format!(" {}", py::dumps(&json!(label)));
        }
        if let Some(id) = element["id"].as_str() {
            line += &format!(" #{id}");
        }
        if let Some(value) = element["value"].as_str() {
            line += &format!(" = {}", py::dumps(&json!(value)));
        }
        line += &format!(" [{},{} {}x{}]", frame[0], frame[1], frame[2], frame[3]);
        if element["tap"] == true {
            line += " tap";
        }
        if element["enabled"] == false {
            line += " disabled";
        }
        say!("{line}");
    }
    say!("{} elements on {device}; {NO_CHECKS}", elements.len());
    Ok(0)
}

/// iOS simulators, driven from the Mac (simulator.rs). Places resolve in the accessibility tree, in points.
#[cfg(target_os = "macos")]
mod ios {
    use crate::fail;
    use crate::simulator::{Simulator, BACKSPACE, END, ENTER, MOVE, START, TAB};
    use crate::state::Result;
    use serde_json::Value;
    use std::time::{Duration, Instant};

    /// One move per display frame, so scroll views see a finger's speed.
    const FRAME_MS: f64 = 16.0;
    /// The field takes the focus on a later turn of the app's main loop; keys before that go nowhere.
    const FOCUS_WAIT_MS: u64 = 200;

    pub fn elements(device: &str) -> Result<Vec<Value>> {
        Simulator::open(device)?.elements()
    }

    /// `x,y` in points, or the center of the first element whose label or id is `text`.
    fn place(simulator: &mut Simulator, device: &str, text: &str) -> Result<(f64, f64)> {
        if let Some(point) = crate::hit::point(text) {
            return Ok(point);
        }
        let elements = simulator.elements()?;
        let Some(element) = elements.iter().find(|e| e["label"] == text || e["id"] == text) else {
            fail!(
                format!("no element '{text}' in the accessibility tree"),
                "unknown_element",
                format!("ddp tree --device {device}")
            );
        };
        let frame: Vec<f64> = (0..4).map(|i| element["frame"][i].as_f64().unwrap_or(0.0)).collect();
        Ok((frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0))
    }

    fn pause(ms: f64) {
        std::thread::sleep(Duration::from_millis(ms.max(0.0) as u64));
    }

    /// The gesture's time from the first lookup to dtuhidd's answer, in ms.
    pub fn gesture(device: &str, gesture: &str, from: &str, to: Option<&str>, ms: f64) -> Result<f64> {
        let mut simulator = Simulator::open(device)?;
        let started = Instant::now();
        let from = place(&mut simulator, device, from)?;
        let to = to.map(|to| place(&mut simulator, device, to)).transpose()?;
        let tap = |simulator: &mut Simulator| -> Result<()> {
            simulator.finger(START, from)?;
            simulator.finger(END, from)
        };
        match (gesture, to) {
            ("tap", _) => tap(&mut simulator)?,
            ("double-tap", _) => {
                tap(&mut simulator)?;
                pause(crate::hit::DOUBLE_TAP_GAP_MS as f64);
                tap(&mut simulator)?;
            }
            ("long-press", _) => {
                simulator.finger(START, from)?;
                pause(ms);
                simulator.finger(END, from)?;
            }
            (_, Some(to)) => {
                simulator.finger(START, from)?;
                if gesture == "drag" {
                    pause(crate::hit::DRAG_HOLD_MS as f64);
                }
                let steps = (ms / FRAME_MS).round().max(1.0) as u32;
                for step in 1..=steps {
                    pause(FRAME_MS);
                    let share = step as f64 / steps as f64;
                    simulator.finger(MOVE, (from.0 + (to.0 - from.0) * share, from.1 + (to.1 - from.1) * share))?;
                }
                simulator.finger(END, to)?;
            }
            _ => unreachable!("swipe and drag have a place to go"),
        }
        simulator.delivered()?;
        Ok(started.elapsed().as_secs_f64() * 1000.0)
    }

    pub fn type_text(device: &str, into: Option<&str>, text: &str) -> Result<()> {
        let mut simulator = Simulator::open(device)?;
        if let Some(into) = into {
            let at = place(&mut simulator, device, into)?;
            simulator.finger(START, at)?;
            simulator.finger(END, at)?;
            simulator.delivered()?;
            std::thread::sleep(Duration::from_millis(FOCUS_WAIT_MS));
        }
        simulator.type_text(text)
    }

    /// `ddp doctor`: one step per Mac-side hook, each asked once (an empty key release, the app in front).
    pub fn hook_steps(device: &str) -> Vec<(bool, String, String)> {
        let step = |hook: &str, answer: Result<()>, feeds: &str| match answer {
            Ok(()) => (true, format!("hook {hook} (private): answered"), String::new()),
            Err(failure) => (
                false,
                format!("hook {hook} (private): failing ({})", failure.message),
                format!("update agent-ddp, or report it with this Xcode version; until then: {feeds} on iOS fail"),
            ),
        };
        match Simulator::open(device) {
            Ok(mut simulator) => vec![
                step(crate::simulator::HID_HOOK, simulator.hid_answers(), "tap, type and press with --device"),
                step(
                    crate::simulator::ACCESSIBILITY_HOOK,
                    simulator.front_app().map(|_| ()),
                    "tree --device and places by label",
                ),
            ],
            Err(failure) => vec![step(crate::simulator::HID_HOOK, Err(failure), "everything with --device")],
        }
    }

    pub fn press(device: &str, key: &str) -> Result<()> {
        let usage = match key {
            "Enter" => ENTER,
            "Tab" => TAB,
            "Backspace" => BACKSPACE,
            _ => fail!(format!("no key {key} on iOS"), "bad_value"),
        };
        let mut simulator = Simulator::open(device)?;
        simulator.key(usage, true)?;
        simulator.key(usage, false)?;
        simulator.delivered()
    }
}

/// Simulators exist only on a Mac.
#[cfg(not(target_os = "macos"))]
mod ios {
    use crate::fail;
    use crate::state::Result;
    use serde_json::Value;

    fn mac_only<T>() -> Result<T> {
        fail!("iOS simulators run on a Mac only", "not_supported");
    }
    pub fn elements(_: &str) -> Result<Vec<Value>> {
        mac_only()
    }
    pub fn gesture(_: &str, _: &str, _: &str, _: Option<&str>, _: f64) -> Result<f64> {
        mac_only()
    }
    pub fn type_text(_: &str, _: Option<&str>, _: &str) -> Result<()> {
        mac_only()
    }
    pub fn press(_: &str, _: &str) -> Result<()> {
        mac_only()
    }
    pub fn hook_steps(_: &str) -> Vec<(bool, String, String)> {
        Vec::new()
    }
}

/// The Mac-side hooks for iOS apps without the library, as `ddp doctor` steps.
pub fn ios_hook_steps(device: &str) -> Vec<(bool, String, String)> {
    ios::hook_steps(device)
}
