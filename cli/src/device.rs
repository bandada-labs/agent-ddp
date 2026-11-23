//! Acting on the device and the app: record, tap, wait, logs, settings, deep links, annotate, explore.
use crate::args::Args;
use crate::checks::{check_parity, RECORDING_CHECKS};
use crate::control::Control;
use crate::live::{
    follow_logs, live_session, open, platform_of, record_live_inspected, require_session, snapshot_live, spawn_then,
    stream_frames, AndroidDevice, IosDevice, OfflineDevice,
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
        // With the platform's answers at the end, so a check on the file later judges what a live check would.
        let recording = record_live_inspected(args)?;
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
    // The action runs while ddp waits: `am instrument -w` returns only when its test ends, and the app it starts is
    // a new process that must be asked again.
    let mut then = args.then.as_deref().and_then(spawn_then);
    if args.then.is_some() && then.is_none() {
        warn!("ddp: warning: --then command failed, the recording may show no change");
    }
    // A recording runs at the display rate; allow slow frames plus app startup jitter, counted after the action.
    let allowance = Duration::from_secs_f64(frames as f64 / 30.0 + 10.0);
    let mut deadline = Instant::now() + allowance;
    let mut nudged = Instant::now();
    while Instant::now() < deadline || then.is_some() {
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
        if let Some(child) = then.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                if !status.success() {
                    warn!("ddp: warning: --then command failed, the recording may show no change");
                }
                then = None;
                deadline = deadline.max(Instant::now() + allowance);
            }
        }
        if nudged.elapsed() >= Duration::from_secs(1) {
            device.ask_again(frames);
            nudged = Instant::now();
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
    // The session describes its own device only: another one given with --device says its platform by its id.
    let same_device = session["device"].as_str() == Some(device.as_str());
    let app = args.app.clone().or_else(|| session["app"].as_str().filter(|_| same_device).map(String::from));
    let platform = args
        .platform
        .clone()
        .or_else(|| session["platform"].as_str().filter(|_| same_device).map(String::from))
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
    let Some(element) = frame.named(id) else {
        fail!(format!("no element '{id}' on screen"), "unknown_element", "ddp tree -i");
    };
    let [x, y, w, h] = rect(&element["frame"]);
    Ok((x + w / 2.0, y + h / 2.0))
}

pub fn touch(control: &mut Control, (x, y): (f64, f64)) -> Result<()> {
    for phase in ["down", "up"] {
        control.call("input.touch", json!({"phase": phase, "x": x, "y": y}))?;
    }
    Ok(())
}

const FOCUS_WAIT_MS: u64 = 200;
/// How long `type --into` waits for the field to show what was typed.
const TYPE_SHOW_WAIT_MS: u64 = 2000;

/// Types into the focused field, or into `--into` after tapping it.
pub fn type_text(args: &mut Args) -> Result<i32> {
    if crate::runner::device_mode(args) {
        return crate::runner::type_text(args);
    }
    let session = require_session()?;
    if args.clear {
        fail!(
            "type --clear works without the library only (--device) for now",
            "not_supported",
            "ddp press Backspace, once per character"
        );
    }
    let text = args.value.clone().unwrap_or_default();
    let mut control = open(&session)?;
    let mut places = Vec::new();
    if let Some(id) = &args.id {
        let at = element_center(&session, id)?;
        let field = crate::hit::frame_now(&session)?.named(id).map(crate::hit::seen);
        places.push((id.clone(), at, field));
        touch(&mut control, at)?;
        // The field takes the focus on a later turn of the main loop; typed text before that goes nowhere.
        std::thread::sleep(Duration::from_millis(FOCUS_WAIT_MS));
    }
    // Compose takes the text at once and shows it a frame or more later. With --into a Compose field, return once
    // its text changed, so the next command reads it. A field the library cannot read is not waited for.
    let editable = |control: &mut Control| -> Option<Value> {
        let id = args.id.as_ref().filter(|_| session["platform"] == "android")?;
        let answer = control.call("screen.value", json!({"id": id, "path": "semantics.EditableText"})).ok()?;
        (answer["type"] == "value").then(|| answer["value"].clone())
    };
    let before = editable(&mut control);
    control.call("input.type", json!({"text": text}))?;
    let mut late = String::new();
    if before.is_some() {
        let deadline = Instant::now() + Duration::from_millis(TYPE_SHOW_WAIT_MS);
        while editable(&mut control) == before {
            if Instant::now() >= deadline {
                late = format!("; the field's text did not change within {TYPE_SHOW_WAIT_MS}ms");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    control.close();
    crate::hit::note(&py::str(&session["platform"]), places, &[], json!({"text": text}));
    say!(
        "typed {} characters{}{late}",
        text.chars().count(),
        args.id.as_ref().map(|id| format!(" into {id}")).unwrap_or_default()
    );
    Ok(0)
}

pub fn press(args: &mut Args) -> Result<i32> {
    if crate::runner::device_mode(args) {
        return crate::runner::press(args);
    }
    let session = require_session()?;
    let key = args.value.clone().unwrap_or_default();
    // The libraries press these three; any other key name they would take and do nothing with.
    if !["Enter", "Tab", "Backspace"].contains(&key.as_str()) {
        fail!(
            format!("{key} is a key ddp presses without the library only (--device); the library presses Enter, Tab and Backspace"),
            "not_supported",
            format!("ddp press {key} --device <device>")
        );
    }
    open(&session)?.call("input.key", json!({"key": key}))?;
    crate::hit::note(&py::str(&session["platform"]), Vec::new(), &[], json!({"key": key}));
    say!("pressed {key}");
    Ok(0)
}

/// `scroll` drives the platform's own touches, as an app without the library gets them.
pub fn scroll(args: &mut Args) -> Result<i32> {
    if crate::runner::device_mode(args) {
        return crate::runner::scroll(args);
    }
    fail!(
        "scroll works without the library only (--device, or a session ddp up made) for now",
        "not_supported",
        "ddp swipe 200,600 200,200 moves the content with the library"
    );
}

/// Live: until an element is drawn, a label shows, or nothing changes for a while.
pub fn wait(args: &mut Args) -> Result<i32> {
    if crate::runner::device_mode(args) {
        return crate::runner::wait(args);
    }
    let session = require_session()?;
    if args.by.is_none() && args.settled.is_none() {
        fail!("wait needs `id <id>`, `text <label>` or --settled 300ms", "missing_option");
    }
    if args.gone && args.by.is_none() {
        fail!("wait --gone needs `id <id>` or `text <label>`", "missing_option", "ddp wait text Loading --gone");
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
            match (found, args.gone) {
                (Some(found), false) => {
                    say!("{} drawn at t={t:.0}ms, frame {}", element_key(found), py::str(&found["frame"]));
                    done = true;
                    return Ok(false);
                }
                (None, true) => {
                    say!("no {by} {} drawn at t={t:.0}ms", py::dumps(&json!(value)));
                    done = true;
                    return Ok(false);
                }
                _ => {}
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
        Some(by) if args.gone => format!("{by} {value} to go"),
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
    if args.since_launch {
        return logs_since_launch(args);
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
        let executable = ios_executable(&device, &app)?;
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
            fail!(format!("{app} is not running"), "not_running", format!("ddp launch {device} --app {app}"));
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

/// The app process's log lines since it started, the newest `--limit` of them, at their time of day. Android keeps
/// what its log buffer still holds; iOS keeps the simulator's whole log.
fn logs_since_launch(args: &Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else {
        fail!("no app", "missing_option", "pass --app, or ddp connect first");
    };
    let not_running = || {
        failure(
            format!("{app} is not running on {device}"),
            "not_running",
            Some(&format!("ddp launch {device} --app {app}")),
        )
    };
    let mut lines: Vec<String> = Vec::new();
    if platform == "ios" {
        let pid = ios_app_pid(&device, &app).ok_or_else(not_running)?.to_string();
        // A simulator's processes are the Mac's own: `ps` says how long this one has run.
        let elapsed =
            elapsed_seconds(capture("ps", &["-o", "etime=", "-p", &pid]).stdout.trim()).ok_or_else(not_running)?;
        let start = state::local_time("%Y-%m-%d %H:%M:%S", state::now_ms() / 1000.0 - elapsed - 1.0);
        let predicate = format!("processIdentifier == {pid}");
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
            &start,
            "--predicate",
            &predicate,
        ])?;
        lines = output
            .lines()
            .filter(|line| line.get(..19).and_then(state::parse_local).is_some())
            .map(|line| format!("{} {}", line.get(11..23).unwrap_or(""), line.get(24..).unwrap_or("").trim()))
            .collect();
    } else {
        let pid = adb(&device, &["shell", "pidof", &app])?.stdout.trim().to_string();
        if pid.is_empty() {
            return Err(not_running());
        }
        for line in adb(&device, &["logcat", "-d", "-v", "epoch", &format!("--pid={pid}")])?.stdout.lines() {
            let mut parts = line.trim_start().splitn(2, char::is_whitespace);
            let Some(stamp) = parts.next().and_then(|part| part.parse::<f64>().ok()) else { continue };
            let millis = ((stamp.fract() * 1000.0).round() as u32).min(999);
            lines.push(format!(
                "{}.{millis:03} {}",
                state::local_time("%H:%M:%S", stamp),
                parts.next().unwrap_or("").trim()
            ));
        }
    }
    if let Some(grep) = &args.grep {
        lines.retain(|text| text.to_lowercase().contains(&grep.to_lowercase()));
    }
    if lines.is_empty() {
        say!("no log lines from {app} since it started");
        return Ok(0);
    }
    let limit = args.limit.unwrap_or(50);
    if lines.len() > limit {
        say!("... {} earlier lines (--grep or --limit)", lines.len() - limit);
    }
    lines[lines.len().saturating_sub(limit)..].iter().for_each(|line| say!("{line}"));
    Ok(0)
}

/// The app's process on an iOS simulator, as its launchd lists it; None when it is not running.
pub fn ios_app_pid(device: &str, app: &str) -> Option<i32> {
    let listed = capture("xcrun", &["simctl", "spawn", device, "launchctl", "list"]).stdout;
    let label = format!("UIKitApplication:{app}[");
    listed.lines().find(|line| line.contains(&label))?.split_whitespace().next()?.parse().ok()
}

/// `ps -o etime`: `[[dd-]hh:]mm:ss` in seconds.
fn elapsed_seconds(text: &str) -> Option<f64> {
    let (days, clock) =
        text.split_once('-').map_or(Some((0.0, text)), |(days, clock)| Some((days.parse().ok()?, clock)))?;
    let parts: Vec<f64> = clock.split(':').map(|part| part.parse().ok()).collect::<Option<_>>()?;
    let clock_seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * 60.0 + seconds,
        [hours, minutes, seconds] => hours * 3600.0 + minutes * 60.0 + seconds,
        _ => return None,
    };
    Some(days * 86400.0 + clock_seconds)
}

/// Stops the app. The session stays: `ddp launch` starts it again.
pub fn stop(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    let was_running = if platform == "ios" {
        let stopped = capture("xcrun", &["simctl", "terminate", &device, &app]);
        // simctl says "found nothing to terminate" when the app was not running.
        if stopped.code != 0 && !stopped.stderr.contains("found nothing to terminate") {
            fail!(
                format!("could not stop {app}: {}", stopped.stderr.trim()),
                "command_failed",
                format!("ddp doctor {device} --app {app}")
            );
        }
        stopped.code == 0
    } else {
        let running = !adb(&device, &["shell", "pidof", &app])?.stdout.trim().is_empty();
        let stopped = adb(&device, &["shell", "am", "force-stop", &app])?;
        if stopped.code != 0 {
            fail!(
                format!("could not stop {app}: {}", stopped.stderr.trim()),
                "command_failed",
                format!("ddp doctor {device} --app {app}")
            );
        }
        running
    };
    say!("{} {app} on {platform} {device}", if was_running { "stopped" } else { "was not running:" });
    Ok(0)
}

/// Clears the app's data, like a fresh install; it stops the app first.
pub fn clear(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    crate::live::reset_data(&platform, &device, &app)?;
    Ok(0)
}

/// What `am instrument -r` reported, test by test.
#[derive(Debug, Default, PartialEq)]
struct TestRun {
    passed: usize,
    skipped: usize,
    /// The skipped tests (ignored, or an assumption that did not hold), by name.
    skipped_tests: Vec<String>,
    /// `Class#method` and the first line of its stack.
    failed: Vec<(String, String)>,
    /// The run ended early: the process died, or the instrumentation never ran. The test running then, if any.
    stopped: Option<(String, Option<String>)>,
    /// Every test that got its final code, in the order they ran.
    finished: Vec<String>,
    /// The tests that passed, by name.
    passed_tests: Vec<String>,
    /// Failed, then passed when run once more (`--rerun-failed`): the first failure's line.
    flaky: Vec<(String, String)>,
}

/// What a run says when its output ends with neither a result nor a crash message.
const NO_RESULT: &str = "the run ended without a result";

/// `am instrument -r` output. A status block ends with `INSTRUMENTATION_STATUS_CODE`: 1 a test started, 0 it passed,
/// -1 an error, -2 a failure, -3 ignored, -4 an assumption that did not hold. A value can run over several lines.
fn instrumentation_run(output: &str) -> TestRun {
    let mut run = TestRun::default();
    let (mut status, mut result) = (std::collections::HashMap::new(), std::collections::HashMap::new());
    let mut last: Option<(bool, String)> = None;
    let mut running: Option<String> = None;
    let mut finished = false;
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(pair) = line.strip_prefix("INSTRUMENTATION_STATUS: ") {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            status.insert(key.to_string(), value.to_string());
            last = Some((true, key.to_string()));
        } else if let Some(pair) = line.strip_prefix("INSTRUMENTATION_RESULT: ") {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            result.insert(key.to_string(), value.to_string());
            last = Some((false, key.to_string()));
        } else if let Some(code) = line.strip_prefix("INSTRUMENTATION_STATUS_CODE: ") {
            let field = |name: &str| status.get(name).cloned().unwrap_or_default();
            let test = format!("{}#{}", field("class"), field("test"));
            match code.trim() {
                "1" => running = Some(test.clone()),
                "0" => {
                    run.passed += 1;
                    run.passed_tests.push(test.clone());
                }
                "-1" | "-2" => {
                    let stack = field("stack");
                    run.failed.push((test.clone(), stack.lines().next().unwrap_or("").trim().to_string()));
                }
                "-3" | "-4" => {
                    run.skipped += 1;
                    run.skipped_tests.push(test.clone());
                }
                _ => {}
            }
            if code.trim() != "1" {
                running = None;
                run.finished.push(test);
            }
            status.clear();
            last = None;
        } else if line.starts_with("INSTRUMENTATION_CODE: ") {
            finished = true;
            last = None;
        } else if let Some(why) = line.strip_prefix("INSTRUMENTATION_FAILED: ") {
            run.stopped = Some((format!("the instrumentation did not run: {}", why.trim()), None));
            return run;
        } else if let Some((is_status, key)) = &last {
            let values = if *is_status { &mut status } else { &mut result };
            if let Some(value) = values.get_mut(key) {
                value.push('\n');
                value.push_str(line);
            }
        }
    }
    // A crash ends the run with a message and no code for the test that was running.
    if let Some(message) = result.get("shortMsg").filter(|message| !message.is_empty()) {
        run.stopped = Some((message.trim().to_string(), running));
    } else if !finished {
        run.stopped = Some((NO_RESULT.to_string(), running));
    }
    run
}

/// How long a test run may say nothing before ddp asks whether the device is still there.
const SILENCE_BEFORE_ASKING: Duration = Duration::from_secs(10);
/// The window the app's drawn frames are counted over when a run stalled.
const STALL_FRAME_WINDOW: Duration = Duration::from_secs(2);

/// Runs `am instrument` and reads what it prints as it comes. A long run that goes silent is not waited on forever
/// when its device is gone: the emulator died or hangs, or adb lost the device. With `stall`, a run with no test
/// line for that long is stopped too, with what the app drew meanwhile.
fn instrument(
    adb_program: &str,
    words: &[&str],
    device: &str,
    app: &str,
    stall: Option<Duration>,
) -> Result<Instrumented> {
    use std::io::BufRead;
    use std::sync::mpsc::{channel, RecvTimeoutError};
    let mut child = std::process::Command::new(adb_program)
        .args(words)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| failure(format!("could not run adb: {e}"), "command_failed", None))?;
    let (sender, lines) = channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().map_while(std::result::Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let mut output = String::new();
    let mut line_times = Vec::new();
    let started = Instant::now();
    let mut first_line = None;
    let mut last_line = Instant::now();
    let mut ask_after = SILENCE_BEFORE_ASKING;
    let stopped = loop {
        match lines.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                first_line.get_or_insert_with(|| started.elapsed());
                line_times.push(started.elapsed().as_secs_f64());
                output.push_str(&line);
                output.push('\n');
                last_line = Instant::now();
                ask_after = SILENCE_BEFORE_ASKING;
            }
            Err(RecvTimeoutError::Disconnected) => break None,
            Err(RecvTimeoutError::Timeout) => {
                let silent = last_line.elapsed();
                if silent >= ask_after {
                    ask_after += SILENCE_BEFORE_ASKING;
                    if let Some(gone) = device_gone(adb_program, device) {
                        break Some(gone);
                    }
                }
                if let Some(stall) = stall.filter(|stall| silent >= *stall) {
                    break Some(stalled(adb_program, device, app, stall));
                }
            }
        }
    };
    if stopped.is_some() {
        let _ = child.kill();
    }
    let _ = child.wait();
    Ok(Instrumented { output, stopped, first_line, line_times })
}

/// What `am instrument` printed, and how.
struct Instrumented {
    output: String,
    /// Why ddp stopped the run, when it did.
    stopped: Option<String>,
    /// How long the instrumentation took to say its first line (its process starting and finding the tests).
    first_line: Option<Duration>,
    /// When each line came, in seconds from the start.
    line_times: Vec<f64>,
}

/// Why the device no longer runs the tests, when it does not: ddp's own emulator died or hangs, or adb lost it.
fn device_gone(adb_program: &str, device: &str) -> Option<String> {
    if let Some(lost) = crate::up::lost_emulator(device) {
        return Some(lost.message);
    }
    let patience = Duration::from_secs(5);
    match crate::up::capture_within(adb_program, &["-s", device, "get-state"], patience) {
        None => Some(format!("{device} did not answer adb in {}s", patience.as_secs())),
        Some(state) if state.stdout.trim() == "device" => None,
        Some(state) => {
            let said = if state.stderr.trim().is_empty() { state.stdout } else { state.stderr };
            Some(format!("adb lost {device}: {}", said.lines().next().unwrap_or("no state").trim()))
        }
    }
}

/// A run stopped for `--stall`: how long it was silent and how many frames the app drew meanwhile, as the platform
/// counts them (`dumpsys gfxinfo`). The app is stopped so the device is free again.
fn stalled(adb_program: &str, device: &str, app: &str, stall: Duration) -> String {
    let frames = || -> Option<u64> {
        let dump = capture(adb_program, &["-s", device, "shell", "dumpsys", "gfxinfo", app]).stdout;
        dump.lines().find_map(|line| line.trim().strip_prefix("Total frames rendered: ")?.trim().parse().ok())
    };
    let before = frames();
    std::thread::sleep(STALL_FRAME_WINDOW);
    let drew = match (before, frames()) {
        (Some(before), Some(after)) => {
            format!(
                "; the app drew {} frames in the last {}s",
                after.saturating_sub(before),
                STALL_FRAME_WINDOW.as_secs()
            )
        }
        _ => String::new(),
    };
    capture(adb_program, &["-s", device, "shell", "am", "force-stop", app]);
    format!("no test started or finished for {}s{drew}; the run was stopped (--stall)", stall.as_secs())
}

/// How often `ddp test` starts a lost emulator again before it stops there.
const DEFAULT_RESTARTS: usize = 3;

/// One device's part of a test run.
struct DeviceRun {
    device: String,
    run: TestRun,
    seconds: f64,
    /// From `am instrument` to its first line. None when it never said one.
    runner_start: Option<f64>,
    /// The focus check before the tests. None when it was not asked for.
    focus: Option<f64>,
    restarts: Vec<Restart>,
    /// How long each test took on this device.
    times: Vec<(String, f64)>,
}

/// An emulator that was lost in the run and started again.
struct Restart {
    why: String,
    /// The test that was running then.
    test: Option<String>,
    seconds: f64,
}

/// What a test run was asked with, the same for every device.
struct TestPlan<'a> {
    app: &'a str,
    /// The `--env` pairs, as `am instrument` words. A continued run passes these and names its tests itself.
    environment: &'a [String],
    stall: Option<Duration>,
    /// The APKs a fresh emulator needs before it can go on with the run.
    installs: &'a [String],
    restarts: usize,
    check_focus: bool,
    rerun_failed: bool,
}

/// How long an app's window gets to take the focus.
const FOCUS_PATIENCE: Duration = Duration::from_secs(10);

/// Before the tests: the screen on and unlocked, and proof that an app's window takes the focus on this device. An
/// emulator can leave the focus on the launcher with an app in front; then popups never draw, injected touches reach
/// nothing, and tests fail for the device's sake. The proof is Settings opened and closed again. A second try after
/// the Home key, which clears most cases. Silent where the device does not say what it needs to (unsure is no failure).
fn takes_focus(adb_program: &str, device: &str) -> Result<()> {
    const AWAKE: &str = "input keyevent KEYCODE_WAKEUP; wm dismiss-keyguard";
    let shell = |script: &str| capture(adb_program, &["-s", device, "shell", script]).stdout;
    let mut focus = String::new();
    for home_first in ["", "input keyevent KEYCODE_HOME; "] {
        shell(&format!("{home_first}{AWAKE}"));
        let opened = shell("am start -W -a android.settings.SETTINGS");
        let settings = opened.lines().find_map(|line| line.trim().strip_prefix("Activity: ")?.split('/').next());
        let Some(settings) = settings else { return Ok(()) };
        let deadline = Instant::now() + FOCUS_PATIENCE;
        let mut focused = false;
        while !focused && Instant::now() < deadline {
            focus = shell("dumpsys window | grep -m1 mCurrentFocus").trim().to_string();
            if focus.is_empty() {
                break;
            }
            focused = focus.contains(&format!(" {settings}/"));
            if !focused {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        shell(&format!("am force-stop {settings}"));
        if focused || focus.is_empty() {
            return Ok(());
        }
    }
    fail!(
        format!("no app window takes the focus on {device}: Settings was opened twice and the focus stayed elsewhere ({focus}), so touches and popups would not reach the app under test"),
        "no_focus",
        format!("ddp down {device} and ddp up again, or --no-focus-check to run anyway")
    )
}

/// Every test the run was asked for, as the runner names them without running them (`-e log true`). None when the
/// runner did not get to the end of the list.
fn listed_tests(adb_program: &str, device: &str, runner: &str, asked: &[String]) -> Option<Vec<String>> {
    let mut command: Vec<&str> = vec!["-s", device, "shell", "am", "instrument", "-w", "-r", "-e", "log", "true"];
    // A listener changes nothing about which tests run, and one that resets the device per test would do that for
    // every name listed.
    let mut words = asked.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match (word, words.clone().next().map(|key| key.trim_matches('\''))) {
            ("-e", Some("listener")) => {
                words.nth(1);
            }
            _ => command.push(word),
        }
    }
    command.push(runner);
    let listed = crate::up::capture_within(adb_program, &command, Duration::from_secs(120))?;
    let run = instrumentation_run(&listed.stdout);
    (run.stopped.is_none() && !run.finished.is_empty()).then_some(run.finished)
}

/// Where the tests of a continued run are named, on the device: AndroidJUnitRunner's `-e testFile`.
const TEST_FILE: &str = "/data/local/tmp/ddp-tests.txt";

/// Puts the names of the tests still to run on the device.
fn push_test_file(adb_program: &str, device: &str, tests: &[&String]) -> bool {
    let local = std::env::temp_dir().join(format!("ddp-tests-{}-{device}.txt", std::process::id()));
    let lines: String = tests.iter().map(|test| format!("{test}\n")).collect();
    let pushed = std::fs::write(&local, lines).is_ok()
        && capture(adb_program, &["-s", device, "push", &local.to_string_lossy(), TEST_FILE]).code == 0;
    let _ = std::fs::remove_file(local);
    pushed
}

/// Starts a lost emulator again and makes it ready for the rest of the run: the APKs, the focus. What that prints is
/// not the run's answer, so it is not printed.
fn start_again(adb_program: &str, device: &str, plan: &TestPlan) -> Result<()> {
    state::push_capture(false);
    let back = crate::up::restart(device)
        .and_then(|_| plan.installs.iter().try_for_each(|apk| crate::install::apk(device, apk).map(|_| ())))
        .and_then(|_| if plan.check_focus { takes_focus(adb_program, device) } else { Ok(()) });
    state::pop_capture();
    back
}

/// The app's instrumentation on this device: the one named, or the only one that targets the app.
fn instrumentation_of(device: &str, app: &str, named: Option<&String>) -> Result<String> {
    let listed = adb(device, &["shell", "pm", "list", "instrumentation"])?.stdout;
    let target_note = format!("(target={app})");
    let runners: Vec<String> = listed
        .lines()
        .filter_map(|line| line.trim().strip_prefix("instrumentation:"))
        .filter(|line| line.ends_with(&target_note))
        .filter_map(|line| line.split_whitespace().next().map(String::from))
        .collect();
    match (named, runners.as_slice()) {
        (Some(runner), _) => Ok(runner.clone()),
        (None, [one]) => Ok(one.clone()),
        (None, []) => fail!(
            format!("{app} has no instrumentation on {device}"),
            "no_tests",
            "install the test APK: ./gradlew installDebugAndroidTest"
        ),
        (None, many) => fail!(
            format!("{app} has {} instrumentations on {device}: {}", many.len(), many.join(", ")),
            "ambiguous",
            "--runner <one of them>"
        ),
    }
}

/// The `am instrument` arguments the run was asked with: the class filter, and each `--env` pair.
fn instrumentation_arguments(args: &Args) -> Result<(Vec<String>, Vec<String>)> {
    let filter = match &args.filter {
        Some(class) => vec!["-e".into(), "class".into(), crate::live::shell_quoted(class)],
        None => Vec::new(),
    };
    let mut environment = Vec::new();
    for pair in &args.env {
        let Some((key, value)) = pair.split_once('=').filter(|(key, _)| !key.is_empty()) else {
            fail!(format!("--env takes KEY=VALUE, not '{pair}'"), "bad_value", "--env size=small");
        };
        environment.extend(["-e".into(), crate::live::shell_quoted(key), crate::live::shell_quoted(value)]);
    }
    Ok((filter, environment))
}

/// What one device runs: what the run was asked for (a run on one device), or tests ddp named (its share of a run
/// over several devices).
enum Part<'a> {
    Asked(&'a [String]),
    Named(&'a [String]),
}

impl Part<'_> {
    /// Every test in this part, for going on after the run ended early. None when the runner could not list them.
    fn tests(&self, adb_program: &str, device: &str, runner: &str) -> Option<Vec<String>> {
        match self {
            Part::Asked(asked) => listed_tests(adb_program, device, runner, asked),
            Part::Named(tests) => Some(tests.to_vec()),
        }
    }
}

fn can_start_again(device: &str, plan: &TestPlan, restarts: &[Restart]) -> bool {
    restarts.len() < plan.restarts && !plan.installs.is_empty() && crate::up::can_restart(device)
}

/// Runs the instrumentation on one device: what was asked, or the tests named for it.
fn run_tests_on(device: &str, runner: &str, part: &Part, plan: &TestPlan) -> Result<DeviceRun> {
    let adb_program = adb_path()?;
    let started = Instant::now();
    let since = |moment: Instant| py::round_f(moment.elapsed().as_secs_f64(), 1);
    let mut done = DeviceRun {
        device: device.to_string(),
        run: TestRun::default(),
        seconds: 0.0,
        runner_start: None,
        focus: None,
        restarts: Vec::new(),
        times: Vec::new(),
    };
    // A device whose share of the split is empty has nothing to be ready for.
    if matches!(part, Part::Named(tests) if tests.is_empty()) {
        return Ok(done);
    }
    if plan.check_focus {
        if let Err(no_focus) = takes_focus(&adb_program, device) {
            if !can_start_again(device, plan, &done.restarts) {
                return Err(no_focus);
            }
            let again = Instant::now();
            start_again(&adb_program, device, plan)?;
            done.restarts.push(Restart { why: no_focus.message, test: None, seconds: since(again) });
        }
        done.focus = Some(since(started));
    }
    run_part(&adb_program, device, runner, part, plan, &mut done)?;
    if plan.rerun_failed && done.run.stopped.is_none() {
        rerun_failed(&adb_program, device, runner, plan, &mut done.run)?;
    }
    done.seconds = since(started);
    Ok(done)
}

/// Runs one part on a device. A run that ends early goes on with the tests that are left, named in a file for the
/// runner:
/// - the device was lost (it died, froze, or the run stalled): an emulator `ddp up` made is started again and the
///   test that was running runs once more; lost in it a second time, that test is given up on;
/// - the app's process crashed: that test failed, and the rest runs on the same device;
/// - the output just ended and the device answers: that test runs once more, then it is given up on.
fn run_part(
    adb_program: &str,
    device: &str,
    runner: &str,
    part: &Part,
    plan: &TestPlan,
    done: &mut DeviceRun,
) -> Result<()> {
    let since = |moment: Instant| py::round_f(moment.elapsed().as_secs_f64(), 1);
    let total = &mut done.run;
    let asked: Vec<String> = match part {
        Part::Asked(asked) => asked.to_vec(),
        Part::Named(_) => Vec::new(),
    };
    // Named tests go to the runner in the file from the start, like the rest of a run that went on.
    let mut continued = matches!(part, Part::Named(_));
    if let Part::Named(tests) = part {
        if !push_test_file(adb_program, device, &tests.iter().collect::<Vec<_>>()) {
            total.stopped = Some((format!("the {} tests could not be named to the runner", tests.len()), None));
            return Ok(());
        }
    }
    // The test the run last ended in without a verdict on it: ending there again is its last chance.
    let mut ended_in: Option<String> = None;
    loop {
        let mut command: Vec<&str> = vec!["-s", device, "shell", "am", "instrument", "-w", "-r"];
        if continued {
            command.extend(plan.environment.iter().map(String::as_str));
            command.extend(["-e", "testFile", TEST_FILE]);
        } else {
            command.extend(asked.iter().map(String::as_str));
        }
        command.push(runner);
        let Instrumented { output, stopped: stopped_from_outside, first_line, line_times } =
            instrument(adb_program, &command, device, plan.app, plan.stall)?;
        done.runner_start = done.runner_start.or(first_line.map(|waited| py::round_f(waited.as_secs_f64(), 1)));
        done.times.extend(test_seconds(&output, &line_times));
        let run = instrumentation_run(&output);
        total.passed += run.passed;
        total.passed_tests.extend(run.passed_tests);
        total.skipped += run.skipped;
        total.skipped_tests.extend(run.skipped_tests);
        total.failed.extend(run.failed);
        total.finished.extend(run.finished);
        // Why it ended early, the test running then, and whether the device is lost. An emulator that dies takes the
        // output with it before ddp asks, so a run that just ended asks now.
        let (why, running, lost) = match (stopped_from_outside, run.stopped) {
            (None, None) => break,
            (Some(why), stopped) => (why, stopped.and_then(|(_, test)| test), true),
            (None, Some((why, running))) if why == NO_RESULT => match device_gone(adb_program, device) {
                Some(gone) => (gone, running, true),
                None => (why, running, false),
            },
            (None, Some((why, running))) => (why, running, false),
        };
        let crashed = !lost && why != NO_RESULT;
        if lost {
            if !can_start_again(device, plan, &done.restarts) {
                total.stopped = Some((why, running));
                break;
            }
            let again = Instant::now();
            if let Err(not_back) = start_again(adb_program, device, plan) {
                total.stopped = Some((format!("{why} Starting it again failed: {}", not_back.message), running));
                break;
            }
            done.restarts.push(Restart { why: why.clone(), test: running.clone(), seconds: since(again) });
        }
        let Some(test) = running else {
            // Before the first test or between two: nothing to hold against a test. On a new emulator the run goes on.
            if lost {
                continued = true;
            } else {
                total.stopped = Some((why, None));
                break;
            }
            ended_in = None;
            if !continue_with_the_rest(adb_program, device, runner, part, total) {
                break;
            }
            continue;
        };
        if crashed {
            total.failed.push((test.clone(), why));
            total.finished.push(test);
        } else if ended_in.as_ref() == Some(&test) {
            total.failed.push((test.clone(), format!("the run ended in this test both times it ran: {why}")));
            total.finished.push(test);
        } else {
            ended_in = Some(test);
        }
        continued = true;
        if !continue_with_the_rest(adb_program, device, runner, part, total) {
            break;
        }
    }
    // A run that names its tests to the runner: one it did not take (a name the runner filters another way) would be
    // missing from the counts without a word.
    if continued && total.stopped.is_none() {
        let never_ran: Vec<String> = part
            .tests(adb_program, device, runner)
            .unwrap_or_default()
            .into_iter()
            .filter(|test| !total.finished.contains(test))
            .collect();
        if let Some(first) = never_ran.first() {
            total.stopped = Some((
                format!("{} of the tests named to the runner did not run, {first} the first", never_ran.len()),
                None,
            ));
        }
    }
    Ok(())
}

/// Runs each failed test once more, on the same device. One that passes then moves to `flaky` and counts as passed;
/// any other answer (failed again, skipped, the run cut short) leaves it failed, with its first failure's line.
fn rerun_failed(adb_program: &str, device: &str, runner: &str, plan: &TestPlan, total: &mut TestRun) -> Result<()> {
    let failed_tests: Vec<&String> = total.failed.iter().map(|(test, _)| test).collect();
    if failed_tests.is_empty() || !push_test_file(adb_program, device, &failed_tests) {
        return Ok(());
    }
    let mut command: Vec<&str> = vec!["-s", device, "shell", "am", "instrument", "-w", "-r"];
    command.extend(plan.environment.iter().map(String::as_str));
    command.extend(["-e", "testFile", TEST_FILE, runner]);
    let again = instrumentation_run(&instrument(adb_program, &command, device, plan.app, plan.stall)?.output);
    let (flaky, still_failed) =
        std::mem::take(&mut total.failed).into_iter().partition(|(test, _)| again.passed_tests.contains(test));
    total.failed = still_failed;
    total.passed += flaky.len();
    total.flaky = flaky;
    Ok(())
}

/// Names the tests that have no verdict yet to the runner, for the next part of the run. False when there are none
/// left, or when they could not be named: then `stopped` says so.
fn continue_with_the_rest(adb_program: &str, device: &str, runner: &str, part: &Part, total: &mut TestRun) -> bool {
    let Some(all) = part.tests(adb_program, device, runner) else {
        total.stopped =
            Some(("the run ended early and the tests that were left could not be listed".to_string(), None));
        return false;
    };
    let left: Vec<&String> = all.iter().filter(|test| !total.finished.contains(test)).collect();
    if left.is_empty() {
        return false;
    }
    if !push_test_file(adb_program, device, &left) {
        total.stopped = Some((
            format!("the run ended early and the {} tests left could not be named to the runner", left.len()),
            None,
        ));
        return false;
    }
    true
}

/// How long each test took, from when the runner said it started to its verdict, as ddp read the lines (`at`: the
/// seconds of each output line).
fn test_seconds(output: &str, at: &[f64]) -> Vec<(String, f64)> {
    let mut times = Vec::new();
    let (mut class, mut name) = (String::new(), String::new());
    let mut began: Option<(String, f64)> = None;
    for (line, seconds) in output.lines().zip(at) {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("INSTRUMENTATION_STATUS: class=") {
            class = value.to_string();
        } else if let Some(value) = line.strip_prefix("INSTRUMENTATION_STATUS: test=") {
            name = value.to_string();
        } else if let Some(code) = line.strip_prefix("INSTRUMENTATION_STATUS_CODE: ") {
            let test = format!("{class}#{name}");
            if code.trim() == "1" {
                began = Some((test, *seconds));
            } else if let Some((started, from)) = began.take().filter(|(started, _)| *started == test) {
                times.push((started, py::round_f(seconds - from, 2)));
            }
        }
    }
    times
}

/// Where ddp keeps how long each of an app's tests took when it last ran: in the home folder, so every folder (a
/// gate's fresh worktree too) splits its next run by them.
fn test_times_file(app: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let name: String =
        app.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    Some(std::path::PathBuf::from(home).join(".ddp").join("test-times").join(format!("{name}.json")))
}

fn known_test_times(app: &str) -> std::collections::HashMap<String, f64> {
    test_times_file(app)
        .and_then(|file| read_json(&file))
        .and_then(|times| serde_json::from_value(times).ok())
        .unwrap_or_default()
}

/// Adds this run's times to the known ones. Written to a new file and moved over the old one, so two runs at once
/// never leave half a file. Unsure is no failure: without the file the next split is by count.
fn save_test_times(app: &str, times: impl Iterator<Item = (String, f64)>) {
    let Some(file) = test_times_file(app) else { return };
    let mut known = known_test_times(app);
    known.extend(times);
    let fresh = file.with_extension(format!("json.{}", std::process::id()));
    if write_file(&fresh, serde_json::to_string(&known).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&fresh, &file);
    }
}

/// The tests split over `devices` by how long each took when it last ran, whole classes each (a class's setup runs
/// once), the longest class first onto the device with the least time so far: one runner start per device, and the
/// devices end close together. A test with no time yet counts as the mean of those with one, or 1 s when none has.
fn split_by_time(tests: &[String], known: &std::collections::HashMap<String, f64>, devices: usize) -> Vec<Vec<String>> {
    let mut classes: Vec<Vec<String>> = Vec::new();
    let mut class_at: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for test in tests {
        let class = test.split('#').next().unwrap_or(test);
        let at = *class_at.entry(class).or_insert_with(|| {
            classes.push(Vec::new());
            classes.len() - 1
        });
        classes[at].push(test.clone());
    }
    let timed: Vec<f64> = tests.iter().filter_map(|test| known.get(test).copied()).collect();
    let mean = if timed.is_empty() { 1.0 } else { timed.iter().sum::<f64>() / timed.len() as f64 };
    let mut weighed: Vec<(f64, Vec<String>)> = classes
        .into_iter()
        .map(|class| (class.iter().map(|test| known.get(test).copied().unwrap_or(mean)).sum(), class))
        .collect();
    // Stable: classes that weigh the same keep the order the runner listed them in.
    weighed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut parts: Vec<(f64, Vec<String>)> = vec![(0.0, Vec::new()); devices];
    for (seconds, class) in weighed {
        if let Some(lightest) = parts.iter_mut().min_by(|a, b| a.0.total_cmp(&b.0)) {
            lightest.0 += seconds;
            lightest.1.extend(class);
        }
    }
    parts.into_iter().map(|(_, tests)| tests).collect()
}

/// Where a test run's time went, phase by phase, as far as ddp saw it: the boot only for emulators `ddp up` made
/// (their lease keeps it), the install only with `--install`.
fn test_phases(
    devices: &[String],
    install: Option<(f64, usize, usize)>,
    listing: Option<f64>,
    runs: &[DeviceRun],
    seconds: f64,
) -> (String, Value) {
    let mut said = Vec::new();
    let mut phases = json!({});
    let boots: Vec<(f64, String)> = devices
        .iter()
        .filter_map(|device| {
            let lease = crate::lease::standing(device)?;
            Some((lease["boot_s"].as_f64()?, py::str(&lease["boot"])))
        })
        .collect();
    // With several devices, the slowest boot: they started side by side.
    if let Some((boot, from)) = boots.iter().max_by(|a, b| a.0.total_cmp(&b.0)).filter(|_| boots.len() == devices.len())
    {
        let from_what = if from == "cold" { "cold".to_string() } else { format!("from the snapshot {from}") };
        said.push(format!("boot {boot}s ({from_what}, at ddp up)"));
        phases["boot"] = json!(boot);
        phases["boot_from"] = json!(from);
    }
    if let Some((install, apks, unchanged)) = install {
        said.push(format!("install {install}s ({unchanged} of {apks} unchanged)"));
        phases["install"] = json!(install);
        phases["apks"] = json!(apks);
        phases["unchanged"] = json!(unchanged);
    }
    // With several devices each phase is its slowest device's: they ran side by side.
    let slowest = |of: &dyn Fn(&DeviceRun) -> Option<f64>| runs.iter().filter_map(of).max_by(f64::total_cmp);
    let mut tests = seconds;
    if let Some(listing) = listing {
        said.push(format!("listing {listing}s"));
        phases["listing"] = json!(listing);
        tests -= listing;
    }
    if let Some(focus) = slowest(&|run| run.focus) {
        said.push(format!("focus {focus}s"));
        phases["focus"] = json!(focus);
        tests -= focus;
    }
    let starts = runs.iter().filter(|run| run.runner_start.is_some()).count();
    if let Some(start) = slowest(&|run| run.runner_start).filter(|_| starts == runs.len()) {
        said.push(format!("runner start {start}s"));
        phases["runner_start"] = json!(start);
        tests -= start;
    }
    let restarted = |run: &DeviceRun| (!run.restarts.is_empty()).then(|| run.restarts.iter().map(|r| r.seconds).sum());
    if let Some(restarts) = slowest(&restarted) {
        let count: usize = runs.iter().map(|run| run.restarts.len()).sum();
        let restarts = py::round_f(restarts, 1);
        said.push(format!("restarts {restarts}s ({count})"));
        phases["restarts"] = json!(restarts);
        tests -= restarts;
    }
    let tests = py::round_f(tests.max(0.0), 1);
    said.push(format!("tests {tests}s"));
    phases["tests"] = json!(tests);
    (format!("phases: {}", said.join(", ")), phases)
}

/// Runs the app's instrumentation tests and counts what passed. It frees UiAutomation from ddp's runner first.
/// With `--on <device>,<device>` the run is split over those devices, one shard each at the same time, and the
/// counts are joined; a failing test says which device ran it.
pub fn test(args: &mut Args) -> Result<i32> {
    let devices: Vec<String> =
        args.on.as_deref().map(|on| on.split(',').map(str::trim).map(String::from).collect()).unwrap_or_default();
    if let Some(first) = devices.first() {
        if devices.iter().any(String::is_empty) || devices.len() < 2 {
            fail!(
                "--on takes two or more devices",
                "bad_value",
                "ddp test --on emulator-5560,emulator-5562 --app <id>"
            );
        }
        if let Some(twice) = devices.iter().enumerate().find(|(at, device)| devices[..*at].contains(device)) {
            fail!(format!("--on names {} twice", twice.1), "bad_value", "one shard per device: name each once");
        }
        // The app and the platform come from the first device, as for a run on it alone.
        args.device = Some(first.clone());
    }
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    if platform == "ios" {
        fail!(
            "iOS tests run from Xcode's build, not from the simulator",
            "not_supported",
            format!("xcodebuild test-without-building -xctestrun <file> -destination id={device}")
        );
    }
    let devices = if devices.is_empty() { vec![device] } else { devices };
    // Installed first: the test APK is one of them. With --json the answer is one JSON value on stdout, so what the
    // installs print goes beside it.
    let install_started = Instant::now();
    if ctx().json {
        state::push_capture(false);
    }
    let installs = devices
        .iter()
        .flat_map(|device| args.installs.iter().map(move |apk| crate::install::apk(device, apk)))
        .collect::<Result<Vec<bool>>>();
    if ctx().json {
        let printed = state::pop_capture();
        if !printed.trim().is_empty() {
            warn!("{}", printed.trim_end());
        }
    }
    let installs = installs?;
    let install = (!installs.is_empty()).then(|| {
        let unchanged = installs.iter().filter(|sent| !**sent).count();
        (py::round_f(install_started.elapsed().as_secs_f64(), 1), installs.len(), unchanged)
    });
    // Every device is asked before any test starts: half a run on the devices that have the tests is no run.
    let runners: Vec<String> =
        devices.iter().map(|device| instrumentation_of(device, &app, args.name.as_ref())).collect::<Result<_>>()?;
    let (filter, environment) = instrumentation_arguments(args)?;
    let stall = args.stall.as_deref().map(state::parse_ms).transpose()?.map(|ms| Duration::from_millis(ms as u64));
    let plan = TestPlan {
        app: &app,
        environment: &environment,
        stall,
        installs: &args.installs,
        restarts: args.restarts.unwrap_or(DEFAULT_RESTARTS),
        check_focus: !args.no_focus_check,
        rerun_failed: args.rerun_failed,
    };
    for device in &devices {
        if crate::runner::stop(device).unwrap_or(false) {
            // With --json the answer is one JSON value on stdout: the note goes beside it.
            let note = format!("ddp's runner stopped on {device}: UiAutomation is free for the tests");
            if ctx().json {
                warn!("{note}");
            } else {
                say!("{note}");
            }
        }
    }
    let asked: Vec<String> = filter.into_iter().chain(environment.iter().cloned()).collect();
    let started = Instant::now();
    let shards = devices.len();
    let mut listing = None;
    let mut runs: Vec<DeviceRun> = if shards == 1 {
        vec![run_tests_on(&devices[0], &runners[0], &Part::Asked(&asked), &plan)?]
    } else {
        // Split by time, not by AndroidJUnitRunner's numShards: that one splits by count, and on one suite of 360
        // tests its two halves took 205 s and 264 s.
        let Some(tests) = listed_tests(&adb_path()?, &devices[0], &runners[0], &asked) else {
            fail!(
                format!("the runner on {} could not list the tests, so the run could not be split", devices[0]),
                "no_tests",
                format!("ddp test --device {} --app {app} --class <one class>", devices[0])
            );
        };
        listing = Some(py::round_f(started.elapsed().as_secs_f64(), 1));
        let parts = split_by_time(&tests, &known_test_times(&app), shards);
        std::thread::scope(|scope| {
            let running: Vec<_> = devices
                .iter()
                .zip(&runners)
                .zip(&parts)
                .map(|((device, runner), part)| {
                    let (plan, context) = (&plan, ctx());
                    scope.spawn(move || {
                        // The session's folder and output mode are per thread.
                        state::use_context(context.session_name.as_deref(), context.json);
                        run_tests_on(device, runner, &Part::Named(part), plan)
                    })
                })
                .collect();
            running
                .into_iter()
                .map(|run| {
                    run.join().unwrap_or_else(|_| fail!("a device's test run ended unexpectedly", "command_failed"))
                })
                .collect::<Result<Vec<DeviceRun>>>()
        })?
    };
    let seconds = py::round_f(started.elapsed().as_secs_f64(), 1);
    save_test_times(&app, runs.iter().flat_map(|r| r.times.iter().cloned()));
    // A run with no test in it proves nothing, so it is no pass: a filter that matched none, or a device that went
    // away before the runner said a word.
    let tests_in = |r: &DeviceRun| r.run.passed + r.run.skipped + r.run.failed.len();
    if runs.iter().all(|r| tests_in(r) == 0 && r.run.stopped.is_none()) {
        let adb_program = adb_path()?;
        let why = devices.iter().find_map(|device| device_gone(&adb_program, device)).unwrap_or_else(|| {
            "no test ran: the runner found none to run (check --class, and notClass in --env)".to_string()
        });
        runs[0].run.stopped = Some((why, None));
    }
    let runner = &runners[0];
    let passed: usize = runs.iter().map(|r| r.run.passed).sum();
    let skipped: usize = runs.iter().map(|r| r.run.skipped).sum();
    let failures: usize = runs.iter().map(|r| r.run.failed.len()).sum();
    let flaky: usize = runs.iter().map(|r| r.run.flaky.len()).sum();
    let all_ran = runs.iter().all(|r| r.run.failed.is_empty() && r.run.stopped.is_none());
    let (phases_line, phases) = test_phases(&devices, install, listing, &runs, seconds);
    if ctx().json {
        let on = |device: &str, mut entry: Value| {
            if shards > 1 {
                entry["device"] = json!(device);
            }
            entry
        };
        let failed: Vec<Value> = runs
            .iter()
            .flat_map(|r| {
                r.run.failed.iter().map(|(test, message)| on(&r.device, json!({"test": test, "message": message})))
            })
            .collect();
        let stopped: Vec<Value> = runs
            .iter()
            .filter_map(|r| r.run.stopped.as_ref().map(|(why, test)| on(&r.device, json!({"why": why, "test": test}))))
            .collect();
        let mut result = json!({"runner": runner, "passed": passed, "failed": failed, "skipped": skipped,
            "seconds": seconds, "phases": phases});
        let restarts: Vec<Value> = runs
            .iter()
            .flat_map(|r| {
                r.restarts.iter().map(|restart| {
                    on(&r.device, json!({"why": restart.why, "test": restart.test, "seconds": restart.seconds}))
                })
            })
            .collect();
        if !restarts.is_empty() {
            result["restarts"] = json!(restarts);
        }
        let skipped_tests: Vec<Value> = runs
            .iter()
            .flat_map(|r| r.run.skipped_tests.iter().map(|test| on(&r.device, json!({"test": test}))))
            .collect();
        if !skipped_tests.is_empty() {
            result["skipped_tests"] = json!(skipped_tests);
        }
        if flaky > 0 {
            result["flaky"] = runs
                .iter()
                .flat_map(|r| {
                    r.run.flaky.iter().map(|(test, message)| on(&r.device, json!({"test": test, "message": message})))
                })
                .collect();
        }
        if shards == 1 {
            result["stopped"] = stopped.into_iter().next().unwrap_or(Value::Null);
        } else {
            result["stopped"] = json!(stopped);
            result["devices"] = runs
                .iter()
                .map(|r| json!({"device": r.device, "passed": r.run.passed, "failed": r.run.failed.len(), "skipped": r.run.skipped, "seconds": r.seconds}))
                .collect();
        }
        say!("{}", py::dumps(&result));
        return Ok(if all_ran { 0 } else { 1 });
    }
    for device_run in &runs {
        let on = if shards > 1 { format!(" on {}", device_run.device) } else { String::new() };
        for restart in &device_run.restarts {
            let during = restart.test.as_ref().map(|test| format!(" (in {test})")).unwrap_or_default();
            say!("RESTARTED {} in {}s{during}: {}", device_run.device, restart.seconds, restart.why);
        }
        for (test, message) in &device_run.run.failed {
            say!("FAIL {test}{on}: {message}");
        }
        for (test, message) in &device_run.run.flaky {
            say!("FLAKY {test}{on}: failed first ({message}), passed when run again");
        }
        for test in &device_run.run.skipped_tests {
            say!("SKIPPED {test}{on}");
        }
        if let Some((why, test)) = &device_run.run.stopped {
            let on = on.trim_start();
            let place = match (test, on.is_empty()) {
                (Some(test), true) => format!("in {test}: "),
                (Some(test), false) => format!("{on} in {test}: "),
                (None, true) => String::new(),
                (None, false) => format!("{on}: "),
            };
            say!("STOPPED {place}{why}");
        }
    }
    say!("{phases_line}");
    let flaky = if flaky > 0 { format!(", {flaky} of the passed flaky") } else { String::new() };
    if shards == 1 {
        say!("{runner} in {seconds}s: {passed} passed, {failures} failed, {skipped} skipped{flaky}");
    } else {
        let each: Vec<String> = runs
            .iter()
            .map(|r| {
                format!(
                    "{}: {} passed, {} failed, {} skipped in {}s",
                    r.device,
                    r.run.passed,
                    r.run.failed.len(),
                    r.run.skipped,
                    r.seconds
                )
            })
            .collect();
        say!(
            "{runner} on {shards} devices in {seconds}s: {passed} passed, {failures} failed, {skipped} skipped{flaky} ({})",
            each.join("; ")
        );
    }
    Ok(if all_ran { 0 } else { 1 })
}

/// iOS content size categories as `simctl ui content_size` names them. `default` and `largest` are the names the
/// Android side takes too, so one script runs on both. simctl says nothing about a name it does not know.
fn ios_text_size(value: &str) -> Result<&str> {
    const SIZES: [&str; 12] = [
        "extra-small",
        "small",
        "medium",
        "large",
        "extra-large",
        "extra-extra-large",
        "extra-extra-extra-large",
        "accessibility-medium",
        "accessibility-large",
        "accessibility-extra-large",
        "accessibility-extra-extra-large",
        "accessibility-extra-extra-extra-large",
    ];
    match value {
        "default" => Ok("large"),
        "largest" => Ok("accessibility-extra-extra-extra-large"),
        size if SIZES.contains(&size) => Ok(size),
        _ => fail!(
            format!("iOS has no text size '{value}'"),
            "bad_value",
            format!("ddp set text-size default|largest, or one of {}", SIZES.join(", "))
        ),
    }
}

/// A BCP 47 tag as Foundation writes AppleLocale: the region after an underscore (es-ES -> es_ES, zh-Hans-CN ->
/// zh-Hans_CN, es-419 -> es_419); a tag with no region stays as it is.
fn apple_locale(tag: &str) -> String {
    match tag.rsplit_once('-') {
        Some((rest, region))
            if (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic()))
                || (region.len() == 3 && region.chars().all(|c| c.is_ascii_digit())) =>
        {
            format!("{rest}_{}", region.to_ascii_uppercase())
        }
        _ => tag.to_string(),
    }
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
            "text-size" => vec![base(&["ui", &device, "content_size", ios_text_size(&value)?])],
            "location" => vec![base(&["location", &device, "set", &lat_lon(&value)?.join(",")])],
            "orientation" => fail!(
                "the simulator has no public way to rotate from the host",
                "not_supported",
                "rotate with Simulator.app (Device > Rotate), or test landscape on Android"
            ),
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
            // AppleLanguages picks the strings, AppleLocale the region that formats dates and numbers.
            _ => {
                let domain = app.as_deref().unwrap_or("-g");
                vec![
                    base(&["spawn", &device, "defaults", "write", domain, "AppleLanguages", "-array", &value]),
                    base(&[
                        "spawn",
                        &device,
                        "defaults",
                        "write",
                        domain,
                        "AppleLocale",
                        "-string",
                        &apple_locale(&value),
                    ]),
                ]
            }
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
                    other if other.parse::<f64>().is_ok_and(|factor| factor > 0.0) => other,
                    _ => fail!(
                        format!("Android has no text size '{value}'"),
                        "bad_value",
                        "ddp set text-size small|default|large|largest, or a factor like 1.3"
                    ),
                };
                vec![base(&["settings", "put", "system", "font_scale", factor])]
            }
            // The emulator console takes longitude first.
            "location" => {
                let [lat, lon] = lat_lon(&value)?;
                vec![[adb.as_str(), "-s", &device, "emu", "geo", "fix", &lon, &lat]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()]
            }
            "orientation" => {
                let rotation = match value.as_str() {
                    "portrait" => "0",
                    "landscape" => "1",
                    _ => fail!("orientation is portrait or landscape", "bad_value", "ddp set orientation landscape"),
                };
                vec![
                    base(&["settings", "put", "system", "accelerometer_rotation", "0"]),
                    base(&["settings", "put", "system", "user_rotation", rotation]),
                ]
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

/// Back: Android's Back key; on iOS the edge swipe UIKit turns into "back", with the library's real finger.
pub fn back(args: &mut Args) -> Result<i32> {
    let (platform, device, _) = target(args)?;
    if crate::runner::device_mode(args) {
        return crate::runner::back(args);
    }
    if platform == "ios" {
        let session = require_session()?;
        let mut control = open(&session)?;
        let info = control.call("app.info", json!({}))?;
        let (width, height) = (f(&info["screen"][0]), f(&info["screen"][1]));
        if width <= 0.0 {
            fail!(
                "this library does not say the screen size",
                "library_too_old",
                "update the library to the same version as ddp"
            );
        }
        // From the very edge, like a thumb: UIKit's screen-edge recognizer starts only there.
        let (from, to) = ((1.0, height / 2.0), (width * 0.7, height / 2.0));
        control.call("input.touch", json!({"phase": "down", "x": from.0, "y": from.1}))?;
        for step in 1..=10 {
            std::thread::sleep(Duration::from_millis(25));
            let x = from.0 + (to.0 - from.0) * step as f64 / 10.0;
            control.call("input.touch", json!({"phase": "move", "x": x, "y": from.1}))?;
        }
        control.call("input.touch", json!({"phase": "up", "x": to.0, "y": to.1}))?;
        control.close();
        say!("back: an edge swipe on {device}; a screen without a back gesture does not move");
        return Ok(0);
    }
    let result = adb(&device, &["shell", "input", "keyevent", "KEYCODE_BACK"])?;
    if result.code != 0 {
        fail!(format!("adb input failed: {}", result.stderr.trim()), "command_failed");
    }
    say!("back: the Back key on {device}");
    Ok(0)
}

/// The app to the background for `--for`, then back, as a person switching apps does. The libraries put
/// `background` and `foreground` in History from the platform's own notifications.
pub fn background(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    let ms = args.duration.as_deref().map(parse_ms).transpose()?.unwrap_or(2000.0);
    let ios = platform == "ios";
    // iOS: opening another app sends this one to the background; launching it again without terminating brings it back.
    let away = if ios {
        capture("xcrun", &["simctl", "launch", &device, "com.apple.Preferences"])
    } else {
        adb(&device, &["shell", "input", "keyevent", "KEYCODE_HOME"])?
    };
    if away.code != 0 {
        fail!(format!("could not leave the app: {}", away.stderr.trim()), "command_failed");
    }
    std::thread::sleep(Duration::from_millis(ms as u64));
    let returned = if ios {
        capture("xcrun", &["simctl", "launch", &device, &app])
    } else {
        let component = crate::live::launcher_activity(&device, &app)?;
        adb(
            &device,
            &[
                "shell",
                "am",
                "start",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.LAUNCHER",
                "-n",
                &component,
            ],
        )?
    };
    if returned.code != 0 {
        fail!(format!("could not bring {app} back: {}", returned.stderr.trim()), "command_failed");
    }
    say!("{app} was in the background for {}ms and is back; ddp history shows what it did", ms as u64);
    Ok(0)
}

/// The app's executable name on the simulator, which its log lines carry as the process.
fn ios_executable(device: &str, app: &str) -> Result<String> {
    let info = state::run(&["xcrun", "simctl", "appinfo", device, app])?;
    Ok(info
        .lines()
        .find(|l| l.contains("CFBundleExecutable"))
        .and_then(|l| l.split_once('='))
        .map(|(_, v)| v.trim().trim_matches(|c| c == '"' || c == ';').to_string())
        .unwrap_or_else(|| app.rsplit('.').next().unwrap_or(app).to_string()))
}

/// What the platform itself warns about in the app's log: its words, not ddp's judgment, so a line here is a warning
/// the platform gave. iOS: SwiftUI and AttributeGraph runtime issues, Auto Layout conflicts, Main Thread Checker.
/// Android: StrictMode violations, skipped frames, leaked windows.
const IOS_AUDITS: [&str; 6] = [
    "Publishing changes from within view updates",
    "Modifying state during view update",
    "AttributeGraph: cycle detected",
    "Unable to simultaneously satisfy constraints",
    "Main Thread Checker",
    "was mutated while being enumerated",
];
const ANDROID_AUDITS: [&str; 4] =
    ["StrictMode policy violation", "Skipped ", "has leaked window", "A resource failed to call"];

pub fn audits(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    let since = args.since.clone();
    crate::state::parse_ms(&since)?;
    let found: Vec<String> = if platform == "ios" {
        let executable = ios_executable(&device, &app)?;
        let matches: Vec<String> = IOS_AUDITS.iter().map(|text| format!("eventMessage CONTAINS \"{text}\"")).collect();
        let predicate = format!("process == \"{executable}\" AND ({})", matches.join(" OR "));
        let output = state::run(&[
            "xcrun",
            "simctl",
            "spawn",
            &device,
            "log",
            "show",
            "--style",
            "compact",
            "--last",
            &since,
            "--predicate",
            &predicate,
        ])?;
        output
            .lines()
            .filter(|line| IOS_AUDITS.iter().any(|text| line.contains(text)))
            .map(|line| line.trim().to_string())
            .collect()
    } else {
        let pid = adb(&device, &["shell", "pidof", &app])?.stdout.trim().to_string();
        if pid.is_empty() {
            fail!(format!("{app} is not running"), "not_running", format!("ddp launch {device} --app {app}"));
        }
        let seconds = crate::state::parse_ms(&since)? / 1000.0;
        let output = adb(
            &device,
            &[
                "logcat",
                "-d",
                "-v",
                "epoch",
                &format!("--pid={pid}"),
                "-T",
                &format!("{:.0}", state::now_ms() / 1000.0 - seconds),
            ],
        )?
        .stdout;
        // Skipped frames is Choreographer's line; elsewhere "Skipped" can be anything.
        output
            .lines()
            .filter(|line| {
                ANDROID_AUDITS.iter().any(|text| line.contains(text))
                    && (!line.contains("Skipped ") || line.contains("Choreographer"))
            })
            .map(|line| line.trim().to_string())
            .collect()
    };
    if found.is_empty() {
        say!("no runtime warning from the platform in the last {since}");
        return Ok(0);
    }
    let shown = 30;
    found.iter().take(shown).for_each(|line| say!("WARNING {line}"));
    if found.len() > shown {
        say!("... {} more", found.len() - shown);
    }
    Ok(1)
}

/// `LAT,LON` as two numbers in range.
fn lat_lon(text: &str) -> Result<[String; 2]> {
    let numbers: Vec<f64> = text.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    match numbers[..] {
        [lat, lon] if lat.abs() <= 90.0 && lon.abs() <= 180.0 => Ok([lat.to_string(), lon.to_string()]),
        _ => fail!(format!("location is LAT,LON, not '{text}'"), "bad_value", "ddp set location 4.711,-74.072"),
    }
}

/// Simulated biometrics, the way the simulator's and the emulator's own menus do it. Neither confirms: an action, so
/// ddp says it sent it and the app's reaction is what to check next.
/// `permission grant|revoke <name>`: the platform's own switch for one of the app's permissions, before it asks.
/// iOS: `simctl privacy` and its service names (camera, photos, location, contacts, microphone...); Android: `pm` and
/// a runtime permission (CAMERA, or android.permission.CAMERA).
pub fn permission(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else {
        fail!("which app?", "missing_app", format!("ddp permission grant <name> --app <id> --device {device}"));
    };
    let action = args.action.clone().unwrap_or_default();
    let name = args.value.clone().unwrap_or_default();
    let (answer, named) = if platform == "ios" {
        // simctl privacy takes any bundle id: a grant for an app that is not there would be reported as done.
        if capture("xcrun", &["simctl", "get_app_container", &device, &app]).code != 0 {
            fail!(
                format!("{app} is not installed on {device}"),
                "not_found",
                format!("xcrun simctl listapps {device}")
            );
        }
        if name == "paste" {
            set_paste(&device, &app, action == "grant")?;
            let done = if action == "grant" { "allowed" } else { "denied" };
            say!("{done} {app} to paste from other apps on {device} (Settings: Paste from Other Apps)");
            return Ok(0);
        }
        (capture("xcrun", &["simctl", "privacy", &device, &action, &name, &app]), name.clone())
    } else {
        let full =
            if name.contains('.') { name.clone() } else { format!("android.permission.{}", name.to_uppercase()) };
        (adb(&device, &["shell", "pm", &action, &app, &full])?, full)
    };
    let said = format!("{}{}", answer.stdout, answer.stderr);
    // pm prints its refusal as an exception and, on older Android, still exits 0.
    if answer.code != 0 || said.contains("Exception") {
        // pm says "Exception occurred while executing 'grant':", then the exception with the reason.
        let lines: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        let why = lines
            .iter()
            .find_map(|line| line.split_once("Exception: ").map(|(_, reason)| reason))
            .or(lines.first().copied())
            .unwrap_or("no answer");
        fail!(
            format!("{action} {named} for {app} on {device} failed: {why}"),
            "command_failed",
            if platform == "ios" {
                "xcrun simctl privacy --help lists the service names".to_string()
            } else {
                format!("only a runtime permission the app declares: adb -s {device} shell dumpsys package {app} | grep permission")
            }
        );
    }
    // Android 16's pm exits 0 and says nothing for a permission the app does not declare: read the state back.
    if platform == "android" {
        let listed = adb(&device, &["shell", "dumpsys", "package", &app])?.stdout;
        let state = listed.lines().find_map(|line| line.trim().strip_prefix(&format!("{named}: granted=")));
        let wanted = if action == "grant" { "true" } else { "false" };
        if !state.is_some_and(|state| state.starts_with(wanted)) {
            let why = match state {
                Some(_) => format!("{named} is still granted={}", if action == "grant" { "false" } else { "true" }),
                None => format!("{app} does not ask for {named} (it is not a runtime permission in its manifest)"),
            };
            fail!(
                format!("{action} {named} for {app} on {device} did nothing: {why}"),
                "command_failed",
                format!("adb -s {device} shell dumpsys package {app} | grep granted=")
            );
        }
    }
    let done = if action == "grant" { "granted" } else { "revoked" };
    let note = if platform == "ios" {
        "; iOS may end a running app when its permissions change: ddp launch starts it again"
    } else {
        ""
    };
    say!("{done} {named} for {app} on {device}{note}");
    Ok(0)
}

pub const TCC_HOOK: &str = "simulator.tcc";
const PASTE_SERVICE: &str = "kTCCServicePasteboard";

/// The simulator's privacy store: iOS keeps an app's "Paste from Other Apps" there (simctl privacy has no service for
/// it). `auth_value` 2 is Allow, 0 Deny, 3 Ask.
fn tcc_db(udid: &str) -> Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let db =
        PathBuf::from(home).join(format!("Library/Developer/CoreSimulator/Devices/{udid}/data/Library/TCC/TCC.db"));
    if !db.exists() {
        fail!(format!("{TCC_HOOK}: no privacy store at {}", db.display()), "hook_failed", format!("ddp doctor {udid}"));
    }
    Ok(db)
}

fn tcc_sql(udid: &str, sql: &str) -> Result<String> {
    let db = tcc_db(udid)?;
    let answer = capture("sqlite3", &[&db.display().to_string(), sql]);
    if answer.code != 0 {
        fail!(format!("{TCC_HOOK}: {}", answer.stderr.trim()), "hook_failed", format!("ddp doctor {udid}"));
    }
    Ok(answer.stdout.trim().to_string())
}

/// Whether iOS lets the app paste what another app copied without asking the person.
pub fn paste_allowed(udid: &str, app: &str) -> Result<bool> {
    let app = app.replace('\'', "");
    let sql = format!("select auth_value from access where service = '{PASTE_SERVICE}' and client = '{app}'");
    Ok(tcc_sql(udid, &sql)? == "2")
}

/// For `ddp doctor`: the store answers and holds the table iOS reads.
pub fn tcc_answers(udid: &str) -> Result<()> {
    tcc_sql(udid, "select count(*) from access").map(|_| ())
}

/// `permission grant|revoke paste` on iOS: the app's "Paste from Other Apps" set to Allow or Deny, as Settings sets it.
fn set_paste(udid: &str, app: &str, allow: bool) -> Result<()> {
    let app = app.replace('\'', "");
    let value = if allow { 2 } else { 0 };
    tcc_sql(
        udid,
        &format!(
            "insert or replace into access (service, client, client_type, auth_value, auth_reason, auth_version, flags) \
             values ('{PASTE_SERVICE}', '{app}', 0, {value}, 4, 2, 0)"
        ),
    )?;
    if paste_allowed(udid, &app)? != allow {
        fail!(
            format!("{TCC_HOOK}: the paste setting for {app} did not change"),
            "hook_failed",
            format!("ddp doctor {udid}")
        );
    }
    Ok(())
}

/// The bundle id of the app with this process on a simulator, from its launchd job.
pub fn ios_bundle_of(udid: &str, pid: i32) -> Option<String> {
    let listed = capture("xcrun", &["simctl", "spawn", udid, "launchctl", "list"]).stdout;
    listed.lines().find_map(|line| {
        let mut columns = line.split_whitespace();
        (columns.next()? == pid.to_string()).then_some(())?;
        columns.nth(1)?.strip_prefix("UIKitApplication:")?.split('[').next().map(String::from)
    })
}

pub fn biometric(args: &mut Args) -> Result<i32> {
    let (platform, device, _) = target(args)?;
    let action = args.action.clone().unwrap_or_default();
    let result = if platform == "ios" {
        // The notifications Simulator.app's Features > Face ID menu posts (private names, used by test tools too).
        let name = match action.as_str() {
            "enroll" => "com.apple.BiometricKit.enrollmentChanged",
            "match" => "com.apple.BiometricKit_Sim.pearl.match",
            _ => "com.apple.BiometricKit_Sim.pearl.nomatch",
        };
        if action == "enroll" {
            capture("xcrun", &["simctl", "spawn", &device, "notifyutil", "-s", name, "1"]);
        }
        capture("xcrun", &["simctl", "spawn", &device, "notifyutil", "-p", name])
    } else {
        // The emulator console's finger touch: a fingerprint id enrolled in Settings matches, another one fails.
        let finger = match action.as_str() {
            "enroll" => fail!(
                "Android enrolls a fingerprint only through Settings",
                "not_supported",
                "add one in Settings > Security > Fingerprint, touching with: ddp biometric match"
            ),
            "match" => "1",
            _ => "99",
        };
        capture(&adb_path()?, &["-s", &device, "emu", "finger", "touch", finger])
    };
    if result.code != 0 {
        fail!(format!("could not send it: {}", result.stderr.trim()), "command_failed");
    }
    say!("biometric {action} sent to {platform} {device}; check what the app shows next");
    Ok(0)
}

/// iOS: a push notification delivered to the app as if APNs sent it (`simctl push`).
pub fn push(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    if platform != "ios" {
        fail!(
            "Android has no local way to deliver a push as FCM would",
            "not_supported",
            "send it from your FCM test console, or run the app's handler as an app command (ddp run)"
        );
    }
    let path = args.path.clone().unwrap_or_default();
    let result = capture("xcrun", &["simctl", "push", &device, &app, &path]);
    // The simulator refuses a push to an app that never got notification permission.
    if result.stderr.contains("not authorized") {
        fail!(
            format!("{app} has no notification permission, so iOS refused the push"),
            "not_authorized",
            "the app asks with UNUserNotificationCenter requestAuthorization; a .provisional request needs no tap"
        );
    }
    if result.code != 0 {
        fail!(format!("simctl push failed: {}", result.stderr.trim()), "command_failed");
    }
    say!("pushed {path} to {app} on {device}");
    Ok(0)
}

/// Android: one of the app's scheduled jobs, run now (`cmd jobscheduler run -f`). WorkManager runs on JobScheduler.
pub fn job(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    if platform == "ios" {
        fail!(
            "iOS runs a BGTaskScheduler task early only from the debugger",
            "not_supported",
            "pause in Xcode and run e -l objc -- (void)[[BGTaskScheduler sharedScheduler] _simulateLaunchForTaskWithIdentifier:@\"ID\"]"
        );
    }
    let id = args.value.clone().unwrap_or_default();
    let result = capture(&adb_path()?, &["-s", &device, "shell", "cmd", "jobscheduler", "run", "-f", &app, &id]);
    if result.code != 0 || result.stdout.contains("Unknown") || result.stdout.contains("not found") {
        fail!(format!("could not run job {id}: {}", (result.stdout + &result.stderr).trim()), "command_failed");
    }
    say!("ran job {id} of {app} on {device}");
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

/// What the device's display shows now, as a PNG: `simctl io screenshot` or `screencap`. Pixels of the whole screen.
pub fn screenshot_png(session: &Value) -> Result<Vec<u8>> {
    let device = py::str(&session["device"]);
    if session["platform"] == "ios" {
        let path = state::scratch_file("screen.png");
        state::run(&["xcrun", "simctl", "io", &device, "screenshot", "--type=png", &path.display().to_string()])?;
        return std::fs::read(&path).map_err(|e| failure(e.to_string(), "file_not_found", None));
    }
    let output = Command::new(adb_path()?)
        .args(["-s", &device, "exec-out", "screencap", "-p"])
        .output()
        .map_err(|e| failure(e.to_string(), "command_failed", None))?;
    if !output.status.success() {
        return Err(failure("screencap failed", "command_failed", None));
    }
    Ok(output.stdout)
}

/// `screenshot [out.png] [--of <element>]`: what the screen shows, as the device takes it, or the part one element
/// covers. With or without the library.
pub fn screenshot(args: &mut Args) -> Result<i32> {
    let (platform, device, _) = target(args)?;
    let out = PathBuf::from(&args.out);
    let Some(element) = args.id.clone() else {
        write_file(&out, screenshot_png(&json!({"platform": platform, "device": device}))?)?;
        say!("{}: the screen of {device}", out.display());
        return Ok(0);
    };
    let [x, y, w, h] = match crate::runner::device_mode(args) {
        true => crate::runner::element_pixels(args, &element)?,
        false => crate::compare::on_screen(std::slice::from_ref(&element))?[0],
    };
    let (width, height, rows) = png_read(&screenshot_png(&json!({"platform": platform, "device": device}))?)?;
    let (left, top) = (x.max(0.0).round() as usize, y.max(0.0).round() as usize);
    let (right, bottom) =
        (((x + w).round().max(0.0) as usize).min(width), ((y + h).round().max(0.0) as usize).min(height));
    if right <= left || bottom <= top {
        fail!(
            format!("'{element}' has no part on screen"),
            "off_screen",
            format!("ddp scroll down --to '{element}', then the screenshot again")
        );
    }
    let cut: Vec<Vec<u8>> = rows[top..bottom].iter().map(|row| row[left * 4..right * 4].to_vec()).collect();
    write_file(&out, png_write(right - left, bottom - top, &cut))?;
    say!("{}: {element} on {device}, {}x{} pixels at {left},{top}", out.display(), right - left, bottom - top);
    Ok(0)
}

pub fn annotate(args: &mut Args) -> Result<i32> {
    let session = require_session()?;
    let recording = snapshot_live(&session)?;
    let refs = save_refs(&recording.frames)?;
    let frame = recording.frames.last().unwrap();
    let out = PathBuf::from(&args.out);
    let scale = if session["platform"] == "ios" { recording.header.get("scale").map(f).unwrap_or(1.0) } else { 1.0 };
    let (width, height, mut rows) = png_read(&screenshot_png(&session)?)?;
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
    // A control no touch reaches would only show "changed nothing"; say why instead of tapping it.
    let unreachable = crate::hit::unreachable(&session)?;
    let mut control = open(&session)?;
    crate::inspect::say_hooks(&crate::inspect::troubled_hooks(&mut control));
    control.close();
    for (key, why) in &unreachable {
        if tappable.contains(key) {
            say!("{key}: not tapped, {why}");
        }
    }
    let tappable: Vec<String> = tappable.into_iter().filter(|key| !unreachable.iter().any(|(k, _)| k == key)).collect();
    if tappable.is_empty() {
        say!("no control left to explore");
        return Ok(0);
    }
    say!("exploring {} controls: {}", tappable.len(), tappable.join(", "));
    for key in &tappable {
        let name =
            format!("explore-{}", key.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>());
        let mut take_args = args.clone();
        take_args.duration = Some(args.per.clone());
        take_args.frames = None;
        take_args.name = Some(name.clone());
        take_args.shared = true;
        let mut then = state::self_command();
        // Checked above; --force skips tap's own checks so the take keeps its length.
        then.extend(["tap".into(), key.clone(), "--force".into(), "--max-output".into(), "0".into()]);
        if let Some(session_name) = &ctx().session_name {
            then.extend(["--session".into(), session_name.clone()]);
        }
        take_args.then = Some(state::shell_join(&then));
        let recording = record_live_inspected(&take_args)?;
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
    fn a_split_by_time_keeps_classes_whole_and_evens_the_devices_out() {
        let tests: Vec<String> = ["a.Long#one", "a.Long#two", "a.Mid#one", "a.Short#one", "a.Short#two", "a.New#one"]
            .map(String::from)
            .to_vec();
        let known: std::collections::HashMap<String, f64> = [
            ("a.Long#one", 50.0),
            ("a.Long#two", 30.0),
            ("a.Mid#one", 40.0),
            ("a.Short#one", 10.0),
            ("a.Short#two", 10.0),
        ]
        .map(|(test, seconds)| (test.to_string(), seconds))
        .into();
        // Long 80 to the first device; Mid 40, Short 20 and New (the mean, 28) to the second: 80 against 88.
        assert_eq!(
            split_by_time(&tests, &known, 2),
            vec![
                vec!["a.Long#one".to_string(), "a.Long#two".into()],
                vec!["a.Mid#one".to_string(), "a.New#one".into(), "a.Short#one".into(), "a.Short#two".into()],
            ]
        );
        // No times yet: by count, whole classes, the listing's order for classes that weigh the same.
        let by_count = split_by_time(&tests, &std::collections::HashMap::new(), 2);
        assert_eq!(by_count[0], vec!["a.Long#one".to_string(), "a.Long#two".into(), "a.Mid#one".into()]);
        assert_eq!(by_count[1], vec!["a.Short#one".to_string(), "a.Short#two".into(), "a.New#one".into()]);
        // More devices than classes: some get nothing.
        assert_eq!(split_by_time(&tests[..2], &known, 3).iter().filter(|part| part.is_empty()).count(), 2);
    }

    #[test]
    fn a_tests_time_runs_from_its_start_line_to_its_verdict() {
        let output =
            "INSTRUMENTATION_STATUS: class=a.B\nINSTRUMENTATION_STATUS: test=one\nINSTRUMENTATION_STATUS_CODE: 1\n\
            INSTRUMENTATION_STATUS: class=a.B\nINSTRUMENTATION_STATUS: test=one\nINSTRUMENTATION_STATUS_CODE: 0\n\
            INSTRUMENTATION_STATUS: class=a.B\nINSTRUMENTATION_STATUS: test=two\nINSTRUMENTATION_STATUS_CODE: 1\n";
        let at = [0.5, 0.5, 0.6, 2.0, 2.0, 2.1, 2.2, 2.2, 2.3];
        // `two` never got its verdict: no time for it.
        assert_eq!(test_seconds(output, &at), vec![("a.B#one".to_string(), 1.5)]);
    }

    #[test]
    fn an_instrumentation_run_counts_each_test_by_its_final_code() {
        let status = |test: &str, code: i32, extra: &str| {
            format!("INSTRUMENTATION_STATUS: class=dev.app.SampleTest\nINSTRUMENTATION_STATUS: current=1\n{extra}INSTRUMENTATION_STATUS: test={test}\nINSTRUMENTATION_STATUS_CODE: {code}\n")
        };
        let stack = "INSTRUMENTATION_STATUS: stack=org.junit.ComparisonFailure: expected:<[expected]> but was:<[actual]>\n\tat org.junit.Assert.assertEquals(Assert.java:117)\n";
        let passing = status("packageName", 1, "") + &status("packageName", 0, "")
            + &status("failsWhenAsked", 1, "") + &status("failsWhenAsked", -2, stack)
            + &status("skipped", 1, "") + &status("skipped", -4, "")
            + "INSTRUMENTATION_RESULT: stream=\nTime: 0.4\n\nFAILURES!!!\nTests run: 3,  Failures: 1\n\nINSTRUMENTATION_CODE: -1\n";
        assert_eq!(
            instrumentation_run(&passing),
            TestRun {
                passed: 1,
                skipped: 1,
                skipped_tests: vec!["dev.app.SampleTest#skipped".into()],
                failed: vec![(
                    "dev.app.SampleTest#failsWhenAsked".into(),
                    "org.junit.ComparisonFailure: expected:<[expected]> but was:<[actual]>".into()
                )],
                stopped: None,
                finished: ["packageName", "failsWhenAsked", "skipped"]
                    .map(|test| format!("dev.app.SampleTest#{test}"))
                    .to_vec(),
                passed_tests: vec!["dev.app.SampleTest#packageName".into()],
                flaky: Vec::new(),
            }
        );
        // The process dies in the middle of a test: no code for it, a short message for the run.
        let crashed = status("packageName", 1, "")
            + &status("packageName", 0, "")
            + &status("crashesWhenAsked", 1, "")
            + "INSTRUMENTATION_RESULT: shortMsg=Process crashed.\nINSTRUMENTATION_CODE: 0\n";
        let run = instrumentation_run(&crashed);
        assert_eq!(run.passed, 1);
        assert_eq!(run.stopped, Some(("Process crashed.".into(), Some("dev.app.SampleTest#crashesWhenAsked".into()))));
        let missing = "INSTRUMENTATION_FAILED: dev.app.test/androidx.test.runner.AndroidJUnitRunner\nINSTRUMENTATION_STATUS_CODE: -1\n";
        assert!(instrumentation_run(missing)
            .stopped
            .is_some_and(|(why, _)| why.starts_with("the instrumentation did not run")));
    }

    #[test]
    fn ps_elapsed_time_in_seconds() {
        assert_eq!(elapsed_seconds("01:05"), Some(65.0));
        assert_eq!(elapsed_seconds("02:00:01"), Some(7201.0));
        assert_eq!(elapsed_seconds("1-00:00:00"), Some(86400.0));
        assert_eq!(elapsed_seconds("soon"), None);
    }

    #[test]
    fn apple_locale_takes_the_region_after_an_underscore() {
        assert_eq!(apple_locale("es-ES"), "es_ES");
        assert_eq!(apple_locale("zh-Hans-CN"), "zh-Hans_CN");
        assert_eq!(apple_locale("es-419"), "es_419");
        assert_eq!(apple_locale("es"), "es");
        assert_eq!(apple_locale("zh-Hans"), "zh-Hans");
    }

    #[test]
    fn png_round_trip() {
        let rows = vec![[0x10u8, 0x20, 0x30, 0xff].repeat(5); 4];
        let (width, height, back) = png_read(&png_write(5, 4, &rows)).unwrap();
        assert_eq!((width, height), (5, 4));
        assert_eq!(back, rows);
    }

    #[test]
    fn ios_text_sizes() {
        assert_eq!(ios_text_size("default").unwrap(), "large");
        assert_eq!(ios_text_size("largest").unwrap(), "accessibility-extra-extra-extra-large");
        assert_eq!(ios_text_size("accessibility-large").unwrap(), "accessibility-large");
        assert_eq!(ios_text_size("huge").unwrap_err().code, "bad_value");
    }
}
