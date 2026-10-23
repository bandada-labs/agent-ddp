//! The live endpoint inside the app: connect, stream frames, and the commands that only make sense live.
use crate::args::Args;
use crate::model::{self, element_alpha, element_state, load, save_recording, Frame, Recording};
use crate::py::{self, f, obj};
use crate::state::{
    self, adb_path, capture, ctx, failure, parse_ms, read_json, write_file, Result, ANDROID_DIR, IOS_DIR,
};
use crate::{fail, say, warn};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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

/// One command on the app's live endpoint: a token line, a command line, then JSON lines back.
pub struct LiveClient {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    session: Value,
    answered: bool,
    done: bool,
}

impl LiveClient {
    pub fn new(session: &Value, command: Value) -> Result<Self> {
        let port = session["port"].as_u64().unwrap_or(0) as u16;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(|error| {
            failure(
                format!("can't reach the app ({error})"),
                "unreachable",
                Some(&format!(
                    "is it running? ddp connect {} --app {}, or ddp doctor",
                    py::str(&session["device"]),
                    py::str(&session["app"])
                )),
            )
        })?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let token = json!({"token": session.get("token").cloned().unwrap_or(json!(""))});
        let mut writer = stream.try_clone().map_err(|e| failure(e.to_string(), "unreachable", None))?;
        let _ = writer.write_all(format!("{}\n{}\n", py::dumps(&token), py::dumps(&command)).as_bytes());
        let reader = BufReader::new(stream.try_clone().map_err(|e| failure(e.to_string(), "unreachable", None))?);
        Ok(LiveClient { stream, reader, session: session.clone(), answered: false, done: false })
    }

    /// The next message, or None at the end of the answer.
    pub fn next_message(&mut self) -> Result<Option<Value>> {
        if self.done {
            return Ok(None);
        }
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line);
            if matches!(read, Ok(0) | Err(_)) {
                self.done = true;
                if !self.answered {
                    // adb forward accepts the connection even when nothing listens in the app, then closes it.
                    fail!(
                        "the app closed the connection without answering",
                        "unreachable",
                        format!(
                            "is the app running? ddp doctor {} --app {}",
                            py::str(&self.session["device"]),
                            py::str(&self.session["app"])
                        )
                    );
                }
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue; // keep-alive
            }
            self.answered = true;
            let message: Value = serde_json::from_str(&line)
                .map_err(|e| failure(format!("the app sent something that is not JSON ({e})"), "app_error", None))?;
            if message["type"] == "error" {
                self.done = true;
                fail!(py::str(&message["error"]), "app_error");
            }
            if message["type"] == "end" {
                self.done = true;
                if message.get("frames").is_none() {
                    return Ok(None);
                }
            }
            return Ok(Some(message));
        }
    }

    pub fn all(mut self) -> Result<Vec<Value>> {
        let mut messages = Vec::new();
        while let Some(message) = self.next_message()? {
            messages.push(message);
        }
        Ok(messages)
    }

    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
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
        let endpoint_file = Path::new(&model::app_container(&device, &app)?).join(IOS_DIR).join("live.json");
        let Some(endpoint) = read_json(&endpoint_file) else {
            fail!(
                "the app has no live endpoint",
                "no_endpoint",
                format!("run a Debug build that calls AgentDDP.start(); ddp doctor {device} --app {app}")
            );
        };
        json!({"platform": "ios", "app": app, "device": device, "port": endpoint["port"], "token": endpoint["token"]})
    } else {
        let forwarded =
            capture(&adb_path()?, &["-s", &device, "forward", "tcp:0", &format!("localabstract:agentddp.{app}")]);
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
    let snapshot = LiveClient::new(&session, json!({"cmd": "snapshot"}))?.all()?;
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
    let mut client = LiveClient::new(session, json!({"cmd": "stream", "frames": 0}))?;
    let mut started_then = false;
    let result = (|| {
        while let Some(message) = client.next_message()? {
            if !each(&message)? {
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
    client.close();
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
    let messages = LiveClient::new(session, json!({"cmd": "snapshot"}))?.all()?;
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
    let command = json!({"cmd": "eval", "id": args.id, "path": args.path});
    for message in LiveClient::new(&session, command)?.all()? {
        say!("{}", py::dumps(message.get("value").unwrap_or(&Value::Null)));
    }
    Ok(0)
}

pub fn net(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    match args.action.as_deref() {
        Some("get") => {
            let Some(id) = &args.request_id else {
                fail!("net get needs a request id", "missing_option", "ddp net list");
            };
            for message in LiveClient::new(&session, json!({"cmd": "net-get", "id": id}))?.all()? {
                say!("{}", py::dumps_pretty(&message));
            }
        }
        Some("list") => {
            let requests = LiveClient::new(&session, json!({"cmd": "net"}))?.all()?;
            if requests.is_empty() {
                say!("no requests yet (the app reports them from its Debug network decorator)");
            }
            for request in &requests {
                say!("{}", net_line(request));
            }
        }
        _ => {
            let clauses = args.until.as_deref().map(model::parse_where).transpose()?;
            let mut client = LiveClient::new(&session, json!({"cmd": "net-watch"}))?;
            say!("watching requests live");
            let result = (|| {
                while let Some(request) = client.next_message()? {
                    say!("{}", net_line(&request));
                    if let (Some(clauses), Some(fields)) = (&clauses, request.as_object()) {
                        if model::matches(fields, clauses)? {
                            say!("until met: {}", py::str(&request["id"]));
                            return Ok(());
                        }
                    }
                }
                Ok(())
            })();
            client.close();
            result?;
        }
    }
    Ok(0)
}

pub fn time_control(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let action = args.action.clone().unwrap_or_default();
    let mut command = obj(vec![("cmd", "time".into()), ("action", action.clone().into())]);
    if let Some(value) = &args.value {
        let number = if action == "seek" {
            parse_ms(value)?
        } else {
            value.parse::<f64>().map_err(|_| failure(format!("can't read '{value}'"), "bad_value", None))?
        };
        command["value"] = py::fnum(number);
    }
    for state in LiveClient::new(&session, command)?.all()? {
        let paused = if state["paused"] == json!(true) {
            "paused".to_string()
        } else {
            format!("running at rate {}", py::str(&state["rate"]))
        };
        let since = state
            .get("sincePauseMs")
            .map(|v| format!(", {}ms from the pause point", py::fixed(v, 0)))
            .unwrap_or_default();
        say!("clock {paused}{since}");
    }
    Ok(0)
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
