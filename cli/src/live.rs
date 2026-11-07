//! The app's control endpoint, live: connect, stream frames, and the commands that only make sense live.
use crate::args::Args;
use crate::control::Control;
use crate::model::{self, element_alpha, element_state, load, save_recording, Frame, Recording};
use crate::py::{self, f};
use crate::state::{
    self, adb_path, capture, ctx, failure, parse_ms, read_json, write_file, Failure, Result, ANDROID_DIR, IOS_DIR,
};
use crate::{fail, say, warn};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const STILL_SCREEN_SLACK_MS: u64 = 500;
/// `state watch` reads the value this often: app values have no change signal of their own.
const STATE_POLL_MS: u64 = 50;
/// `state watch` prints this many changes, then only how many more came.
const STATE_PRINTED_CHANGES: usize = 20;
/// The comparison operators a bare `--until` such as '>100' may start with.
const QUERY_START: [&str; 6] = [">=", "<=", "!=", ">", "<", "="];

/// Offline recording on an iOS simulator: a request file and a Darwin notification.
pub struct IosDevice {
    udid: String,
    directory: PathBuf,
}

impl IosDevice {
    pub fn new(udid: &str, app: &str) -> Result<Self> {
        Ok(IosDevice { udid: udid.into(), directory: Path::new(&model::app_container(udid, app)?).join(IOS_DIR) })
    }
}

/// Offline recording on Android: a broadcast the shell may send, and run-as to fetch the file.
pub struct AndroidDevice {
    adb: String,
    serial: String,
    app: String,
    /// The app process the last request went to: a new process (an instrumentation, a restart) must be asked again.
    asked: std::cell::RefCell<String>,
}

impl AndroidDevice {
    pub fn new(serial: &str, app: &str) -> Result<Self> {
        Ok(AndroidDevice { adb: adb_path()?, serial: serial.into(), app: app.into(), asked: Default::default() })
    }

    fn pid(&self) -> String {
        self.shell(&["pidof", &self.app]).stdout.trim().to_string()
    }

    fn shell(&self, args: &[&str]) -> state::Output {
        let mut all = vec!["-s", &self.serial, "exec-out"];
        all.extend_from_slice(args);
        capture(&self.adb, &all)
    }

    fn send(&self, frames: i64) -> state::Output {
        *self.asked.borrow_mut() = self.pid();
        self.shell(&[
            "am",
            "broadcast",
            "-a",
            "dev.agentddp.RECORD",
            "-p",
            &self.app,
            "--ei",
            "frames",
            &frames.to_string(),
        ])
    }
}

pub trait OfflineDevice {
    fn request(&self, frames: i64) -> Result<()>;
    fn fetch(&self) -> Option<String>;
    /// Ask again while no recording has started: the app was not listening yet, or a new process replaced it.
    fn ask_again(&self, _frames: i64) {}
}

impl OfflineDevice for IosDevice {
    fn request(&self, frames: i64) -> Result<()> {
        let _ = std::fs::create_dir_all(&self.directory);
        let _ = std::fs::remove_file(self.directory.join("recording.jsonl"));
        write_file(&self.directory.join("request.json"), py::dumps(&json!({"frames": frames})))?;
        state::run(&["xcrun", "simctl", "spawn", &self.udid, "notifyutil", "-p", "dev.agentddp.record"])?;
        Ok(())
    }

    fn fetch(&self) -> Option<String> {
        std::fs::read_to_string(self.directory.join("recording.jsonl")).ok()
    }
}

impl OfflineDevice for AndroidDevice {
    fn request(&self, frames: i64) -> Result<()> {
        // A partial file left by a killed run would read as a recording that started.
        let (done, partial) = (format!("{ANDROID_DIR}/recording.jsonl"), format!("{ANDROID_DIR}/recording.partial"));
        self.shell(&["run-as", &self.app, "rm", "-f", &done, &partial]);
        let sent = self.send(frames);
        if sent.code != 0 {
            fail!(
                format!("broadcast failed: {}", sent.stderr.trim()),
                "broadcast_failed",
                format!("ddp doctor {} --app {}", self.serial, self.app)
            );
        }
        Ok(())
    }

    /// The app ignores a request while it records, so asking again until its file shows up is safe. A new process
    /// gets asked too, after its killed predecessor's partial file is removed.
    fn ask_again(&self, frames: i64) {
        let pid = self.pid();
        if pid.is_empty() {
            return;
        }
        if pid != *self.asked.borrow() {
            self.shell(&["run-as", &self.app, "rm", "-f", &format!("{ANDROID_DIR}/recording.partial")]);
            self.send(frames);
            return;
        }
        let listing = self.shell(&["run-as", &self.app, "ls", ANDROID_DIR]);
        if !listing.stdout.contains("recording.") {
            self.send(frames);
        }
    }

    fn fetch(&self) -> Option<String> {
        let result = self.shell(&["run-as", &self.app, "cat", &format!("{ANDROID_DIR}/recording.jsonl")]);
        (result.code == 0 && result.stdout.starts_with('{')).then_some(result.stdout)
    }
}

/// The action without a shell: no pipes, globs or variables, only a program and its arguments.
fn then_command(command: &str) -> Option<Command> {
    match state::shell_split(command) {
        Ok(words) if !words.is_empty() => {
            let mut program = Command::new(&words[0]);
            program.args(&words[1..]);
            Some(program)
        }
        Ok(_) => None,
        Err(error) => {
            warn!("ddp: --then: {}", error.message);
            None
        }
    }
}

pub fn run_then(command: &str) -> bool {
    let Some(mut program) = then_command(command) else { return false };
    match program.status() {
        Ok(status) => status.success(),
        Err(error) => {
            warn!("ddp: --then: {error}");
            false
        }
    }
}

pub fn spawn_then(command: &str) -> Option<Child> {
    then_command(command)?.spawn().map_err(|error| warn!("ddp: --then: {error}")).ok()
}

/// The saved `ddp connect` session, unless this command reads a file (offline mode).
pub fn live_session(args: &Args) -> Option<Value> {
    if args.file.is_some() {
        return None;
    }
    read_json(&ctx().session_file)
}

pub fn require_session() -> Result<Value> {
    match read_json(&ctx().session_file) {
        Some(session) if session["mode"] == "device" => fail!(
            format!("this session drives {} without the library: this command needs it", py::str(&session["app"])),
            "no_library",
            "without it: tree, tap, double-tap, long-press, swipe, drag, type, press and walk work (no checks)"
        ),
        Some(session) => Ok(session),
        None => fail!("not connected", "not_connected", "ddp connect DEVICE --app ID"),
    }
}

/// A connection to the saved session's control endpoint.
pub fn open(session: &Value) -> Result<Control> {
    Control::open(session)
}

/// The header of a recording, from `app.info`, for answers that are not a stream.
fn header(control: &mut Control, frames: u64) -> Result<Value> {
    let info = control.call("app.info", json!({}))?;
    model::check_protocol(&info)?;
    let mut header = json!({"type": "header", "protocol": model::PROTOCOL, "platform": info["platform"], "unit": info["unit"],
              "scale": info["scale"], "frames": frames, "startedAt": py::fnum(state::now_ms().round())});
    if info.get("screen").is_some_and(|screen| !screen.is_null()) {
        header["screen"] = info["screen"].clone();
    }
    Ok(header)
}

/// What is on screen now as a recording: header, one frame, end.
pub fn snapshot_messages(session: &Value) -> Result<Vec<Value>> {
    let mut control = open(session)?;
    let header = header(&mut control, 1)?;
    let frame = control.call("screen.frame", json!({}))?;
    control.close();
    Ok(vec![header, frame, json!({"type": "end", "frames": 1})])
}

fn guess_platform(device: &str) -> &'static str {
    if device.len() == 36 && device.matches('-').count() == 4 {
        "ios"
    } else {
        "android"
    }
}

pub fn platform_of(args_platform: Option<&str>, device: &str) -> String {
    args_platform.map(String::from).unwrap_or_else(|| guess_platform(device).into())
}

pub fn connect(args: &mut Args) -> Result<i32> {
    let device = args.device.clone().unwrap_or_default();
    let app = args.app.clone().unwrap_or_default();
    let platform = platform_of(args.platform.as_deref(), &device);
    let patience = args.connect_wait.as_deref().map(parse_ms).transpose()?.unwrap_or(0.0);
    let (session, snapshot) = attach_within(&platform, &device, &app, Duration::from_secs_f64(patience / 1000.0))?;
    keep_session(&session, &snapshot)
}

/// `ddp launch`: the app started fresh (a running copy is stopped first), with values for it, then connected.
/// iOS gets them as environment variables, Android as string extras of the launch intent: Android has no
/// per-launch environment for an app. Launch arguments are iOS only: an Android activity has no arguments.
pub fn launch(args: &mut Args) -> Result<i32> {
    let device = args.device.clone().unwrap_or_default();
    let app = args.app.clone().unwrap_or_default();
    let platform = platform_of(args.platform.as_deref(), &device);
    let values = args
        .env
        .iter()
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
            _ => Err(failure(
                format!("--env takes KEY=VALUE, not '{pair}'"),
                "bad_value",
                Some("--env API_URL=http://10.0.2.2:8080"),
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    if platform != "ios" && !args.launch_args.is_empty() {
        fail!(
            "an Android activity takes no launch arguments",
            "not_supported",
            "pass values as intent extras: --env KEY=VALUE"
        );
    }
    if args.reset {
        reset_data(&platform, &device, &app)?;
    }
    let started = if platform == "ios" {
        let mut command = Command::new("xcrun");
        command.args(["simctl", "launch", "--terminate-running-process", &device, &app]);
        command.args(&args.launch_args);
        command.envs(values.iter().map(|(key, value)| (format!("SIMCTL_CHILD_{key}"), value)));
        command.output()
    } else {
        let component = launcher_activity(&device, &app)?;
        let mut command = vec!["-s".to_string(), device.clone(), "shell".into(), "am start -S -W -n".into(), component];
        for (key, value) in &values {
            // adb shell joins its arguments into one line for the device's shell.
            command.extend(["--es".into(), shell_quoted(key), shell_quoted(value)]);
        }
        Command::new(adb_path()?).args(&command).output()
    }
    .map_err(|e| failure(e.to_string(), "command_failed", None))?;
    let said = String::from_utf8_lossy(&started.stdout).into_owned() + &String::from_utf8_lossy(&started.stderr);
    if !started.status.success() || said.contains("Error") {
        fail!(
            format!("could not launch {app}: {}", said.trim()),
            "launch_failed",
            format!("ddp doctor {device} --app {app}")
        );
    }
    say!("launched {app} on {platform} {device}");
    // The old run's endpoint may still be on disk: wait until the new run's answers.
    let Some((session, snapshot)) = session_after_start(&platform, &device, &app, Duration::from_secs(15))? else {
        return Ok(0);
    };
    if !values.is_empty() {
        let names: Vec<&str> = values.iter().map(|(key, _)| key.as_str()).collect();
        // The platform's answer, not ddp's: what the app process was given. A library before app.launch says nothing.
        if let Ok(answer) = open(&session).and_then(|mut control| control.call("app.launch", json!({"names": names}))) {
            let received: Vec<String> = answer["received"].as_array().into_iter().flatten().map(py::str).collect();
            let missing: Vec<&str> = names.iter().copied().filter(|name| !received.iter().any(|r| r == name)).collect();
            if !received.is_empty() {
                say!("app received {}", received.join(", "));
            }
            if !missing.is_empty() {
                warn!("ddp: warning: the app did not receive {}", missing.join(", "));
            }
        }
    }
    keep_session(&session, &snapshot)
}

/// The app's data as after a fresh install, with the app stopped: Android's own `pm clear`; on iOS the simulator's data
/// container emptied. The iOS Keychain lives outside the container and stays.
pub fn reset_data(platform: &str, device: &str, app: &str) -> Result<()> {
    if platform != "ios" {
        let cleared = capture(&adb_path()?, &["-s", device, "shell", "pm", "clear", app]);
        if !cleared.stdout.contains("Success") {
            fail!(
                format!("could not clear {app}: {}", cleared.stdout.trim()),
                "reset_failed",
                format!("ddp doctor {device} --app {app}")
            );
        }
        say!("cleared {app}'s data");
        return Ok(());
    }
    capture("xcrun", &["simctl", "terminate", device, app]);
    let container = PathBuf::from(model::app_container(device, app)?);
    // It empties a folder: only ever an app's data container on this simulator, whatever the cache says.
    let path = container.display().to_string();
    if !path.contains(&format!("/Devices/{device}/data/Containers/Data/Application/")) {
        fail!(
            format!("{path} is not {app}'s data container on {device}"),
            "reset_failed",
            format!("xcrun simctl get_app_container {device} {app} data")
        );
    }
    for entry in std::fs::read_dir(&container).map_err(|e| failure(e.to_string(), "reset_failed", None))?.flatten() {
        let path = entry.path();
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.map_err(|e| failure(format!("could not remove {}: {e}", path.display()), "reset_failed", None))?;
    }
    say!("cleared {app}'s data container (the Keychain stays)");
    Ok(())
}

/// The app's launcher activity, as the package manager resolves it (`monkey` does not start every app).
pub fn launcher_activity(device: &str, app: &str) -> Result<String> {
    let resolved = capture(
        &adb_path()?,
        &[
            "-s",
            device,
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-a",
            "android.intent.action.MAIN",
            "-c",
            "android.intent.category.LAUNCHER",
            app,
        ],
    );
    match resolved.stdout.lines().map(str::trim).rfind(|line| line.contains('/')) {
        Some(component) if resolved.code == 0 => Ok(component.to_string()),
        _ => fail!(
            format!("{app} has no launcher activity on {device}"),
            "not_installed",
            format!("ddp doctor {device} --app {app}")
        ),
    }
}

/// One argument for a POSIX shell, in single quotes.
pub fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The session for the app's control endpoint, and what is on screen, when it answers.
/// [attach], tried again until `patience` runs out: the app may still be starting.
/// How long an iOS app may take to write its endpoint after it starts; the library writes it at once.
const NO_ENDPOINT_PATIENCE: Duration = Duration::from_secs(5);

/// After the app was started: its own session when the library answers, else a session through the platform, where
/// tree, actions and walks work without checks. An Android build without the library is not waited for.
pub fn session_after_start(
    platform: &str,
    device: &str,
    app: &str,
    patience: Duration,
) -> Result<Option<(Value, Vec<Value>)>> {
    let has_library = platform == "ios" || android_has_library(device, app);
    match attach_within(platform, device, app, if has_library { patience } else { Duration::ZERO }) {
        Ok(attached) => Ok(Some(attached)),
        Err(reason) => {
            let session = json!({"platform": platform, "device": device, "app": app, "mode": "device"});
            write_file(&ctx().session_file, py::dumps(&session))?;
            say!(
                "{app} did not answer as an app with the library ({}): tree, tap, type, swipe, press and walk now go through the platform, with no checks; if it has the library, ddp connect {device} --app {app}",
                reason.message
            );
            Ok(None)
        }
    }
}

/// Whether the installed Android build has the library. Unknown (no adb) counts as yes: it is only waited for.
pub fn android_has_library(device: &str, app: &str) -> bool {
    adb_path()
        .map(|adb| {
            has_library_component(&capture(&adb, &["-s", device, "shell", "dumpsys", "package", app]).stdout, app)
        })
        .unwrap_or(true)
}

/// The library's starter in the app's own manifest, in `dumpsys package <app>`. The dump also lists other packages,
/// so "dev.agentddp" alone matches any app on a phone that has ddp's sample or runner.
pub fn has_library_component(package_dump: &str, app: &str) -> bool {
    package_dump.contains(&format!("{app}/dev.agentddp.AgentDdpStarter"))
}

/// The app's session once its library answers. An iOS app that writes no endpoint within a few seconds of its start
/// has no library: it is not waited for to the end.
pub fn attach_within(platform: &str, device: &str, app: &str, patience: Duration) -> Result<(Value, Vec<Value>)> {
    let started = Instant::now();
    let deadline = started + patience;
    loop {
        match attach(platform, device, app) {
            Ok(attached) => return Ok(attached),
            Err(failure) if Instant::now() >= deadline => return Err(failure),
            Err(failure) if failure.code == "no_endpoint" && started.elapsed() >= NO_ENDPOINT_PATIENCE => {
                return Err(failure)
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn attach(platform: &str, device: &str, app: &str) -> Result<(Value, Vec<Value>)> {
    let session = if platform == "ios" {
        let endpoint_file = Path::new(&model::app_container(device, app)?).join(IOS_DIR).join("control.json");
        let Some(endpoint) = read_json(&endpoint_file) else {
            if older_library(platform, device, app) {
                return Err(library_too_old());
            }
            fail!(
                "the app has no control endpoint",
                "no_endpoint",
                format!("run a Debug build that calls AgentDDP.start(); ddp doctor {device} --app {app}")
            );
        };
        json!({"platform": "ios", "app": app, "device": device, "port": endpoint["port"]})
    } else {
        let forwarded = capture(&adb_path()?, &["-s", device, "forward", "tcp:0", &android_socket(app)]);
        if forwarded.code != 0 {
            fail!(
                format!("adb forward failed: {}", forwarded.stderr.trim()),
                "forward_failed",
                format!("ddp doctor {device} --app {app}")
            );
        }
        let port: u64 = forwarded.stdout.trim().parse().unwrap_or(0);
        json!({"platform": "android", "app": app, "device": device, "port": port})
    };
    let snapshot = match snapshot_messages(&session) {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            if platform == "android" {
                // A retry from `launch` forwards again: leave no forward behind for the run that did not answer.
                let port = py::str(&session["port"]);
                capture(&adb_path()?, &["-s", device, "forward", "--remove", &format!("tcp:{port}")]);
            }
            return Err(if older_library(platform, device, app) { library_too_old() } else { failure });
        }
    };
    Ok((session, snapshot))
}

pub fn keep_session(session: &Value, snapshot: &[Value]) -> Result<i32> {
    write_file(&ctx().session_file, py::dumps(session))?;
    let (app, platform, device) =
        (py::str(&session["app"]), py::str(&session["platform"]), py::str(&session["device"]));
    let frame = snapshot.iter().find(|m| m["type"] == "frame").cloned().unwrap_or(json!({"elements": [], "nodes": []}));
    let count = |field: &str| frame[field].as_array().map(Vec::len).unwrap_or(0);
    say!(
        "connected to {app} on {platform} {device} (port {}): {} elements, {} nodes on screen",
        py::str(&session["port"]),
        count("elements"),
        count("nodes")
    );
    if count("elements") == 0 {
        warn!("ddp: warning: no elements on screen\nnext: ddp doctor");
    }
    Ok(0)
}

pub fn disconnect(args: &mut Args) -> Result<i32> {
    let context = ctx();
    if let Ok((platform, device, _)) = crate::device::target(args) {
        // Unreachable device or no adb: the session is still forgotten.
        if platform == "android" && crate::runner::stop(&device).unwrap_or(false) {
            say!("ddp's runner stopped on {device}: UiAutomation is free for the app's own tests");
        }
    }
    if let Some(session) = read_json(&context.session_file) {
        if session["platform"] == "android" {
            if let Ok(adb) = adb_path() {
                capture(
                    &adb,
                    &[
                        "-s",
                        &py::str(&session["device"]),
                        "forward",
                        "--remove",
                        &format!("tcp:{}", py::str(&session["port"])),
                    ],
                );
            }
        }
        let _ = std::fs::remove_file(&context.session_file);
    }
    say!("disconnected; commands read {} (or --file) again", context.last.display());
    Ok(0)
}

/// Live frames for `duration_ms` (None: until `each` says stop), running `then` once the stream starts.
/// `each` gets every message (header, frames) and returns false to stop.
pub fn stream_frames(
    session: &Value,
    duration_ms: Option<f64>,
    then: Option<&str>,
    mut each: impl FnMut(&Value) -> Result<bool>,
) -> Result<()> {
    let mut control = open(session)?;
    control.call("screen.watch", json!({"frames": 0}))?;
    // Android sends a frame only when something draws, so a still screen also ends at the wall clock.
    let deadline = duration_ms.map(|ms| Instant::now() + Duration::from_millis(ms as u64 + STILL_SCREEN_SLACK_MS));
    let mut started_then = false;
    let result = (|| {
        while let Some(event) = control.next_event(deadline)? {
            if event["method"] != "screen.stream" {
                continue;
            }
            let message = &event["params"];
            if message["type"] == "header" {
                model::check_protocol(message)?;
            }
            if message["type"] == "end" {
                return Ok(());
            }
            if !each(message)? {
                return Ok(());
            }
            if message["type"] != "frame" {
                continue;
            }
            if let Some(then) = then {
                if !started_then {
                    started_then = true;
                    if !run_then(then) {
                        warn!("ddp: warning: --then command failed");
                    }
                }
            }
            if duration_ms.is_some_and(|d| f(&message["t"]) >= d) {
                return Ok(());
            }
        }
        Ok(())
    })();
    control.close();
    result
}

/// Records the live window the command asks for into the recording file and loads it.
pub fn record_live(args: &Args) -> Result<Recording> {
    record(args, false)
}

/// For the checks: the take, then the platform's answers at its end (`inspect`: hit test, screenshot), kept in it.
pub fn record_live_inspected(args: &Args) -> Result<Recording> {
    record(args, true)
}

fn record(args: &Args, inspected: bool) -> Result<Recording> {
    let session = live_session(args).ok_or_else(|| failure("not connected", "not_connected", None))?;
    let duration = match &args.duration {
        Some(text) => parse_ms(text)?,
        None => args.frames.unwrap_or(120) as f64 * 1000.0 / 60.0,
    };
    let mut lines = Vec::new();
    stream_frames(&session, Some(duration), args.then.as_deref(), |message| {
        lines.push(py::dumps(message));
        Ok(true)
    })?;
    if inspected {
        lines.push(py::dumps(&crate::inspect::inspect(&session)?));
    }
    save_recording(&(lines.join("\n") + "\n"), args.name.as_deref())?;
    load(&ctx().last)
}

/// One frame of what is on screen now. Not kept in the history: it is not a take.
pub fn snapshot_live(session: &Value) -> Result<Recording> {
    let messages = snapshot_messages(session)?;
    let text = messages.iter().map(py::dumps).collect::<Vec<_>>().join("\n") + "\n";
    write_file(&ctx().last, text)?;
    load(&ctx().last)
}

/// Live: record the window the command asks for. Offline: load --file (default .ddp/last.jsonl).
pub fn recording_for(args: &Args) -> Result<Recording> {
    if live_session(args).is_some() {
        return record_live(args);
    }
    load(args.file.as_deref().unwrap_or(&ctx().last))
}

/// For the checks: like [recording_for], with the platform's answers at the end of a live take.
pub fn recording_for_checks(args: &Args) -> Result<Recording> {
    let recording = if live_session(args).is_some() {
        record_live_inspected(args)?
    } else {
        load(args.file.as_deref().unwrap_or(&ctx().last))?
    };
    crate::inspect::say_hooks(recording.inspect["hooks"].as_array().map(Vec::as_slice).unwrap_or_default());
    Ok(recording)
}

const WATCH_FIELDS: [&str; 11] =
    ["x", "y", "w", "h", "alpha", "fill", "border", "borderColor", "opacity", "draws", "pixels"];

pub fn watch(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let id = args.id.clone().unwrap_or_default();
    let clauses = args.until.as_deref().map(model::parse_where).transpose()?;
    let duration = args.duration.as_deref().map(parse_ms).transpose()?;
    let mut tolerance = state::OWNER_TOLERANCE_PT;
    let mut previous: Option<serde_json::Map<String, Value>> = None;
    stream_frames(&session, duration, args.then.as_deref(), |message| {
        if message["type"] == "header" {
            tolerance = model::tolerance_for(message);
            say!("watching {id} live, unit {} (only changes are printed)", py::str(&message["unit"]));
            return Ok(true);
        }
        if message["type"] != "frame" {
            return Ok(true);
        }
        let frame = Frame::from(message.clone(), tolerance);
        let state = watch_state(&frame, &id);
        let t = py::fixed(&message["t"], 0);
        match &previous {
            None => {
                let present = state["present"] == json!(true);
                let parts: Vec<String> = state
                    .iter()
                    .filter(|(_, v)| !v.is_null() || present)
                    .map(|(k, v)| format!("{k} {}", py::str(v)))
                    .collect();
                say!("t={t}ms {}", parts.join("; "));
            }
            Some(before) => {
                let changes: Vec<String> = state
                    .iter()
                    .filter(|(k, v)| !py::equal(before.get(*k).unwrap_or(&Value::Null), v))
                    .map(|(k, v)| format!("{k} {} -> {}", py::str(before.get(k).unwrap_or(&Value::Null)), py::str(v)))
                    .collect();
                if !changes.is_empty() {
                    say!("t={t}ms {}", changes.join("; "));
                }
            }
        }
        let mut fields = state.clone();
        fields.insert("t".into(), message["t"].clone());
        previous = Some(state);
        if let Some(clauses) = &clauses {
            if model::matches(&fields, clauses)? {
                say!("until met at t={t}ms");
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    Ok(0)
}

fn watch_state(frame: &Frame, key: &str) -> serde_json::Map<String, Value> {
    let mut state = serde_json::Map::new();
    match element_state(frame, key) {
        None => {
            // Every field, empty: `--until 'alpha>0.9'` is simply false until the element shows up.
            state.insert("present".into(), json!(false));
            for field in WATCH_FIELDS {
                state.insert(field.into(), Value::Null);
            }
        }
        Some(s) => {
            state.insert("present".into(), json!(true));
            for (index, name) in ["x", "y", "w", "h"].into_iter().enumerate() {
                state.insert(name.into(), s["frame"][index].clone());
            }
            state.insert("alpha".into(), py::round(&element_alpha(frame, key), 2));
            for field in ["fill", "border", "borderColor", "opacity", "draws", "pixels"] {
                state.insert(field.into(), s[field].clone());
            }
        }
    }
    state
}

pub fn evaluate(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    let answer = match &args.set_value {
        // A number or true/false goes as JSON, a color as its text.
        Some(text) => {
            let value = serde_json::from_str(text).unwrap_or(Value::String(text.clone()));
            control.call("screen.setValue", json!({"id": args.id, "path": args.path, "value": value}))?
        }
        None => control.call("screen.value", json!({"id": args.id, "path": args.path}))?,
    };
    say!("{}", py::dumps(answer.get("value").unwrap_or(&Value::Null)));
    Ok(0)
}

pub fn net(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    match args.action.as_deref() {
        Some("cookies") => {
            let answer = control.call("net.cookies", json!({}))?;
            if ctx().json {
                say!("{}", py::dumps(&answer));
            } else {
                let cookies = answer["cookies"].as_array().cloned().unwrap_or_default();
                for cookie in &cookies {
                    let expires =
                        if cookie["expires"].is_null() { "session".to_string() } else { py::str(&cookie["expires"]) };
                    let secure = if cookie["secure"] == true { " secure" } else { "" };
                    say!(
                        "{} {}{} (expires {expires}{secure})",
                        py::str(&cookie["domain"]),
                        py::str(&cookie["name"]),
                        py::str(&cookie["path"])
                    );
                }
                match answer["unread"].as_str() {
                    Some(why) => say!("not read: {why}"),
                    None if cookies.is_empty() => say!("no cookies"),
                    None => {}
                }
            }
        }
        Some("clear-cookies") => {
            let answer = control.call("net.clearCookies", json!({}))?;
            say!("removed {} cookies", py::str(&answer["removed"]));
        }
        Some("get") => {
            let Some(id) = &args.request_id else {
                fail!("net get needs a request id", "missing_option", "ddp net list");
            };
            say!("{}", py::dumps_pretty(&control.call("net.get", json!({"id": id}))?));
        }
        Some("send") => {
            let Some(url) = &args.request_id else {
                fail!("net send needs a URL", "missing_option", "ddp net send https://example.com/api");
            };
            let mut params = json!({"url": url, "method": args.method.as_deref().unwrap_or("GET")});
            if let Some(body) = &args.body {
                params["body"] = json!(body);
            }
            let answer = control.call("net.send", params)?;
            say!("{}", py::str(&answer["status"]));
            if let Some(body) = answer["body"].as_str().filter(|b| !b.is_empty()) {
                say!("{body}");
            }
        }
        Some(action @ ("mock" | "fail")) => answer_paused(&mut control, args, action)?,
        Some("list") => {
            let answer = control.call("net.list", json!({}))?;
            let requests = answer["requests"].as_array().cloned().unwrap_or_default();
            if requests.is_empty() {
                say!("no requests yet (the app reports them from its Debug network decorator)");
            }
            for request in &requests {
                say!("{}", net_line(request));
            }
        }
        _ => {
            let clauses = args.until.as_deref().map(model::parse_where).transpose()?;
            control.call("net.watch", json!({}))?;
            say!("watching requests live");
            let result = (|| {
                while let Some(event) = control.next_event(None)? {
                    if event["method"] != "net.request" {
                        continue;
                    }
                    let request = &event["params"];
                    say!("{}", net_line(request));
                    if let (Some(clauses), Some(fields)) = (&clauses, request.as_object()) {
                        if model::matches(fields, clauses)? {
                            say!("until met: {}", py::str(&request["id"]));
                            return Ok(());
                        }
                    }
                }
                Ok(())
            })();
            result?;
        }
    }
    control.close();
    Ok(0)
}

/// Holds the requests that match the pattern for `--for` (default 10s) and answers each one itself: `mock` with
/// `--status` and `--body`, `fail` with the `--error` the app's client would throw. `--then` runs meanwhile.
fn answer_paused(control: &mut Control, args: &Args, action: &str) -> Result<()> {
    let Some(pattern) = &args.request_id else {
        fail!(
            format!("net {action} needs a URL pattern"),
            "missing_option",
            "ddp net list, then a pattern like '*/api/*'"
        );
    };
    let (method, answer, outcome) = if action == "mock" {
        let status = args.status.unwrap_or(200);
        let body = args.body.clone().unwrap_or_default();
        ("net.respond", json!({"status": status, "body": body}), status.to_string())
    } else {
        let error = args.error.clone().unwrap_or_else(|| "offline".into());
        ("net.fail", json!({"error": error}), error)
    };
    let duration = parse_ms(args.duration.as_deref().unwrap_or("10s"))?;
    control.call("net.intercept", json!({"urls": [pattern]}))?;
    say!("{action} {pattern} -> {outcome} for {}ms", py::fixed(&py::fnum(duration), 0));
    // Spawned, not waited for: the action may wait on a request only this loop can answer.
    let mut then = args.then.as_deref().and_then(spawn_then);
    let deadline = Instant::now() + Duration::from_millis(duration as u64);
    let mut answered = 0;
    while let Some(event) = control.next_event(Some(deadline))? {
        if event["method"] != "net.paused" {
            continue;
        }
        let mut params = answer.clone();
        params["id"] = event["params"]["id"].clone();
        control.call(method, params)?;
        answered += 1;
        let request = &event["params"]["request"];
        say!("{} {} -> {outcome}", py::str(&request["method"]), py::str(&request["url"]));
        if args.limit.is_some_and(|limit| answered >= limit) {
            break;
        }
    }
    control.call("net.release", json!({}))?;
    if let Some(child) = then.as_mut() {
        if !child.wait().is_ok_and(|status| status.success()) {
            warn!("ddp: warning: --then command failed");
        }
    }
    say!("{answered} {} answered", if answered == 1 { "request" } else { "requests" });
    Ok(())
}

/// The app's preferences: UserDefaults on iOS, SharedPreferences on Android. `--in` picks a suite or file.
pub fn prefs(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    let file = args.name.clone().map(Value::from).unwrap_or(Value::Null);
    let key = || match &args.key {
        Some(key) => Ok(key.clone()),
        None => Err(failure("which key?", "missing_option", Some("ddp prefs list"))),
    };
    match args.action.as_deref().unwrap_or("list") {
        "files" => {
            for name in control.call("prefs.files", json!({}))?["files"].as_array().into_iter().flatten() {
                say!("{}", py::str(name));
            }
        }
        "set" => {
            let (key, value) = (key()?, args.value.clone().unwrap_or_default());
            control.call("prefs.write", json!({"file": file, "key": key, "value": value}))?;
            say!("set {key}");
        }
        "rm" => {
            let key = key()?;
            control.call("prefs.remove", json!({"file": file, "key": key}))?;
            say!("removed {key}");
        }
        "clear" => {
            control.call("prefs.clear", json!({"file": file}))?;
            say!("cleared");
        }
        _ => {
            let answer = control.call("prefs.read", json!({"file": file}))?;
            let mut values: Vec<(&String, &Value)> = answer["values"].as_object().into_iter().flatten().collect();
            values.sort_by(|a, b| a.0.cmp(b.0));
            if values.is_empty() {
                say!("no values");
            }
            for (key, value) in values {
                say!("{key} = {}", py::str(value));
            }
        }
    }
    control.close();
    Ok(0)
}

/// Values the app added in Debug (`AgentDDP.state`, `AgentDdpState.add`): list, read, watch, and set the writable ones.
pub fn app_state(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    let name = || match &args.key {
        Some(name) => Ok(name.clone()),
        None => Err(failure("which value?", "missing_option", Some("ddp state list"))),
    };
    let code = match args.action.as_deref().unwrap_or("list") {
        "get" => {
            let name = name()?;
            let values = control.call("state.get", json!({"names": [name]}))?;
            if ctx().json {
                say!("{}", py::dumps(&values["values"][&name]));
            } else {
                say!("{name} = {}", py::str(&values["values"][&name]));
            }
            0
        }
        "set" => {
            let name = name()?;
            let text = args.value.clone().unwrap_or_default();
            // 500, true and {"a": 1} keep their JSON type; anything that is not JSON is text.
            let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
            let answer = control.call("state.set", json!({"name": name, "value": value}))?;
            say!("{name} = {}", py::str(&answer["value"]));
            0
        }
        "watch" => watch_state_values(&mut control, &name()?, args)?,
        _ => {
            let listed = control.call("state.list", json!({}))?;
            let entries = listed["values"].as_array().cloned().unwrap_or_default();
            if ctx().json {
                let values = control.call("state.get", json!({}))?;
                let rows: Vec<Value> = entries
                    .iter()
                    .map(|entry| json!({"name": entry["name"], "writable": entry["writable"], "value": values["values"][py::str(&entry["name"])]}))
                    .collect();
                say!("{}", py::dumps(&Value::Array(rows)));
            } else if entries.is_empty() {
                say!("no values: the app adds them in Debug with AgentDDP.state (iOS) or AgentDdpState.add (Android)");
                say!("next: ddp skills get app");
            } else {
                let values = control.call("state.get", json!({}))?;
                for entry in entries {
                    let name = py::str(&entry["name"]);
                    let writable = if entry["writable"] == json!(true) { " (writable)" } else { "" };
                    say!("{name} = {}{writable}", py::str(&values["values"][&name]));
                }
            }
            0
        }
    };
    control.close();
    Ok(code)
}

/// Reads the value until `--until` holds or `--for` ends, printing each change. 1 when `--until` was never met.
fn watch_state_values(control: &mut Control, name: &str, args: &Args) -> Result<i32> {
    // '>100' is about the watched value itself.
    let until = args.until.as_deref().map(|text| {
        let bare = QUERY_START.iter().any(|operator| text.trim_start().starts_with(operator));
        if bare {
            format!("{name}{}", text.trim_start())
        } else {
            text.to_string()
        }
    });
    let clauses = until.as_deref().map(model::parse_where).transpose()?;
    let duration = parse_ms(args.duration.as_deref().unwrap_or("10s"))?;
    let started = Instant::now();
    let mut previous: Option<Value> = None;
    // A value that changes on every read would print a line every 50 ms: past this many, only the count.
    let (mut changes, mut unprinted) = (0usize, 0usize);
    while (started.elapsed().as_millis() as f64) < duration {
        let values = control.call("state.get", json!({"names": [name]}))?;
        let value = values["values"][name].clone();
        let t = started.elapsed().as_millis();
        match &previous {
            None => say!("t={t}ms {name} = {}", py::str(&value)),
            Some(before) if !py::equal(before, &value) => {
                changes += 1;
                if changes <= STATE_PRINTED_CHANGES {
                    say!("t={t}ms {name} {} -> {}", py::str(before), py::str(&value));
                } else {
                    unprinted += 1;
                }
            }
            Some(_) => {}
        }
        previous = Some(value);
        if let Some(clauses) = &clauses {
            let fields = values["values"].as_object().cloned().unwrap_or_default();
            if model::matches(&fields, clauses)? {
                if unprinted > 0 {
                    say!("... {unprinted} more changes, now {}", py::str(previous.as_ref().unwrap_or(&Value::Null)));
                }
                say!("until met at t={t}ms");
                return Ok(0);
            }
        }
        std::thread::sleep(Duration::from_millis(STATE_POLL_MS));
    }
    if unprinted > 0 {
        say!("... {unprinted} more changes, now {}", py::str(previous.as_ref().unwrap_or(&Value::Null)));
    }
    if let Some(until) = &until {
        say!("until not met in {}: {until}", args.duration.as_deref().unwrap_or("10s"));
        return Ok(1);
    }
    Ok(0)
}

/// Runs a command the app added in Debug (`AgentDDP.command`, `AgentDdpCommands.add`); without a name, lists them.
pub fn run_command(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    let Some(name) = &args.name else {
        let names = control.call("app.commands", json!({}))?;
        let names: Vec<String> = names["commands"].as_array().into_iter().flatten().map(py::str).collect();
        say!("{}", if names.is_empty() { "the app added no commands".to_string() } else { names.join("\n") });
        return Ok(0);
    };
    let params: Value = match &args.value {
        Some(text) => serde_json::from_str(text).map_err(|e| {
            failure(format!("params are not JSON ({e})"), "bad_value", Some("ddp run NAME '{\"user\": \"test\"}'"))
        })?,
        None => json!({}),
    };
    let answer = control.call("app.run", json!({"name": name, "params": params}))?;
    say!("{}", py::dumps(&answer["value"]));
    Ok(0)
}

/// The app's log lines as they come: the last 100, then each new one, for `--for` or until stopped.
pub fn follow_logs(args: &Args) -> Result<i32> {
    let session = require_session()?;
    let duration = args.duration.as_deref().map(parse_ms).transpose()?;
    let deadline = duration.map(|ms| Instant::now() + Duration::from_millis(ms as u64));
    let grep = args.grep.as_deref().map(str::to_lowercase);
    let mut control = open(&session)?;
    control.call("log.watch", json!({}))?;
    while let Some(event) = control.next_event(deadline)? {
        if event["method"] != "log.entry" {
            continue;
        }
        let entry = &event["params"];
        let text = py::str(&entry["text"]);
        if grep.as_ref().is_some_and(|grep| !text.to_lowercase().contains(grep)) {
            continue;
        }
        let time = state::local_time("%H:%M:%S", f(&entry["t"]) / 1000.0);
        let source = if args.source {
            let parts: Vec<String> =
                ["subsystem", "sender"].iter().filter_map(|key| entry.get(*key).map(py::str)).collect();
            if parts.is_empty() {
                String::new()
            } else {
                format!(" [{}]", parts.join(" "))
            }
        } else {
            String::new()
        };
        say!("{time} {} {}{source} {text}", py::str(&entry["level"]), py::str(&entry["tag"]));
    }
    control.close();
    Ok(0)
}

/// The app's files, SQLite databases (read-only) and Keychain/Keystore names.
pub fn storage(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    let path = args.key.clone().unwrap_or_default();
    let needs_path = || {
        if path.is_empty() {
            Err(failure("which file?", "missing_option", Some("ddp storage files")))
        } else {
            Ok(path.clone())
        }
    };
    if ctx().json && args.action.as_deref() != Some("rm") {
        let action = args.action.clone().unwrap_or_else(|| "files".into());
        let params = match action.as_str() {
            "read" => json!({"path": needs_path()?}),
            "query" => json!({"path": needs_path()?, "sql": args.value.clone().unwrap_or_default()}),
            "keys" => json!({}),
            _ => json!({"path": path}),
        };
        say!("{}", py::dumps(&control.call(&format!("storage.{action}"), params)?));
        control.close();
        return Ok(0);
    }
    match args.action.as_deref().unwrap_or("files") {
        "read" => {
            let answer = control.call("storage.read", json!({"path": needs_path()?}))?;
            match answer["text"].as_str() {
                Some(text) => {
                    let limit = args.limit.unwrap_or(50);
                    let lines: Vec<&str> = text.lines().collect();
                    lines.iter().take(limit).for_each(|line| say!("{line}"));
                    if lines.len() > limit || answer["truncated"] == true {
                        say!(
                            "... {} lines in all, {} bytes: --limit N for more",
                            lines.len(),
                            py::str(&answer["bytes"])
                        );
                    }
                }
                None => say!("{path}: {} bytes, not text", py::str(&answer["bytes"])),
            }
        }
        "rm" => {
            control.call("storage.remove", json!({"path": needs_path()?}))?;
            say!("removed {path}");
        }
        "query" => {
            let sql = args.value.clone().unwrap_or_default();
            let answer = control.call("storage.query", json!({"path": needs_path()?, "sql": sql}))?;
            let columns: Vec<String> = answer["columns"].as_array().into_iter().flatten().map(py::str).collect();
            say!("{}", columns.join(" | "));
            let rows = answer["rows"].as_array().cloned().unwrap_or_default();
            let limit = args.limit.unwrap_or(50);
            for row in rows.iter().take(limit) {
                let cells: Vec<String> = row.as_array().into_iter().flatten().map(py::str).collect();
                say!("{}", cells.join(" | "));
            }
            if rows.len() > limit {
                say!("... {} more rows: --limit N, or LIMIT/OFFSET in the query", rows.len() - limit);
            } else if answer["more"] == true {
                say!("... more rows: add LIMIT/OFFSET to see them");
            }
        }
        "keys" => {
            let answer = control.call("storage.keys", json!({}))?;
            let keys = answer["keys"].as_array().cloned().unwrap_or_default();
            if let Some(unread) = answer["unread"].as_str() {
                say!("not all read: {unread}");
            } else if keys.is_empty() {
                say!("no keys");
            }
            for key in keys {
                let service = key
                    .get("service")
                    .map(py::str)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("{s} / "))
                    .unwrap_or_default();
                say!("{} {service}{}", py::str(&key["store"]), py::str(&key["name"]));
            }
        }
        _ => {
            let answer = control.call("storage.files", json!({"path": path}))?;
            for entry in answer["entries"].as_array().into_iter().flatten() {
                let name = py::str(&entry["name"]);
                if entry["folder"] == true {
                    say!("{name}/");
                } else {
                    say!("{name}  {} bytes", py::str(&entry["bytes"]));
                }
            }
        }
    }
    control.close();
    Ok(0)
}

/// Players the platform shows the library, and the now-playing info the app gave the system.
/// `ddp source`: the composable calls above an element, innermost first, each with where it is called from, then the
/// element's modifiers in chain order. Calls inside androidx libraries are counted, shown with --full.
pub fn source(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let id = args.id.clone().unwrap_or_default();
    let mut answer = open(&session)?.call("screen.source", json!({"id": id}))?;
    if session["platform"] == "ios" && answer["status"] == "ok" {
        swiftui_bodies(&session, &mut answer);
    }
    if answer["status"] == "uikit" {
        uikit_creation(&session, &mut answer)?;
    }
    if ctx().json {
        say!("{}", py::dumps(&answer));
        return Ok(0);
    }
    if answer["status"] != "ok" {
        say!("{id}: no source information: {}", py::str(&answer["why"]));
        return Ok(0);
    }
    if answer.get("created").is_some() {
        say!("{id}: a {} created in", py::str(&answer["class"]));
        for frame in answer["created"].as_array().into_iter().flatten().take(6) {
            say!("  {}  {}:{}", py::str(&frame["function"]), py::str(&frame["file"]), py::str(&frame["line"]));
        }
        return Ok(0);
    }
    if session["platform"] == "ios" {
        return say_swiftui_source(&id, &answer);
    }
    let calls = answer["calls"].as_array().cloned().unwrap_or_default();
    let (shown, hidden): (Vec<&Value>, Vec<&Value>) =
        calls.iter().partition(|call| args.full || (call.get("library").is_none() && call.get("line").is_some()));
    say!("{id}: called from");
    let limit = 12;
    for call in shown.iter().take(limit) {
        let place = match (call.get("file"), call.get("line")) {
            (Some(file), Some(line)) => format!("{}:{}", py::str(file), py::str(line)),
            _ => "(no position)".to_string(),
        };
        let library = call.get("library").map(|name| format!("  ({})", py::str(name))).unwrap_or_default();
        say!("  {}  {place}{library}", py::str(&call["name"]));
    }
    if shown.len() > limit {
        say!("  ... {} more", shown.len() - limit);
    }
    if !hidden.is_empty() {
        let calls = if hidden.len() == 1 { "call" } else { "calls" };
        say!("  ({} {calls} inside androidx libraries: --full)", hidden.len());
    }
    let modifiers: Vec<String> = answer["modifiers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|modifier| {
            let values: Vec<String> = modifier
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| *key != "name" && *key != "inspector")
                .map(|(key, value)| format!("{key}={}", py::str(value)))
                .collect();
            if values.is_empty() {
                py::str(&modifier["name"])
            } else {
                format!("{}({})", py::str(&modifier["name"]), values.join(", "))
            }
        })
        .collect();
    if !modifiers.is_empty() {
        say!("modifiers: {}", modifiers.join(" > "));
    }
    Ok(0)
}

/// `ddp recompositions`: the composables whose bodies ran in the window (Compose skips the rest), most first, with
/// the state objects that made Compose run each again. Counts and causes only: whether a run was needed is the app's.
pub fn recompositions(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    if session["platform"] == "ios" {
        fail!(
            "SwiftUI keeps no runtime record of which view bodies ran",
            "not_supported",
            "in a view, `let _ = Self._printChanges()` in body prints why it ran; ddp audits shows SwiftUI's warnings"
        );
    }
    let ms = parse_ms(args.duration.as_deref().unwrap_or("3s"))?;
    let mut control = open(&session)?;
    control.call("compose.recompositions.start", json!({"ms": ms}))?;
    let mut then = args.then.as_deref().and_then(spawn_then);
    std::thread::sleep(Duration::from_secs_f64(ms / 1000.0));
    let answer = control.call("compose.recompositions.stop", json!({}))?;
    control.close();
    if let Some(child) = then.as_mut() {
        let _ = child.wait();
    }
    if ctx().json {
        say!("{}", py::dumps(&answer));
        return Ok(0);
    }
    let all = answer["composables"].as_array().cloned().unwrap_or_default();
    let seconds = py::fixed(&json!(f(&answer["ms"]) / 1000.0), 1);
    if all.is_empty() {
        say!("{seconds}s: no composable ran (nothing on screen changed state)");
        return Ok(0);
    }
    // The compiler's name starts with the package, so a composable from inside androidx says so itself.
    let (composables, inside): (Vec<&Value>, Vec<&Value>) =
        all.iter().partition(|c| args.full || !py::str(&c["name"]).starts_with("androidx."));
    let library = if inside.is_empty() {
        String::new()
    } else {
        format!(" ({} more inside androidx libraries: --full)", inside.len())
    };
    let whose = if args.full { "" } else { " of the app" };
    say!("{seconds}s: {} composables{whose} ran{library}", composables.len());
    let limit = 20;
    for composable in composables.iter().take(limit) {
        let causes: Vec<String> = composable["invalidatedBy"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|cause| format!("{} x{}", py::str(&cause["state"]), py::str(&cause["times"])))
            .collect();
        let caused = if causes.is_empty() { String::new() } else { format!(", invalidated by {}", causes.join(", ")) };
        say!("  {}  {} runs{caused}", py::str(&composable["name"]), py::str(&composable["runs"]));
    }
    if composables.len() > limit {
        say!("  ... {} more", composables.len() - limit);
    }
    Ok(0)
}

/// The app's view types above a SwiftUI element (innermost first) get their `body` place and the modifier calls in it,
/// from the app's debug image. A type whose body is not in the image (a framework's view) is left out.
fn swiftui_bodies(session: &Value, answer: &mut Value) {
    let roles: Vec<(String, String)> = answer["views"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((swift_type(v["type"].as_str()?), py::str(&v["role"]))))
        .collect();
    let types: Vec<String> = roles.iter().map(|(view, _)| view.clone()).collect();
    let image = crate::swift_source::app_image(&py::str(&session["device"]), &py::str(&session["app"]));
    if types.is_empty() {
        return;
    }
    let Some(image) = image else {
        answer["views"] = json!([]);
        answer["why"] = json!("the app's Debug image was not found in the simulator");
        return;
    };
    let bodies = crate::swift_source::bodies(&image, &types);
    let views: Vec<Value> = roles
        .iter()
        .filter_map(|(view, role)| {
            let body = bodies.get(view)?;
            let calls: Vec<Value> = body.calls.iter().map(|(name, line)| json!({"name": name, "line": line})).collect();
            Some(json!({"type": view, "role": role, "file": body.file, "line": body.line, "calls": calls}))
        })
        .collect();
    answer["views"] = json!(views);
}

/// `Module.Row<Content>` names its body getter `Module.Row.body.getter`.
fn swift_type(name: &str) -> String {
    name.split('<').next().unwrap_or(name).to_string()
}

/// A UIKit view: the app's frames of its allocation stack, from the stack log the app was started with.
fn uikit_creation(session: &Value, answer: &mut Value) -> Result<()> {
    let (device, app) = (py::str(&session["device"]), py::str(&session["app"]));
    let pid = crate::process::ios_pid(&device, &app)?;
    let image = crate::swift_source::app_image(&device, &app);
    let name = image.as_ref().and_then(|i| i.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let created = crate::swift_source::creation(&pid, &py::str(&answer["address"]), &name);
    if created.is_empty() {
        *answer = json!({"status": "none", "why": format!("the stack log has no frame of the app's code for this {}", py::str(&answer["class"]))});
        return Ok(());
    }
    let frames: Vec<Value> = created
        .iter()
        .map(|(function, file, line)| json!({"function": function, "file": file, "line": line}))
        .collect();
    *answer = json!({"status": "ok", "class": answer["class"].clone(), "created": frames});
    Ok(())
}

fn say_swiftui_source(id: &str, answer: &Value) -> Result<i32> {
    let views = answer["views"].as_array().cloned().unwrap_or_default();
    say!("{id}: a {}", py::str(&answer["element"]));
    if views.is_empty() {
        say!("  no view of the app's own around it with a body in the app's Debug image");
    }
    for view in views.iter().take(6) {
        let short = py::str(&view["type"]);
        let short = short.split_once('.').map(|(_, rest)| rest.to_string()).unwrap_or(short);
        let role = if view["role"] == "writes" { "written in" } else { "inside" };
        say!("  {role} {short}.body  {}:{}", py::str(&view["file"]), py::str(&view["line"]));
        let calls: Vec<String> = view["calls"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| format!("{} :{}", py::str(&c["name"]), py::str(&c["line"])))
            .collect();
        if !calls.is_empty() {
            say!("    modifier calls in it: {}", calls.join(", "));
        }
    }
    let modifiers: Vec<String> = answer["modifiers"].as_array().into_iter().flatten().map(py::str).collect();
    if !modifiers.is_empty() {
        say!("modifiers around it (SwiftUI's types, innermost first): {}", modifiers.join(" > "));
    }
    Ok(0)
}

pub fn animations(_args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let answer = open(&session)?.call("screen.animations", json!({}))?;
    if ctx().json {
        say!("{}", py::dumps(&answer));
        return Ok(0);
    }
    let animations = answer["animations"].as_array().cloned().unwrap_or_default();
    let shown = 30;
    for animation in animations.iter().take(shown) {
        let owner = animation.get("id").map(py::str).unwrap_or_else(|| "layer".to_string());
        let key_path = animation.get("keyPath").map(|path| format!(" {}", py::str(path))).unwrap_or_default();
        let repeat = animation.get("repeat").map(|count| format!(" repeat {}", py::str(count))).unwrap_or_default();
        let speed = animation.get("speed").map(|speed| format!(" speed {}", py::str(speed))).unwrap_or_default();
        say!(
            "{owner}{key_path} {} {}s{repeat}{speed} {}",
            py::str(&animation["type"]),
            py::str(&animation["duration"]),
            py::str(&animation["frame"])
        );
    }
    if animations.len() > shown {
        say!("... {} more", animations.len() - shown);
    }
    if animations.is_empty() {
        say!(
            "no Core Animation animation on screen; SwiftUI draws most of its animations itself: ddp record shows them"
        );
    }
    Ok(0)
}

pub fn media(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = crate::device::target(args)?;
    let session = read_json(&ctx().session_file).unwrap_or(json!({}));
    let library = session["mode"] != "device" && session["device"].as_str() == Some(device.as_str());
    if !library && platform == "ios" {
        require_session()?;
    }
    let mut answer = if library { open(&session)?.call("media.list", json!({}))? } else { json!({"players": []}) };
    // Android's library sees no Media3 or MediaPlayer player, but the system lists the media session the app gave it,
    // library or not. An adb that fails leaves it out: unknown, never "nothing plays".
    if let (true, Some(app)) = (platform == "android", app) {
        let dump = capture(&adb_path()?, &["-s", &device, "shell", "dumpsys", "media_session"]);
        if let Some(session) = (dump.code == 0).then(|| media_session(&dump.stdout, &app)).flatten() {
            answer["nowPlaying"] = session;
        }
    }
    // iOS hands the now-playing info over as a dictionary, in no order: the same answer prints the same way.
    if let Some(now) = answer.get_mut("nowPlaying").and_then(Value::as_object_mut) {
        let order = ["source", "state", "title", "artist", "album", "description", "position", "duration", "rate"];
        let mut fields: Vec<(String, Value)> = std::mem::take(now).into_iter().collect();
        fields
            .sort_by_key(|(key, _)| (order.iter().position(|known| known == key).unwrap_or(order.len()), key.clone()));
        now.extend(fields);
    }
    if ctx().json {
        say!("{}", py::dumps(&answer));
        return Ok(0);
    }
    let time = |value: &Value| if value.is_null() { "?".to_string() } else { format!("{}s", py::str(value)) };
    let players = answer["players"].as_array().cloned().unwrap_or_default();
    for player in &players {
        let rate = player.get("rate").map(|rate| format!(" rate {}", py::str(rate))).unwrap_or_default();
        say!(
            "{} {} at {} of {}{rate} {}",
            py::str(&player["source"]),
            py::str(&player["state"]),
            time(&player["position"]),
            time(&player["duration"]),
            py::str(&player["frame"])
        );
    }
    if let Some(now) = answer.get("nowPlaying") {
        let fields: Vec<String> =
            now.as_object().into_iter().flatten().map(|(key, value)| format!("{key}={}", py::str(value))).collect();
        say!("now playing: {}", fields.join(" "));
    }
    if players.is_empty() && answer.get("nowPlaying").is_none() {
        say!("no player the platform shows: an audio player or a Media3 one with no media session is shown as an app value (ddp skills get app)");
    }
    Ok(0)
}

/// The app's first active media session in the system's own order (`dumpsys media_session`), what the lock screen and
/// the media controls show: `state` and `rate` as the app last set them, `position` in seconds at that moment, and
/// the system's `description` (title, subtitle, description) as it prints it, never split on its commas.
fn media_session(dump: &str, app: &str) -> Option<Value> {
    let mut sessions: Vec<Vec<&str>> = Vec::new();
    for line in dump.lines().map(str::trim) {
        if line.starts_with("package=") {
            sessions.push(Vec::new());
        }
        if let Some(fields) = sessions.last_mut() {
            fields.push(line);
        }
    }
    let field =
        |fields: &[&str], name: &str| fields.iter().find_map(|line| line.strip_prefix(name)).map(str::to_string);
    let found = sessions.into_iter().find(|fields| {
        field(fields, "package=").as_deref() == Some(app) && field(fields, "active=").as_deref() == Some("true")
    })?;
    // A whole number prints as the libraries print it: 0, not 0.0.
    let number = |value: f64| if value.fract() == 0.0 { json!(value as i64) } else { py::fnum(value) };
    let mut entry = json!({"source": "media session"});
    if let Some(state) =
        field(&found, "state=PlaybackState {").and_then(|state| state.strip_suffix('}').map(String::from))
    {
        let values: Vec<(&str, &str)> = state.split(", ").filter_map(|pair| pair.split_once('=')).collect();
        let value = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
        // Android 14 and later print `PLAYING(3)`, before that `3`: the number is PlaybackState's own constant.
        let code =
            value("state").map(|state| state.rsplit_once('(').map_or(state, |(_, code)| code.trim_end_matches(')')));
        let names = [
            "none",
            "stopped",
            "paused",
            "playing",
            "fast forwarding",
            "rewinding",
            "buffering",
            "error",
            "connecting",
            "skipping to previous",
            "skipping to next",
            "skipping to queue item",
        ];
        if let Some(name) = code.and_then(|code| code.parse::<usize>().ok()).and_then(|code| names.get(code)) {
            entry["state"] = json!(name);
        }
        if let Some(position) = value("position").and_then(|ms| ms.parse::<f64>().ok()) {
            entry["position"] = number(position / 1000.0);
        }
        if let Some(rate) = value("speed").and_then(|speed| speed.parse::<f64>().ok()) {
            entry["rate"] = number(rate);
        }
    }
    if let Some(description) = field(&found, "metadata: ")
        .and_then(|metadata| metadata.split_once("description=").map(|(_, text)| text.to_string()))
    {
        entry["description"] = json!(description);
    }
    Some(entry)
}

/// How the app runs for `--for`, from the platform's own clocks. A measurement: it exits 0 whatever it finds.
pub fn perf(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let ms = parse_ms(args.duration.as_deref().unwrap_or("5s"))?;
    let mut control = open(&session)?;
    let answer = control.call("perf.metrics", json!({"ms": ms}))?;
    control.close();
    if ctx().json {
        say!("{}", py::dumps(&answer));
        return Ok(0);
    }
    say!("{}", perf_line(ms, &answer));
    Ok(0)
}

/// One line: fps, missed refreshes and worst gap, then the app's work per frame (Android) or per refresh (iOS).
fn perf_line(ms: f64, answer: &Value) -> String {
    let refreshes = answer["refreshes"].as_u64().unwrap_or(0);
    let missed = answer["missed"].as_u64().unwrap_or(0);
    // An app with an older library gives no fps.
    let fps = if answer["fps"].is_number() { format!(" {} fps,", py::str(&answer["fps"])) } else { String::new() };
    let mut line = format!(
        "{}s:{fps} the main thread missed {missed} of {} refreshes ({} ms each), worst gap {} ms",
        py::fixed(&json!(ms / 1000.0), 1),
        refreshes + missed,
        py::str(&answer["refreshMs"]),
        py::str(&answer["worstGapMs"])
    );
    let work = |what: &str| {
        format!(
            "{what} mean {} ms, p95 {} ms, {} over a refresh",
            py::str(&answer["drawMeanMs"]),
            py::str(&answer["drawP95Ms"]),
            py::str(&answer["drawnOverRefresh"])
        )
    };
    match answer["drawn"].as_u64() {
        Some(0) => line.push_str("; no frame drawn (nothing changed on screen)"),
        Some(drawn) => line.push_str(&format!("; {drawn} frames drawn, {}", work("the app's work per frame"))),
        // iOS does not tell an app which refreshes drew: its library gives the main thread's work per refresh.
        None if answer["drawMeanMs"].is_number() => {
            line.push_str(&format!("; {}", work("the main thread's work per refresh")))
        }
        None if answer.get("drawMeanMs").is_none() => {
            line.push_str("; work per frame not measured (the app's AgentDDP is older than 0.5.0)")
        }
        None => line.push_str("; work per refresh not measured (no refresh came)"),
    }
    line
}

/// Events the app sent in Debug (`AgentDDP.event`, `AgentDdpEvents.send`): the last ones, or each as it comes.
pub fn app_events(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let grep = args.grep.as_deref().map(str::to_lowercase);
    let shown = |entry: &Value| {
        let text = event_line(entry);
        if grep.as_ref().is_none_or(|grep| text.to_lowercase().contains(grep)) {
            let time = state::local_time("%H:%M:%S", f(&entry["t"]) / 1000.0);
            say!("{time} {text}");
        }
    };
    let mut control = open(&session)?;
    if !args.follow {
        let listed = control.call("events.list", json!({}))?;
        control.close();
        if ctx().json {
            say!("{}", py::dumps(&listed["events"]));
            return Ok(0);
        }
        let events = listed["events"].as_array().cloned().unwrap_or_default();
        if events.is_empty() {
            say!("no events: the app sends them in Debug with AgentDDP.event (iOS) or AgentDdpEvents.send (Android)");
        }
        let limit = args.limit.unwrap_or(50);
        events.iter().skip(events.len().saturating_sub(limit)).for_each(shown);
        return Ok(0);
    }
    let deadline =
        args.duration.as_deref().map(parse_ms).transpose()?.map(|ms| Instant::now() + Duration::from_millis(ms as u64));
    control.call("events.watch", json!({}))?;
    while let Some(event) = control.next_event(deadline)? {
        if event["method"] == "events.entry" {
            shown(&event["params"]);
        }
    }
    control.close();
    Ok(0)
}

/// `name key=value ...`, fields in the order the app gave them.
pub fn event_line(entry: &Value) -> String {
    let fields: Vec<String> = entry["fields"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| format!("{key}={}", if value.is_string() { py::str(value) } else { py::dumps(value) }))
        .collect();
    std::iter::once(py::str(&entry["name"])).chain(fields).collect::<Vec<_>>().join(" ")
}

pub fn time_control(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let action = args.action.clone().unwrap_or_default();
    let mut params = json!({});
    if let Some(value) = &args.value {
        let (key, number) = match action.as_str() {
            "seek" => ("ms", parse_ms(value)?),
            "step" => ("frames", read_number(value)?),
            _ => ("rate", read_number(value)?),
        };
        params[key] = py::fnum(number);
    }
    let state = open(&session)?.call(&format!("time.{action}"), params)?;
    let paused = if state["paused"] == json!(true) {
        "paused".to_string()
    } else {
        format!("running at rate {}", py::str(&state["rate"]))
    };
    let since =
        state.get("sincePauseMs").map(|v| format!(", {}ms from the pause point", py::fixed(v, 0))).unwrap_or_default();
    say!("clock {paused}{since}");
    Ok(0)
}

fn read_number(value: &str) -> Result<f64> {
    value.parse::<f64>().map_err(|_| failure(format!("can't read '{value}'"), "bad_value", None))
}

/// A library before 0.2.0 serves only the v1 endpoint: `live.json` on iOS, the socket `agentddp.<package>` on Android.
pub fn older_library(platform: &str, device: &str, app: &str) -> bool {
    if platform == "ios" {
        let Ok(container) = model::app_container(device, app) else { return false };
        let directory = Path::new(&container).join(IOS_DIR);
        return directory.join("live.json").exists() && !directory.join("control.json").exists();
    }
    let Ok(adb) = adb_path() else { return false };
    let sockets = capture(&adb, &["-s", device, "shell", "cat", "/proc/net/unix"]).stdout;
    let old = format!("@agentddp.{app}");
    let new = format!("{old}.control");
    let names: Vec<&str> = sockets.lines().filter_map(|line| line.split_whitespace().last()).collect();
    names.contains(&old.as_str()) && !names.contains(&new.as_str())
}

pub fn library_too_old() -> Failure {
    failure(
        "the app's agent-ddp library is older than 0.2.0: it serves only the v1 endpoint, which this ddp no longer speaks",
        "library_too_old",
        Some("update the library in the app's Debug build to the same version as ddp (ddp --version), or use ddp 0.1.x"),
    )
}

/// The abstract socket of the control endpoint, as `adb forward` names it.
pub fn android_socket(app: &str) -> String {
    format!("localabstract:agentddp.{app}.control")
}

pub fn net_line(request: &Value) -> String {
    let status = request.get("status").or_else(|| request.get("error")).map(py::str).unwrap_or_else(|| "failed".into());
    format!(
        "{} {} {status} {}ms {}B {}",
        py::str(&request["id"]),
        py::str(&request["method"]),
        py::fixed(&request["ms"], 0),
        py::str(&request["responseBytes"]),
        py::str(&request["url"])
    )
}

#[cfg(test)]
mod tests {
    use super::{media_session, perf_line, shell_quoted};
    use serde_json::json;

    #[test]
    fn the_library_is_the_apps_own_starter_not_another_package_named_in_the_dump() {
        let settings =
            "Packages:\n  Package [com.android.settings]\n    queriesPackages:\n        dev.agentddp.sample\n";
        assert!(!super::has_library_component(settings, "com.android.settings"));
        let sample = "  dev.agentddp.sample/dev.agentddp.AgentDdpStarter:\n    Provider{c8bbda8 dev.agentddp.sample/dev.agentddp.AgentDdpStarter}\n";
        assert!(super::has_library_component(sample, "dev.agentddp.sample"));
        assert!(!super::has_library_component(sample, "dev.agentddp"));
    }

    #[test]
    fn perf_says_work_per_frame_on_android_and_per_refresh_on_ios() {
        let android = json!({"refreshMs": 16.67, "refreshes": 59, "missed": 3, "worstGapMs": 66.7, "fps": 57.1,
                             "drawn": 20, "drawMeanMs": 6.1, "drawP95Ms": 12.4, "drawnOverRefresh": 1});
        assert_eq!(
            perf_line(1000.0, &android),
            "1.0s: 57.1 fps, the main thread missed 3 of 62 refreshes (16.67 ms each), worst gap 66.7 ms; \
             20 frames drawn, the app's work per frame mean 6.1 ms, p95 12.4 ms, 1 over a refresh"
        );
        let ios = json!({"refreshMs": 16.67, "refreshes": 300, "missed": 0, "worstGapMs": 17.1, "fps": 60.0,
                         "drawn": null, "drawMeanMs": 3.2, "drawP95Ms": 7.9, "drawnOverRefresh": 0});
        assert!(perf_line(5000.0, &ios)
            .ends_with("; the main thread's work per refresh mean 3.2 ms, p95 7.9 ms, 0 over a refresh"));
        let old_ios = json!({"refreshMs": 16.67, "refreshes": 300, "missed": 0, "worstGapMs": 17.1, "drawn": null});
        assert_eq!(
            perf_line(5000.0, &old_ios),
            "5.0s: the main thread missed 0 of 300 refreshes (16.67 ms each), worst gap 17.1 ms; \
             work per frame not measured (the app's AgentDDP is older than 0.5.0)"
        );
    }

    #[test]
    fn the_media_session_is_the_apps_first_active_one_as_the_system_lists_it() {
        let dump = "Global priority session is com.android.server.telecom/HeadsetMediaButton/1 (userId=0)
  HeadsetMediaButton com.android.server.telecom/HeadsetMediaButton/1 (userId=0)
    package=com.android.server.telecom
    active=false
    state=null
    metadata: null
  Sessions Stack - have 3 sessions:
    old dev.agentddp.sample/old/5 (userId=0)
      package=dev.agentddp.sample
      active=false
      state=PlaybackState {state=PAUSED(2), position=4000, buffered position=0, speed=0.0, updated=100, actions=0}
      metadata: size=1, description=Old, null, null
    sample dev.agentddp.sample/sample/6 (userId=0)
      package=dev.agentddp.sample
      active=true
      state=PlaybackState {state=PLAYING(3), position=12500, buffered position=0, speed=1.0, updated=115678, actions=0, custom actions=[], active item id=-1, error=null}
      audioAttrs=AudioAttributes: usage=USAGE_MEDIA content=CONTENT_TYPE_UNKNOWN flags=0x800 tags= bundle=null
      metadata: size=3, description=Song, with a comma, Artist, null
    other com.example.other/other/7 (userId=0)
      package=com.example.other
      active=true
      state=PlaybackState {state=3, position=0, buffered position=0, speed=1.0, updated=1, actions=0}
      metadata: null
";
        assert_eq!(
            media_session(dump, "dev.agentddp.sample"),
            Some(json!({"source": "media session", "state": "playing", "position": 12.5, "rate": 1,
                        "description": "Song, with a comma, Artist, null"}))
        );
        // Before Android 14 the state is the bare number; no metadata, no description.
        assert_eq!(
            media_session(dump, "com.example.other"),
            Some(json!({"source": "media session", "state": "playing", "position": 0, "rate": 1}))
        );
        assert_eq!(media_session(dump, "com.android.server.telecom"), None);
        assert_eq!(media_session(dump, "dev.agentddp.missing"), None);
    }

    #[test]
    fn a_value_reaches_the_device_shell_as_one_argument() {
        assert_eq!(shell_quoted("http://10.0.2.2:8080/a b"), "'http://10.0.2.2:8080/a b'");
        assert_eq!(shell_quoted("it's; rm -rf /"), "'it'\\''s; rm -rf /'");
    }
}
