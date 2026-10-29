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
}

impl AndroidDevice {
    pub fn new(serial: &str, app: &str) -> Result<Self> {
        Ok(AndroidDevice { adb: adb_path()?, serial: serial.into(), app: app.into() })
    }

    fn shell(&self, args: &[&str]) -> state::Output {
        let mut all = vec!["-s", &self.serial, "exec-out"];
        all.extend_from_slice(args);
        capture(&self.adb, &all)
    }
}

pub trait OfflineDevice {
    fn request(&self, frames: i64) -> Result<()>;
    fn fetch(&self) -> Option<String>;
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
        self.shell(&["run-as", &self.app, "rm", "-f", &format!("{ANDROID_DIR}/recording.jsonl")]);
        let sent = self.shell(&[
            "am",
            "broadcast",
            "-a",
            "dev.agentddp.RECORD",
            "-p",
            &self.app,
            "--ei",
            "frames",
            &frames.to_string(),
        ]);
        if sent.code != 0 {
            fail!(
                format!("broadcast failed: {}", sent.stderr.trim()),
                "broadcast_failed",
                format!("ddp doctor {} --app {}", self.serial, self.app)
            );
        }
        Ok(())
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

fn spawn_then(command: &str) -> Option<Child> {
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
    Ok(json!({"type": "header", "protocol": model::PROTOCOL, "platform": info["platform"], "unit": info["unit"],
              "scale": info["scale"], "frames": frames, "startedAt": py::fnum(state::now_ms().round())}))
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
    let (session, snapshot) = attach(&platform, &device, &app)?;
    keep_session(&session, &snapshot)
}

/// `ddp launch`: the app started fresh (a running copy is stopped first), with values for it, then connected.
/// iOS gets them as environment variables, Android as string extras of the launch intent: Android has no
/// per-launch environment for an app.
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
    let started = if platform == "ios" {
        let mut command = Command::new("xcrun");
        command.args(["simctl", "launch", "--terminate-running-process", &device, &app]);
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
    // The old run's endpoint may still be on disk: wait until the new run's answers.
    let deadline = Instant::now() + Duration::from_secs(15);
    let (session, snapshot) = loop {
        match attach(&platform, &device, &app) {
            Ok(attached) => break attached,
            Err(failure) if Instant::now() >= deadline => return Err(failure),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    };
    say!("launched {app} on {platform} {device}");
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

/// The app's launcher activity, as the package manager resolves it (`monkey` does not start every app).
fn launcher_activity(device: &str, app: &str) -> Result<String> {
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
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The session for the app's control endpoint, and what is on screen, when it answers.
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

fn keep_session(session: &Value, snapshot: &[Value]) -> Result<i32> {
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

pub fn disconnect(_args: &mut Args) -> Result<i32> {
    let context = ctx();
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
    let answer = open(&session)?.call("screen.value", json!({"id": args.id, "path": args.path}))?;
    say!("{}", py::dumps(answer.get("value").unwrap_or(&Value::Null)));
    Ok(0)
}

pub fn net(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let mut control = open(&session)?;
    match args.action.as_deref() {
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
            say!("{name} = {}", py::str(&values["values"][&name]));
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
            if entries.is_empty() {
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
    while (started.elapsed().as_millis() as f64) < duration {
        let values = control.call("state.get", json!({"names": [name]}))?;
        let value = values["values"][name].clone();
        let t = started.elapsed().as_millis();
        match &previous {
            None => say!("t={t}ms {name} = {}", py::str(&value)),
            Some(before) if !py::equal(before, &value) => {
                say!("t={t}ms {name} {} -> {}", py::str(before), py::str(&value))
            }
            Some(_) => {}
        }
        previous = Some(value);
        if let Some(clauses) = &clauses {
            let fields = values["values"].as_object().cloned().unwrap_or_default();
            if model::matches(&fields, clauses)? {
                say!("until met at t={t}ms");
                return Ok(0);
            }
        }
        std::thread::sleep(Duration::from_millis(STATE_POLL_MS));
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
        say!("{time} {} {} {text}", py::str(&entry["level"]), py::str(&entry["tag"]));
    }
    control.close();
    Ok(0)
}

/// How the app runs for `--for`, from the platform's own clocks. A measurement: it exits 0 whatever it finds.
pub fn perf(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let ms = parse_ms(args.duration.as_deref().unwrap_or("5s"))?;
    let mut control = open(&session)?;
    let answer = control.call("perf.metrics", json!({"ms": ms}))?;
    control.close();
    let refreshes = answer["refreshes"].as_u64().unwrap_or(0);
    let mut line = format!(
        "{}s: the main thread missed {} of {refreshes} refreshes ({} ms each), worst gap {} ms",
        py::fixed(&json!(ms / 1000.0), 1),
        py::str(&answer["missed"]),
        py::str(&answer["refreshMs"]),
        py::str(&answer["worstGapMs"])
    );
    match answer["drawn"].as_u64() {
        None => line.push_str("; draw time not measured (iOS gives an app none)"),
        Some(0) => line.push_str("; no frame drawn (nothing changed on screen)"),
        Some(drawn) => line.push_str(&format!(
            "; {drawn} frames drawn, draw time mean {} ms, p95 {} ms, {} over a refresh",
            py::str(&answer["drawMeanMs"]),
            py::str(&answer["drawP95Ms"]),
            py::str(&answer["drawnOverRefresh"])
        )),
    }
    say!("{line}");
    Ok(0)
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
    use super::shell_quoted;

    #[test]
    fn a_value_reaches_the_device_shell_as_one_argument() {
        assert_eq!(shell_quoted("http://10.0.2.2:8080/a b"), "'http://10.0.2.2:8080/a b'");
        assert_eq!(shell_quoted("it's; rm -rf /"), "'it'\\''s; rm -rf /'");
    }
}
