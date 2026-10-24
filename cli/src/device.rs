//! Acting on the device and the app: record, tap, wait, logs, settings, deep links, annotate, explore.
use crate::args::Args;
use crate::checks::{check_parity, RECORDING_CHECKS};
use crate::control::Control;
use crate::live::{
    follow_logs, live_session, open, platform_of, record_live, require_session, run_then, snapshot_live, stream_frames,
    AndroidDevice, IosDevice, OfflineDevice,
};
use crate::model::{
    self, element_alpha, element_key, load, normalize_label, rect, save_recording, warn_if_empty, Frame,
};
use crate::py::{self, f};
use crate::queries::{changing_keys, save_refs};
use crate::state::{self, adb_path, capture, ctx, failure, parse_ms, read_json, write_file, Result, MAX_FRAMES};
use crate::{fail, say, warn};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn record(args: &mut Args) -> Result<i32> {
    if args.on.is_some() {
        return record_on(args);
    }
    let context = ctx();
    if live_session(args).is_some() {
        let recording = record_live(args)?;
        say!(
            "recorded {} frames live -> {}{}",
            recording.frames.len(),
            context.last.display(),
            args.name.as_ref().map(|n| format!(" (saved as {n})")).unwrap_or_default()
        );
        warn_if_empty(&recording);
        return Ok(0);
    }
    let (Some(platform), Some(app)) = (args.platform.clone(), args.app.clone()) else {
        fail!("not connected", "not_connected", "ddp connect DEVICE --app ID, or pass --platform and --app");
    };
    let Some(device_id) = args.device.clone().or_else(|| std::env::var("DDP_DEVICE").ok()) else {
        fail!("no device", "missing_device", "pass --device (simulator UDID or adb serial) or set DDP_DEVICE");
    };
    let frames = args.frames.unwrap_or(120);
    if !(1..=MAX_FRAMES).contains(&frames) {
        fail!(format!("--frames must be 1..{MAX_FRAMES} (about 10 s)"), "bad_frames", "record longer flows in parts");
    }
    if args.then.is_some() && frames < 60 {
        // The action lands about 0.5 s after the request on a simulator.
        warn!("ddp: warning: an action from --then lands about 0.5 s after recording starts; record 120+ frames to see it");
    }
    let device: Box<dyn OfflineDevice> = if platform == "ios" {
        Box::new(IosDevice::new(&device_id, &app)?)
    } else {
        Box::new(AndroidDevice::new(&device_id, &app)?)
    };
    device.request(frames)?;
    if let Some(then) = &args.then {
        if !run_then(then) {
            warn!("ddp: warning: --then command failed, the recording may show no change");
        }
    }
    // A recording runs at the display rate; allow slow frames plus app startup jitter.
    let deadline = Instant::now() + Duration::from_secs_f64(frames as f64 / 30.0 + 10.0);
    while Instant::now() < deadline {
        if let Some(content) = device.fetch() {
            save_recording(&content, args.name.as_deref())?;
            if let Some(file) = &args.file {
                write_file(file, &content)?;
            }
            let path = args.file.clone().unwrap_or(context.last.clone());
            let recording = load(&path)?;
            say!(
                "recorded {} frames -> {} (capture cost per frame: average {} ms, slowest {} ms)",
                recording.frames.len(),
                path.display(),
                py::str(recording.end.get("averageCaptureMs").unwrap_or(&Value::Null)),
                py::str(recording.end.get("slowestCaptureMs").unwrap_or(&Value::Null))
            );
            warn_if_empty(&recording);
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    fail!(
        "no recording arrived",
        "no_recording",
        format!("is the app in the foreground? ddp doctor {device_id} --app {app}")
    )
}

/// (platform, device, app) from the flags or the session.
pub fn target(args: &Args) -> Result<(String, String, Option<String>)> {
    let session = read_json(&ctx().session_file).unwrap_or(json!({}));
    let device = args
        .device
        .clone()
        .or_else(|| session["device"].as_str().map(String::from))
        .or_else(|| std::env::var("DDP_DEVICE").ok());
    let Some(device) = device else {
        fail!("no device", "missing_device", "ddp connect DEVICE --app ID, or pass --device");
    };
    let app = args.app.clone().or_else(|| session["app"].as_str().map(String::from));
    let platform = args
        .platform
        .clone()
        .or_else(|| session["platform"].as_str().map(String::from))
        .unwrap_or_else(|| platform_of(None, &device));
    Ok((platform, device, app))
}

fn adb(device: &str, command: &[&str]) -> Result<state::Output> {
    let mut all = vec!["-s", device];
    all.extend_from_slice(command);
    Ok(capture(&adb_path()?, &all))
}

/// The center of an element on screen now, where a finger would go.
fn element_center(session: &Value, id: &str) -> Result<(f64, f64)> {
    let recording = snapshot_live(session)?;
    let frame = recording.frames.last().unwrap();
    let Some(element) = frame.elements().iter().find(|e| element_key(e) == id) else {
        fail!(format!("no element '{id}' on screen"), "unknown_element", "ddp tree -i");
    };
    let [x, y, w, h] = rect(&element["frame"]);
    Ok((x + w / 2.0, y + h / 2.0))
}

fn touch(control: &mut Control, (x, y): (f64, f64)) -> Result<()> {
    for phase in ["down", "up"] {
        control.call("input.touch", json!({"phase": phase, "x": x, "y": y}))?;
    }
    Ok(())
}

pub fn tap(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let id = args.id.clone().unwrap_or_default();
    let (x, y) = element_center(&session, &id)?;
    let mut control = open(&session)?;
    touch(&mut control, (x, y))?;
    control.close();
    say!("tapped {id} at ({x:.0}, {y:.0})");
    Ok(0)
}

const FOCUS_WAIT_MS: u64 = 200;

/// Types into the focused field, or into `--into` after tapping it.
pub fn type_text(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let text = args.value.clone().unwrap_or_default();
    let mut control = open(&session)?;
    if let Some(id) = &args.id {
        touch(&mut control, element_center(&session, id)?)?;
        // The field takes the focus on a later turn of the main loop; typed text before that goes nowhere.
        std::thread::sleep(Duration::from_millis(FOCUS_WAIT_MS));
    }
    control.call("input.type", json!({"text": text}))?;
    control.close();
    say!(
        "typed {} characters{}",
        text.chars().count(),
        args.id.as_ref().map(|id| format!(" into {id}")).unwrap_or_default()
    );
    Ok(0)
}

pub fn press(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let key = args.value.clone().unwrap_or_default();
    open(&session)?.call("input.key", json!({"key": key}))?;
    say!("pressed {key}");
    Ok(0)
}

/// Live: until an element is drawn, a label shows, or nothing changes for a while.
pub fn wait(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    if args.by.is_none() && args.settled.is_none() {
        fail!("wait needs `id <id>`, `text <label>` or --settled 300ms", "missing_option");
    }
    let settled_ms = args.settled.as_deref().map(parse_ms).transpose()?;
    let timeout = parse_ms(&args.timeout)?;
    let ignored: HashSet<String> = args.ignore.iter().cloned().collect();
    let value = args.value.clone().unwrap_or_default();
    let (mut tolerance, mut last_change, mut previous) = (state::OWNER_TOLERANCE_PT, 0.0, None::<String>);
    let mut done = false;
    stream_frames(&session, Some(timeout), None, |message| {
        if message["type"] == "header" {
            tolerance = model::tolerance_for(message);
            return Ok(true);
        }
        if message["type"] != "frame" {
            return Ok(true);
        }
        let frame = Frame::from(message.clone(), tolerance);
        let t = frame.t;
        if let Some(by) = &args.by {
            let found = frame.elements().iter().find(|e| {
                let hit = if by == "id" {
                    element_key(e) == value || e.get("id").and_then(Value::as_str) == Some(&value)
                } else {
                    e.get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&normalize_label(&value).to_lowercase())
                };
                hit && f(&element_alpha(&frame, &element_key(e))) >= 0.05
            });
            if let Some(found) = found {
                say!("{} drawn at t={t:.0}ms, frame {}", element_key(found), py::str(&found["frame"]));
                done = true;
                return Ok(false);
            }
        } else {
            let elements: Vec<&Value> =
                frame.elements().iter().filter(|e| !ignored.contains(&element_key(e))).collect();
            let nodes: Vec<Value> = frame
                .nodes()
                .iter()
                .filter(|n| !n["owner"].as_str().is_some_and(|o| ignored.contains(o)))
                .map(|n| {
                    let mut copy = n.clone();
                    copy.as_object_mut().unwrap().shift_remove("z");
                    copy
                })
                .collect();
            let state = py::dumps_sorted(&json!([elements, nodes]));
            if previous.as_ref() != Some(&state) {
                previous = Some(state);
                last_change = t;
            } else if t - last_change >= settled_ms.unwrap() {
                say!("settled: nothing changed from t={last_change:.0}ms to t={t:.0}ms");
                done = true;
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    if done {
        return Ok(0);
    }
    let what = match &args.by {
        Some(by) => format!("{by} {value}"),
        None => format!("{} without change", args.settled.clone().unwrap_or_default()),
    };
    fail!(
        format!("timed out after {} waiting for {what}", args.timeout),
        "timeout",
        "ddp tree -i to see what is there, or a longer --timeout"
    )
}

/// The app's log lines during the last take (or --file), at their time in the take.
pub fn logs(args: &mut Args) -> Result<i32> {
    if args.follow {
        return follow_logs(args);
    }
    let recording = load(args.file.as_deref().unwrap_or(&ctx().last))?;
    let Some(started) = recording.header.get("startedAt").map(f).filter(|s| *s != 0.0) else {
        fail!("this recording has no start time (an older library)", "no_start_time", "record again");
    };
    let end = started + recording.frames.last().map(|f| f.t).unwrap_or(0.0) + 500.0;
    let (platform, device, app) = target(args)?;
    let Some(app) = app else {
        fail!("no app", "missing_option", "pass --app, or ddp connect first");
    };
    let mut lines: Vec<(f64, String)> = Vec::new();
    if platform == "ios" {
        let info = state::run(&["xcrun", "simctl", "appinfo", &device, &app])?;
        let executable = info
            .lines()
            .find(|l| l.contains("CFBundleExecutable"))
            .and_then(|l| l.split_once('='))
            .map(|(_, v)| v.trim().trim_matches(|c| c == '"' || c == ';').to_string())
            .unwrap_or_else(|| app.rsplit('.').next().unwrap_or(&app).to_string());
        let start_text = state::local_time("%Y-%m-%d %H:%M:%S", started / 1000.0 - 1.0);
        let output = state::run(&[
            "xcrun",
            "simctl",
            "spawn",
            &device,
            "log",
            "show",
            "--info",
            "--style",
            "compact",
            "--start",
            &start_text,
            "--predicate",
            &format!("process == \"{executable}\""),
        ])?;
        for line in output.lines() {
            let Some(seconds) = line.get(..19).and_then(state::parse_local) else { continue };
            let fraction: f64 = line.get(19..23).filter(|s| !s.is_empty()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let stamp = seconds * 1000.0 + fraction * 1000.0;
            if started - 100.0 <= stamp && stamp <= end {
                lines.push((stamp - started, line.get(24..).unwrap_or("").trim().to_string()));
            }
        }
    } else {
        let pid = adb(&device, &["shell", "pidof", &app])?.stdout.trim().to_string();
        if pid.is_empty() {
            fail!(format!("{app} is not running"), "not_running", format!("adb -s {device} shell monkey -p {app} 1"));
        }
        let output = adb(&device, &["logcat", "-d", "-v", "epoch", &format!("--pid={pid}")])?.stdout;
        for line in output.lines() {
            let mut parts = line.trim_start().splitn(2, char::is_whitespace);
            let Some(stamp) = parts.next().and_then(|p| p.parse::<f64>().ok()).map(|s| s * 1000.0) else { continue };
            if started - 100.0 <= stamp && stamp <= end {
                lines.push((stamp - started, parts.next().unwrap_or("").trim().to_string()));
            }
        }
    }
    if let Some(grep) = &args.grep {
        lines.retain(|(_, text)| text.to_lowercase().contains(&grep.to_lowercase()));
    }
    if lines.is_empty() {
        say!("no log lines from {app} during the take");
        return Ok(0);
    }
    let limit = args.limit.unwrap_or(50);
    for (t, text) in lines.iter().take(limit) {
        say!("t={t:.0}ms {text}");
    }
    if lines.len() > limit {
        say!("... {} more (--grep or --limit)", lines.len() - limit);
    }
    Ok(0)
}

pub fn settings(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let what = args.what.clone().unwrap_or_default();
    let value = args.value.clone().unwrap_or_default();
    let commands: Vec<Vec<String>> = if platform == "ios" {
        let base =
            |rest: &[&str]| -> Vec<String> { ["xcrun", "simctl"].iter().chain(rest).map(|s| s.to_string()).collect() };
        match what.as_str() {
            "appearance" => vec![base(&["ui", &device, "appearance", &value])],
            "text-size" => vec![base(&["ui", &device, "content_size", &value])],
            "reduce-motion" => vec![base(&[
                "spawn",
                &device,
                "defaults",
                "write",
                "com.apple.Accessibility",
                "ReduceMotionEnabled",
                "-bool",
                if value == "on" { "YES" } else { "NO" },
            ])],
            _ => vec![base(&[
                "spawn",
                &device,
                "defaults",
                "write",
                app.as_deref().unwrap_or("-g"),
                "AppleLanguages",
                "-array",
                &value,
            ])],
        }
    } else {
        let adb = adb_path()?;
        let base = |rest: &[&str]| -> Vec<String> {
            [adb.as_str(), "-s", &device, "shell"].iter().chain(rest).map(|s| s.to_string()).collect()
        };
        let scale = if value == "on" { "0" } else { "1" };
        match what.as_str() {
            "appearance" => vec![base(&["cmd", "uimode", "night", if value == "dark" { "yes" } else { "no" }])],
            "text-size" => {
                let factor = match value.as_str() {
                    "small" => "0.85",
                    "default" => "1.0",
                    "large" => "1.15",
                    "largest" => "1.3",
                    other => other,
                };
                vec![base(&["settings", "put", "system", "font_scale", factor])]
            }
            "reduce-motion" => ["animator_duration_scale", "transition_animation_scale", "window_animation_scale"]
                .iter()
                .map(|name| base(&["settings", "put", "global", name, scale]))
                .collect(),
            _ => vec![base(&["cmd", "locale", "set-app-locales", app.as_deref().unwrap_or(""), "--locales", &value])],
        }
    };
    for command in &commands {
        let parts: Vec<&str> = command.iter().map(String::as_str).collect();
        let result = capture(parts[0], &parts[1..]);
        if result.code != 0 {
            fail!(
                format!("{} failed: {}", parts[parts.len().saturating_sub(4)..].join(" "), result.stderr.trim()),
                "set_failed"
            );
        }
    }
    let relaunch = what == "locale" || (what == "reduce-motion" && platform == "ios");
    say!("{what} {value} on {platform} {device}{}", if relaunch { "; relaunch the app to see it" } else { "" });
    Ok(0)
}

pub fn open_url(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let url = args.url.clone().unwrap_or_default();
    let result = if platform == "ios" {
        capture("xcrun", &["simctl", "openurl", &device, &url])
    } else {
        let mut command =
            vec!["-s", device.as_str(), "shell", "am", "start", "-W", "-a", "android.intent.action.VIEW", "-d", &url];
        if let Some(app) = &app {
            command.extend(["-p", app.as_str()]);
        }
        capture(&adb_path()?, &command)
    };
    if result.code != 0 || result.stdout.contains("Error") {
        let detail = if result.stderr.is_empty() { &result.stdout } else { &result.stderr };
        fail!(format!("could not open {url}: {}", detail.trim()), "open_failed");
    }
    say!("opened {url} on {platform} {device}");
    if platform == "ios" && !url.starts_with("http://") && !url.starts_with("https://") {
        // A system alert, outside the app: ddp can't see it.
        say!("iOS asks 'Open in <app>?' for a custom scheme: confirm it (agent-device press Open)");
    }
    Ok(0)
}

/// The same take on several sessions at once (one per platform), then parity between the first two.
fn record_on(args: &mut Args) -> Result<i32> {
    let names: Vec<String> = args.on.clone().unwrap_or_default().split(',').map(String::from).collect();
    let mut processes = Vec::new();
    for name in &names {
        let session_file = Path::new(".ddp").join("sessions").join(name).join("session.json");
        let Some(session) = read_json(&session_file) else {
            fail!(
                format!("no session '{name}'"),
                "not_connected",
                format!("ddp connect DEVICE --app ID --session {name}")
            );
        };
        let mut command = state::self_command();
        command.extend(
            [
                "record",
                "--session",
                name,
                "--name",
                &format!("take-{name}"),
                "--for",
                args.duration.as_deref().unwrap_or("2s"),
            ]
            .map(String::from),
        );
        if let Some(then) = &args.then {
            // Only these three placeholders: a command may hold other braces (JSON).
            let mut then = then.clone();
            for field in ["platform", "device", "app"] {
                then = then.replace(&format!("{{{field}}}"), &py::str(&session[field]));
            }
            command.extend(["--then".to_string(), then]);
        }
        let child = Command::new(&command[0])
            .args(&command[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| failure(format!("can't start {}: {e}", command[0]), "command_failed", None))?;
        processes.push((name.clone(), child));
    }
    let mut failed = false;
    for (name, child) in processes {
        let output = child.wait_with_output().map_err(|e| failure(e.to_string(), "command_failed", None))?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
        say!("{name}: {}", text.trim());
        failed |= !output.status.success();
    }
    if failed {
        return Ok(1);
    }
    if names.len() >= 2 {
        let take = |name: &str| {
            Path::new(".ddp").join("sessions").join(name).join("saved").join(format!("take-{name}.jsonl.gz"))
        };
        args.file = Some(take(&names[0]));
        args.against = Some(take(&names[1]));
        check_parity(args)?;
    }
    Ok(0)
}

// 3x5 digits, one row per string, for labels that match @eN refs.
const DIGITS: [&str; 10] = [
    "111101101101111",
    "010110010010111",
    "111001111100111",
    "111001111001111",
    "101101111001001",
    "111100111001111",
    "111100111101111",
    "111001001001001",
    "111101111101111",
    "111101111001111",
];
const BOX_COLORS: [[u8; 3]; 6] =
    [[230, 25, 75], [60, 180, 75], [0, 130, 200], [245, 130, 48], [145, 30, 180], [240, 50, 230]];

/// 8-bit RGB or RGBA, not interlaced (what simctl and screencap write) to (width, height, rows of RGBA).
pub fn png_read(data: &[u8]) -> Result<(usize, usize, Vec<Vec<u8>>)> {
    let bad = || failure("unsupported screenshot format", "bad_png", None);
    let (mut position, mut idat, mut width, mut height, mut channels) = (8, Vec::new(), 0, 0, 4);
    while position + 8 <= data.len() {
        let length = u32::from_be_bytes(data[position..position + 4].try_into().unwrap()) as usize;
        let kind = &data[position + 4..position + 8];
        let body = data.get(position + 8..position + 8 + length).ok_or_else(bad)?;
        if kind == b"IHDR" {
            width = u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize;
            height = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
            if body[8] != 8 || !(body[9] == 2 || body[9] == 6) {
                return Err(bad());
            }
            channels = if body[9] == 6 { 4 } else { 3 };
        } else if kind == b"IDAT" {
            idat.extend_from_slice(body);
        }
        position += 12 + length;
    }
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&idat[..]), &mut raw).map_err(|_| bad())?;
    let stride = width * channels;
    let mut rows = Vec::with_capacity(height);
    let mut previous = vec![0u8; stride];
    for y in 0..height {
        let start = y * (stride + 1);
        let kind = *raw.get(start).ok_or_else(bad)?;
        let mut line = raw.get(start + 1..start + 1 + stride).ok_or_else(bad)?.to_vec();
        for i in 0..stride {
            let left = if i >= channels { line[i - channels] as i32 } else { 0 };
            let up = previous[i] as i32;
            let up_left = if i >= channels { previous[i - channels] as i32 } else { 0 };
            let add = match kind {
                1 => left,
                2 => up,
                3 => (left + up) / 2,
                4 => {
                    let guess = left + up - up_left;
                    let (pa, pb, pc) = ((guess - left).abs(), (guess - up).abs(), (guess - up_left).abs());
                    if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        up_left
                    }
                }
                _ => 0,
            };
            line[i] = (line[i] as i32 + add) as u8;
        }
        rows.push(if channels == 4 {
            line.clone()
        } else {
            line.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        });
        previous = line;
    }
    Ok((width, height, rows))
}

pub fn png_write(width: usize, height: usize, rows: &[Vec<u8>]) -> Vec<u8> {
    use std::io::Write;
    let chunk = |kind: &[u8], body: &[u8]| -> Vec<u8> {
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(body);
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&crc.sum().to_be_bytes());
        out
    };
    let mut raw = Vec::new();
    for row in rows {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(&raw).unwrap();
    let mut header = (width as u32).to_be_bytes().to_vec();
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &header));
    out.extend(chunk(b"IDAT", &encoder.finish().unwrap()));
    out.extend(chunk(b"IEND", b""));
    out
}

fn paint(rows: &mut [Vec<u8>], x0: f64, y0: f64, x1: f64, y1: f64, color: [u8; 3]) {
    let height = rows.len() as i64;
    let width = rows.first().map(|r| r.len() / 4).unwrap_or(0) as i64;
    for y in (y0 as i64).max(0)..(y1 as i64).min(height) {
        let row = &mut rows[y as usize];
        for x in (x0 as i64).max(0)..(x1 as i64).min(width) {
            let at = x as usize * 4;
            row[at..at + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

pub fn annotate(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let recording = snapshot_live(&session)?;
    let refs = save_refs(&recording.frames)?;
    let frame = recording.frames.last().unwrap();
    let out = PathBuf::from(&args.out);
    let device = py::str(&session["device"]);
    let scale = if session["platform"] == "ios" {
        state::run(&["xcrun", "simctl", "io", &device, "screenshot", "--type=png", &out.display().to_string()])?;
        recording.header.get("scale").map(f).unwrap_or(1.0)
    } else {
        let output = Command::new(adb_path()?)
            .args(["-s", &device, "exec-out", "screencap", "-p"])
            .output()
            .map_err(|e| failure(e.to_string(), "command_failed", None))?;
        write_file(&out, output.stdout)?;
        1.0
    };
    let data = std::fs::read(&out).map_err(|e| failure(e.to_string(), "file_not_found", None))?;
    let (width, height, mut rows) = png_read(&data)?;
    let density = recording.header.get("scale").map(f).unwrap_or(1.0); // labels the same size in pt on both platforms
    let thick = 2f64.max(density.round_ties_even());
    let dot = 3f64.max((density * 1.5).round_ties_even());
    for (index, element) in frame.elements().iter().enumerate() {
        let color = BOX_COLORS[index % BOX_COLORS.len()];
        let [x, y, w, h] = rect(&element["frame"]).map(|v| v * scale);
        for (a, b, c, d) in [
            (x, y, x + w, y + thick),
            (x, y + h - thick, x + w, y + h),
            (x, y, x + thick, y + h),
            (x + w - thick, y, x + w, y + h),
        ] {
            paint(&mut rows, a, b, c, d, color);
        }
        let number = refs[&element_key(element)][2..].to_string();
        let (label_w, label_h) = ((number.len() * 4 + 1) as f64 * dot, 7.0 * dot);
        paint(&mut rows, x, y, x + label_w, y + label_h, color);
        for (position, digit) in number.chars().enumerate() {
            for (bit, on) in DIGITS[digit.to_digit(10).unwrap() as usize].chars().enumerate() {
                if on == '1' {
                    let cx = x + (1 + position * 4 + bit % 3) as f64 * dot;
                    let cy = y + (1 + bit / 3) as f64 * dot;
                    paint(&mut rows, cx, cy, cx + dot, cy + dot, [255, 255, 255]);
                }
            }
        }
    }
    write_file(&out, png_write(width, height, &rows))?;
    say!("{}: {} boxes, label N is @eN", out.display(), frame.elements().len());
    for element in frame.elements() {
        say!("  {} {}", refs[&element_key(element)], element_key(element));
    }
    Ok(0)
}

/// Live: tap each tappable element on the screen once, record what happens, run check all on each take.
/// Prints one line per control; each take is saved as explore-<control> for a closer look.
/// Stops when a tap leaves the screen (most of its elements gone), so the agent decides where to go next.
pub fn explore(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let start = snapshot_live(&session)?;
    let start_frame = start.frames.last().unwrap();
    let start_keys: HashSet<String> = start_frame.keys().into_iter().collect();
    let tappable: Vec<String> = start_frame
        .elements()
        .iter()
        .filter(|e| py::truthy(e.get("tap")) && !args.skip.contains(&element_key(e)))
        .map(element_key)
        .collect();
    if tappable.is_empty() {
        fail!("nothing tappable on screen", "nothing_to_explore", "ddp tree -i (the recorder marks controls with tap)");
    }
    say!("exploring {} controls: {}", tappable.len(), tappable.join(", "));
    for key in &tappable {
        let name =
            format!("explore-{}", key.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>());
        let mut take_args = args.clone();
        take_args.duration = Some(args.per.clone());
        take_args.frames = None;
        take_args.name = Some(name.clone());
        take_args.all_frames = true;
        take_args.shared = true;
        let mut then = state::self_command();
        then.extend(["tap".into(), key.clone(), "--max-output".into(), "0".into()]);
        if let Some(session_name) = &ctx().session_name {
            then.extend(["--session".into(), session_name.clone()]);
        }
        take_args.then = Some(state::shell_join(&then));
        let recording = record_live(&take_args)?;
        // The details stay in the saved take; one line per control here.
        state::push_capture(false);
        let mut problems: Vec<(&str, usize)> = Vec::new();
        let mut outcome = Ok(());
        for (check_name, check) in RECORDING_CHECKS {
            match check(&take_args, &recording) {
                Ok(count) if count > 0 => problems.push((check_name, count)),
                Ok(_) => {}
                Err(error) => {
                    outcome = Err(error);
                    break;
                }
            }
        }
        state::pop_capture();
        outcome?;
        let changed: Vec<String> = changing_keys(&recording.frames).into_iter().filter(|k| k != key).take(6).collect();
        say!(
            "{key}: changed {}; {} (ddp check all --file {name})",
            if changed.is_empty() { "nothing else".to_string() } else { changed.join(", ") },
            if problems.is_empty() {
                "no problems".to_string()
            } else {
                problems.iter().map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ")
            }
        );
        let now: HashSet<String> = recording.frames.last().unwrap().keys().into_iter().collect();
        if (start_keys.intersection(&now).count() as f64) < start_keys.len() as f64 / 2.0 {
            let mut new: Vec<&String> = now.difference(&start_keys).collect();
            new.sort();
            say!("  {key} left the screen (new: {}); stopped. Go back and run explore again with --skip, or explore the new screen",
                 new.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
            return Ok(0);
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let rows = vec![[0x10u8, 0x20, 0x30, 0xff].repeat(5); 4];
        let (width, height, back) = png_read(&png_write(5, 4, &rows)).unwrap();
        assert_eq!((width, height), (5, 4));
        assert_eq!(back, rows);
    }
}
