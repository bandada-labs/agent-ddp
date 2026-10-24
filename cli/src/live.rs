//! The live endpoint inside the app: connect, stream frames, and the commands that only make sense live.
use crate::args::Args;
use crate::control::Control;
use crate::model::{self, element_alpha, element_state, load, save_recording, Frame, Recording};
use crate::py::{self, f};
use crate::state::{
    self, adb_path, capture, ctx, failure, parse_ms, read_json, write_file, Result, ANDROID_DIR, IOS_DIR,
};
use crate::{fail, say, warn};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The control endpoint's version this ddp speaks (`app.info` `protocol`).
const CONTROL_PROTOCOL: u64 = 2;
const STILL_SCREEN_SLACK_MS: u64 = 500;

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

/// Runs the action without a shell: no pipes, globs or variables, only a program and its arguments.
pub fn run_then(command: &str) -> bool {
    let words = match state::shell_split(command) {
        Ok(words) if !words.is_empty() => words,
        Ok(_) => return false,
        Err(error) => {
            warn!("ddp: --then: {}", error.message);
            return false;
        }
    };
    match Command::new(&words[0]).args(&words[1..]).status() {
        Ok(status) => status.success(),
        Err(error) => {
            warn!("ddp: --then: {error}");
            false
        }
    }
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
    let spoken = info["protocol"].as_u64().unwrap_or(0);
    if spoken > CONTROL_PROTOCOL {
        fail!(
            format!("the app's library speaks control protocol {spoken}; this ddp speaks up to {CONTROL_PROTOCOL}"),
            "protocol_too_new",
            "npm i -g agent-ddp@latest"
        );
    }
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
    let session = if platform == "ios" {
        let endpoint_file = Path::new(&model::app_container(&device, &app)?).join(IOS_DIR).join("control.json");
        let Some(endpoint) = read_json(&endpoint_file) else {
            fail!(
                "the app has no control endpoint",
                "no_endpoint",
                format!("run a Debug build that calls AgentDDP.start(); ddp doctor {device} --app {app}")
            );
        };
        json!({"platform": "ios", "app": app, "device": device, "port": endpoint["port"]})
    } else {
        let forwarded = capture(&adb_path()?, &["-s", &device, "forward", "tcp:0", &android_socket(&app)]);
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
    let snapshot = snapshot_messages(&session)?;
    write_file(&ctx().session_file, py::dumps(&session))?;
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
