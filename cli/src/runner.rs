//! Apps without the library. On Android a runner (android/runner, an instrumentation inside the ddp binary) serves
//! the control protocol on the device: touches go in as the system's own input and elements come from the platform's
//! accessibility tree. It knows nothing of the app's engine, so ddp says what it did and checks nothing.
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

/// The runner can only work where the platform lets it: Android. iOS gets its own helper (ddp-mz4, iOS part).
fn android_device(args: &Args) -> Result<String> {
    let (platform, device, _) = crate::device::target(args)?;
    if platform == "ios" {
        fail!(
            "without the library ddp drives Android only for now",
            "not_supported",
            "add the library to the Debug build and drop --device"
        );
    }
    Ok(device)
}

/// `type --device`: into the focused field, or into `--into` after tapping it.
pub fn type_text(args: &Args) -> Result<i32> {
    let device = android_device(args)?;
    let text = args.value.clone().unwrap_or_default();
    if let Some(into) = &args.id {
        call(&device, "input.tap", place(into))?;
    }
    call(&device, "input.type", json!({"text": text}))?;
    let into = args.id.as_ref().map(|id| format!(" into {id}")).unwrap_or_default();
    say!("typed {} characters{into} on {device}; {NO_CHECKS}", text.chars().count());
    Ok(0)
}

pub fn press(args: &Args) -> Result<i32> {
    let device = android_device(args)?;
    let key = args.value.clone().unwrap_or_default();
    call(&device, "input.key", json!({"key": key}))?;
    say!("pressed {key} on {device}; {NO_CHECKS}");
    Ok(0)
}

/// `tree --device`: one line per element the platform's accessibility tree has, `type "label" #id [x,y wxh]` in
/// screen pixels; `tap` marks what takes a touch. The label or id is what `tap --device` takes.
pub fn tree(args: &Args) -> Result<i32> {
    let device = android_device(args)?;
    let answer = call(&device, "screen.elements", json!({}))?;
    let elements = answer["elements"].as_array().cloned().unwrap_or_default();
    if args.json {
        say!("{}", py::dumps(&json!({"elements": elements, "checks": false})));
        return Ok(0);
    }
    for element in &elements {
        let frame: Vec<i64> = (0..4).map(|i| element["frame"][i].as_f64().unwrap_or(0.0) as i64).collect();
        let mut line = py::str(&element["type"]);
        if let Some(label) = element["label"].as_str() {
            line += &format!(" {}", py::dumps(&json!(label)));
        }
        if let Some(id) = element["id"].as_str() {
            line += &format!(" #{id}");
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
