//! Walks: a task done step by step on the app, kept as a file. `walk start "<task>"` opens one; while it is open every
//! action (tap, double-tap, long-press, swipe, drag, type, press) is a step with what it touched, what changed and a
//! screenshot; `walk expect <element>` adds a check; `walk end pass|fail` closes it with one line per step.
//! `walk replay` does the steps again with no model: each target is found again by its id or label, never by its old
//! pixels, and a replay fails only when a target or an expected element is not on screen. ddp calls no model: the
//! agent that walks is the model.
use crate::args::Args;
use crate::model::element_key;
use crate::py;
use crate::state::{ctx, failure, read_json, write_file, Result};
use crate::{fail, say};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The commands that are steps.
const ACTIONS: [&str; 7] = ["tap", "double-tap", "long-press", "swipe", "drag", "type", "press"];
/// How long a replay waits for a target or an expected element to show: the step before may still be moving, or
/// waiting on the network.
const FIND_WAIT: Duration = Duration::from_millis(5000);
const LOOK_EVERY: Duration = Duration::from_millis(200);
/// Same platform: an element more than this much bigger or smaller than the one walked is another element.
const SIZE_RATIO: f64 = 2.5;

thread_local! {
    static NOTE: RefCell<Option<Value>> = const { RefCell::new(None) };
    static REPLAYING: Cell<bool> = const { Cell::new(false) };
}

/// What an action did, in the words a replay needs: the places it was given and what was seen there, what changed.
/// The action itself calls this; `record` writes it into the open walk.
pub fn note(step: Value) {
    NOTE.with(|n| *n.borrow_mut() = Some(step));
}

/// Whether a replay is running: its actions take one more look before they say what appeared (see `appeared`).
pub fn replaying() -> bool {
    REPLAYING.with(Cell::get)
}

fn take_note() -> Option<Value> {
    NOTE.with(|n| n.borrow_mut().take())
}

fn pointer() -> PathBuf {
    ctx().dir.join("walk.json")
}

fn walks_dir() -> PathBuf {
    ctx().dir.join("walks")
}

/// The open walk's file, if one is open.
fn open_walk() -> Option<PathBuf> {
    read_json(&pointer()).and_then(|p| p["file"].as_str().map(PathBuf::from)).filter(|f| f.exists())
}

/// Whether this command is a step of an open walk (a replay records nothing).
pub fn records(command: &str) -> bool {
    ACTIONS.contains(&command) && !REPLAYING.with(Cell::get) && open_walk().is_some()
}

fn lines(file: &Path) -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| failure(format!("can't read {}: {e}", file.display()), "io_error", Some("ddp walk list")))?;
    Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

fn append(file: &Path, line: &Value) -> Result<()> {
    use std::io::Write;
    let mut handle = std::fs::OpenOptions::new()
        .append(true)
        .open(file)
        .map_err(|e| failure(format!("can't write {}: {e}", file.display()), "io_error", None))?;
    writeln!(handle, "{}", py::dumps(line))
        .map_err(|e| failure(format!("can't write {}: {e}", file.display()), "io_error", None))
}

/// The screen after a step, taken in the background so the step does not wait for it.
fn screenshot(platform: &str, device: &str, path: &Path) {
    use std::process::{Command, Stdio};
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if platform == "ios" {
        let _ = Command::new("xcrun")
            .args(["simctl", "io", device, "screenshot", &path.to_string_lossy()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else if let (Ok(adb), Ok(file)) = (crate::state::adb_path(), std::fs::File::create(path)) {
        let _ = Command::new(adb)
            .args(["-s", device, "exec-out", "screencap", "-p"])
            .stdout(file)
            .stderr(Stdio::null())
            .spawn();
    }
}

/// Writes a finished action into the open walk: its command, its line, what it noted, a screenshot path.
pub fn record(argv: &[String], output: &str) -> Result<()> {
    let Some(file) = open_walk() else { return Ok(()) };
    let all = lines(&file)?;
    let header = all.first().cloned().unwrap_or(json!({}));
    let number = all.iter().filter(|l| l["type"] == "step" || l["type"] == "expect").count() + 1;
    let shot = file.with_extension("").join(format!("step-{number:02}.png"));
    screenshot(&py::str(&header["platform"]), &py::str(&header["device"]), &shot);
    let mut step = take_note().unwrap_or(json!({}));
    step["type"] = json!("step");
    step["n"] = json!(number);
    step["command"] = json!(argv[0]);
    step["argv"] = json!(argv);
    step["line"] = json!(output.trim());
    step["shot"] = json!(shot.display().to_string());
    append(&file, &step)
}

pub fn walk(args: &mut Args) -> Result<i32> {
    match args.action.as_deref().unwrap_or("show") {
        "start" => start(args),
        "end" => end(args),
        "expect" => expect(args),
        "replay" => replay(args),
        "parity" => parity(args),
        "list" => list(),
        _ => show(args),
    }
}

/// A name for the file from the task's first words.
fn slug(task: &str) -> String {
    let words: Vec<String> =
        task.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).take(6).map(str::to_lowercase).collect();
    let base = if words.is_empty() { "walk".to_string() } else { words.join("-") };
    let mut name = base.clone();
    let mut counter = 1;
    while walks_dir().join(format!("{name}.jsonl")).exists() {
        counter += 1;
        name = format!("{base}-{counter}");
    }
    name
}

/// Where the actions go: (mode, platform, device, app). `device` mode is an app without the library.
fn target(args: &Args) -> Result<(String, String, String, String)> {
    let session = read_json(&ctx().session_file).unwrap_or(json!({}));
    if args.device.is_some() || session["mode"] == "device" {
        let (platform, device, app) = crate::device::target(args)?;
        return Ok(("device".into(), platform, device, app.unwrap_or_default()));
    }
    if session["port"].is_null() {
        fail!(
            "no app to walk: no session and no --device",
            "not_connected",
            "ddp up --platform ios --install <build> --app <id>, or ddp connect <device> --app <id>"
        );
    }
    Ok(("sdk".into(), py::str(&session["platform"]), py::str(&session["device"]), py::str(&session["app"])))
}

fn start(args: &Args) -> Result<i32> {
    let task = args.value.clone().unwrap_or_default();
    if task.trim().is_empty() {
        fail!(
            "a walk needs its task in plain words",
            "missing_option",
            "ddp walk start \"add a note, then delete it\""
        );
    }
    if let Some(open) = open_walk() {
        fail!(
            format!("a walk is already open: {}", open.display()),
            "walk_open",
            "ddp walk end pass|fail, then start the next"
        );
    }
    let (mode, platform, device, app) = target(args)?;
    let name = slug(&task);
    let file = walks_dir().join(format!("{name}.jsonl"));
    let header = json!({
        "type": "walk", "task": task, "name": name, "mode": mode, "platform": platform, "device": device, "app": app,
        "started": chrono::Local::now().to_rfc3339(),
    });
    write_file(&file, format!("{}\n", py::dumps(&header)))?;
    write_file(&pointer(), py::dumps(&json!({"file": file.display().to_string()})))?;
    let checks = if mode == "device" { " (no SDK: no checks)" } else { "" };
    say!("walk {name} started on {platform} {device}{checks}; each tap, swipe, type and press is now a step; ddp walk expect <element> adds a check; end with ddp walk end pass|fail");
    Ok(0)
}

/// `ddp walk expect <element>`: the element is on screen now (waiting up to 3 s). The replay checks it again.
fn expect(args: &Args) -> Result<i32> {
    let Some(file) = open_walk() else {
        fail!("no walk is open", "no_walk", "ddp walk start \"<task>\"");
    };
    let wanted = crate::queries::resolve_ref(&args.value.clone().unwrap_or_default())?;
    let header = lines(&file)?.first().cloned().unwrap_or(json!({}));
    let found = wait_for(&header_target(&header, args), |elements| {
        elements
            .iter()
            .find(|e| element_key(e) == wanted || e["label"] == wanted.as_str() || e["id"] == wanted.as_str())
            .cloned()
    })?;
    let number = lines(&file)?.iter().filter(|l| l["type"] == "step" || l["type"] == "expect").count() + 1;
    let ok = found.is_some();
    append(&file, &json!({"type": "expect", "n": number, "given": wanted, "seen": found, "ok": ok}))?;
    if !ok {
        fail!(
            format!("{} is not on screen", py::dumps(&json!(wanted))),
            "not_found",
            "ddp tree, then walk end fail if the task failed"
        );
    }
    say!("expect {}: on screen", py::dumps(&json!(wanted)));
    Ok(0)
}

/// What a walk's steps go to now: the walk's own target, or `--device` when given.
fn header_target(header: &Value, args: &Args) -> (String, String, String) {
    match &args.device {
        Some(device) => ("device".into(), crate::live::platform_of(args.platform.as_deref(), device), device.clone()),
        None => (py::str(&header["mode"]), py::str(&header["platform"]), py::str(&header["device"])),
    }
}

/// Looks at the screen until `pick` finds something, at most `FIND_WAIT`.
fn wait_for<T>(target: &(String, String, String), pick: impl Fn(&[Value]) -> Option<T>) -> Result<Option<T>> {
    let started = Instant::now();
    loop {
        let elements = elements_now(target)?;
        if let Some(found) = pick(&elements) {
            return Ok(Some(found));
        }
        if started.elapsed() >= FIND_WAIT {
            return Ok(None);
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

/// The elements on screen now: the platform's accessibility tree without the library, the app's own frame with it.
fn elements_now((mode, platform, device): &(String, String, String)) -> Result<Vec<Value>> {
    if mode == "device" {
        return crate::runner::elements_now(platform, device);
    }
    let session = crate::live::require_session()?;
    Ok(crate::hit::frame_now(&session)?.elements().to_vec())
}

/// Shortens an action's line for the report: the device and the no-checks tail are said once at the top.
fn short(line: &str, device: &str) -> String {
    line.replace(&format!(" on {device}"), "").replace(&format!("; {}", crate::runner::NO_CHECKS), "")
}

/// One line per step, the result, where the file and screenshots are.
fn report(file: &Path) -> Result<()> {
    let all = lines(file)?;
    let header = all.first().cloned().unwrap_or(json!({}));
    let device = py::str(&header["device"]);
    let end = all.iter().rev().find(|l| l["type"] == "end");
    let steps: Vec<&Value> = all.iter().filter(|l| l["type"] == "step" || l["type"] == "expect").collect();
    let result = end.map(|e| py::str(&e["result"]).to_uppercase()).unwrap_or_else(|| "OPEN".into());
    let checks = if header["mode"] == "device" { ", no checks (no SDK)" } else { "" };
    say!(
        "walk {}: {result}, {} steps on {} {device}{checks}",
        py::str(&header["name"]),
        steps.len(),
        py::str(&header["platform"])
    );
    say!("task: {}", py::str(&header["task"]));
    for step in &steps {
        let line = if step["type"] == "expect" {
            format!(
                "expect {}: {}",
                py::dumps(&step["given"]),
                if step["ok"] == true { "on screen" } else { "NOT on screen" }
            )
        } else {
            short(&py::str(&step["line"]), &device)
        };
        say!("{}. {line}", py::str(&step["n"]));
    }
    if let Some(note) = end.and_then(|e| e["note"].as_str()) {
        say!("note: {note}");
    }
    say!("file: {}; screenshots: {}", file.display(), file.with_extension("").display());
    Ok(())
}

fn end(args: &Args) -> Result<i32> {
    let Some(file) = open_walk() else {
        fail!("no walk is open", "no_walk", "ddp walk list");
    };
    let result = args.value.clone().unwrap_or_default();
    if result != "pass" && result != "fail" {
        fail!("end a walk with pass or fail", "bad_value", "ddp walk end pass");
    }
    let mut end = json!({"type": "end", "result": result, "ended": chrono::Local::now().to_rfc3339()});
    if let Some(note) = &args.what {
        end["note"] = json!(note);
    }
    append(&file, &end)?;
    let _ = std::fs::remove_file(pointer());
    report(&file)?;
    if result == "pass" {
        say!(
            "next: ddp walk replay {} (no model, on any device)",
            file.file_stem().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(0)
}

/// A walk by name or path; the open walk, else the newest.
fn find_walk(name: Option<&str>) -> Result<PathBuf> {
    if let Some(name) = name {
        let path = PathBuf::from(name);
        if path.exists() {
            return Ok(path);
        }
        let named = walks_dir().join(format!("{name}.jsonl"));
        if named.exists() {
            return Ok(named);
        }
        fail!(format!("no walk {name}"), "unknown_walk", "ddp walk list");
    }
    if let Some(open) = open_walk() {
        return Ok(open);
    }
    newest().ok_or_else(|| failure("no walks yet", "unknown_walk", Some("ddp walk start \"<task>\"")))
}

fn walk_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(walks_dir())
        .map(|entries| {
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")).collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    files
}

fn newest() -> Option<PathBuf> {
    walk_files().pop()
}

fn show(args: &Args) -> Result<i32> {
    report(&find_walk(args.value.as_deref())?)?;
    Ok(0)
}

fn list() -> Result<i32> {
    let files = walk_files();
    if files.is_empty() {
        say!("no walks yet; ddp walk start \"<task>\"");
    }
    for file in files {
        let all = lines(&file)?;
        let header = all.first().cloned().unwrap_or(json!({}));
        let result = all
            .iter()
            .rev()
            .find(|l| l["type"] == "end")
            .map(|e| py::str(&e["result"]))
            .unwrap_or_else(|| "open".into());
        let steps = all.iter().filter(|l| l["type"] == "step" || l["type"] == "expect").count();
        say!(
            "{} {result} {steps} steps {}: {}",
            py::str(&header["name"]),
            py::str(&header["platform"]),
            py::str(&header["task"])
        );
    }
    Ok(0)
}

/// The element a walked place points at now: the same id, else the same label; on the same platform the one nearest
/// to where it was and not much bigger or smaller. A place that was only a point, with nothing seen there, is found
/// again only on the same platform and screen. None when nothing fits: the replay says the target is gone.
fn find_again(place: &Value, elements: &[Value], same_platform: bool) -> Option<(f64, f64)> {
    let seen = &place["seen"];
    let given = py::str(&place["given"]);
    let text = |field: &str| seen[field].as_str().filter(|t| !t.is_empty());
    let mut candidates: Vec<&Value> = match (text("id"), text("label")) {
        (Some(id), _) if elements.iter().any(|e| e["id"] == id) => elements.iter().filter(|e| e["id"] == id).collect(),
        (_, Some(label)) => elements.iter().filter(|e| e["label"] == label).collect(),
        _ if seen.is_null() && crate::hit::point(&given).is_none() => elements
            .iter()
            .filter(|e| e["label"] == given.as_str() || e["id"] == given.as_str() || element_key(e) == given)
            .collect(),
        _ => Vec::new(),
    };
    let at = place["at"].as_array().map(|a| (crate::py::f(&a[0]), crate::py::f(&a[1])));
    if seen.is_null() {
        // A point with no element under it: the same pixels mean the same place only on the same screen.
        return if same_platform { at } else { None };
    }
    let [sx, sy, sw, sh] = crate::model::rect(&seen["frame"]);
    if same_platform && sw * sh > 0.0 {
        candidates.retain(|e| {
            let [_, _, w, h] = crate::model::rect(&e["frame"]);
            let ratio = (w * h) / (sw * sh);
            (1.0 / SIZE_RATIO..=SIZE_RATIO).contains(&ratio)
        });
        let old = (sx + sw / 2.0, sy + sh / 2.0);
        candidates.sort_by(|a, b| distance(a, old).total_cmp(&distance(b, old)));
    }
    let element = candidates.first()?;
    // Where in the element the walk touched: the same share of the element now.
    let [x, y, w, h] = crate::model::rect(&element["frame"]);
    let (share_x, share_y) = match at {
        Some((ax, ay)) if sw > 0.0 && sh > 0.0 => (((ax - sx) / sw).clamp(0.0, 1.0), ((ay - sy) / sh).clamp(0.0, 1.0)),
        _ => (0.5, 0.5),
    };
    Some((x + w * share_x, y + h * share_y))
}

fn distance(element: &Value, (x, y): (f64, f64)) -> f64 {
    let [ex, ey, w, h] = crate::model::rect(&element["frame"]);
    ((ex + w / 2.0 - x).powi(2) + (ey + h / 2.0 - y).powi(2)).sqrt()
}

/// What a step says about a place for a person: its label, its id, or its point.
fn described(place: &Value) -> String {
    let seen = &place["seen"];
    match (seen["label"].as_str(), seen["id"].as_str()) {
        (Some(label), _) => py::dumps(&json!(label)),
        (None, Some(id)) => format!("#{id}"),
        _ => py::str(&place["given"]),
    }
}

/// Runs one ddp command inside this one, its output kept for the replay's own line, with the failure it reported.
fn run_nested(argv: Vec<String>) -> (i32, String, Option<Value>, Option<crate::state::Failure>) {
    let outer = ctx();
    let mut argv = argv;
    if let Some(session) = &outer.session_name {
        argv.extend(["--session".to_string(), session.clone()]);
    }
    crate::state::take_last_failure();
    crate::state::push_capture(true);
    let code = crate::run_command(argv);
    let output = crate::state::pop_capture();
    crate::state::use_context(outer.session_name.as_deref(), outer.json);
    (code, output, take_note(), crate::state::take_last_failure())
}

/// `ddp walk replay [walk] [--device D]`: every step again, with no model. One line when it passes; on a failure,
/// the step, what was expected and what the screen has instead.
fn replay(args: &Args) -> Result<i32> {
    let file = find_walk(args.value.as_deref())?;
    let all = lines(&file)?;
    let header = all.first().cloned().unwrap_or(json!({}));
    let name = py::str(&header["name"]);
    let session = read_json(&ctx().session_file).unwrap_or(json!({}));
    let target = match &args.device {
        Some(_) => header_target(&header, args),
        None if session["mode"] == "device" => {
            ("device".into(), py::str(&session["platform"]), py::str(&session["device"]))
        }
        None if !session["port"].is_null() => {
            ("sdk".into(), py::str(&session["platform"]), py::str(&session["device"]))
        }
        None => header_target(&header, args),
    };
    let (mode, platform, device) = target.clone();
    let same_platform = platform == py::str(&header["platform"]);
    let steps: Vec<&Value> = all.iter().filter(|l| l["type"] == "step" || l["type"] == "expect").collect();
    let started = Instant::now();
    let mut results: Vec<Value> = Vec::new();
    let mut failed: Option<String> = None;
    struct Replaying;
    impl Drop for Replaying {
        fn drop(&mut self) {
            REPLAYING.with(|r| r.set(false));
        }
    }
    REPLAYING.with(|r| r.set(true));
    let replaying = Replaying;
    for step in &steps {
        let number = py::str(&step["n"]);
        if step["type"] == "expect" {
            if step["ok"] != true {
                continue;
            }
            let place = json!({"given": step["given"], "seen": step["seen"]});
            let found = wait_for(&target, |elements| find_again(&place, elements, false))?;
            results.push(json!({"n": step["n"], "expect": described(&place), "ok": found.is_some()}));
            if found.is_none() {
                failed = Some(format!(
                    "step {number}, expect {}: not on screen after {}s",
                    described(&place),
                    FIND_WAIT.as_secs()
                ));
                break;
            }
            continue;
        }
        let command = py::str(&step["command"]);
        let places: Vec<Value> = step["places"].as_array().cloned().unwrap_or_default();
        let mut points = Vec::new();
        for place in &places {
            match wait_for(&target, |elements| find_again(place, elements, same_platform))? {
                Some((x, y)) => points.push(format!("{x:.1},{y:.1}")),
                None => {
                    failed = Some(format!(
                        "step {number}, {command} {}: not on screen after {}s",
                        described(place),
                        FIND_WAIT.as_secs()
                    ));
                    break;
                }
            }
        }
        if failed.is_some() {
            results.push(json!({"n": step["n"], "command": command, "ok": false}));
            break;
        }
        let mut argv = vec![command.clone()];
        match &command[..] {
            "type" => {
                argv.push(py::str(&step["text"]));
                if let Some(field) = points.first() {
                    argv.extend(["--into".into(), field.clone()]);
                }
            }
            "press" => argv.push(py::str(&step["key"])),
            _ => argv.extend(points.iter().cloned()),
        }
        if let Some(ms) = step["ms_given"].as_f64() {
            argv.extend(["--for".into(), format!("{ms}ms")]);
        }
        if mode == "device" {
            argv.extend(["--device".into(), device.clone()]);
        }
        let (code, output, note, failure) = run_nested(argv);
        let appeared: Vec<Value> = note.as_ref().and_then(|n| n["appeared"].as_array().cloned()).unwrap_or_default();
        results.push(
            json!({"n": step["n"], "command": command, "ok": code == 0, "line": output.trim(), "appeared": appeared}),
        );
        if code != 0 {
            let action = py::str(&step["line"]).split(';').next().unwrap_or("").to_string();
            // What the app shows decides a replay; ddp failing to reach the device is not the app's failure.
            let why = match &failure {
                Some(f) if f.code == "unknown_element" || f.code == "not_found" => format!("{action}: {}", f.message),
                Some(f) => format!("{action}: could not run it ({}: {})", f.code, f.message),
                None => format!("{action}: {}", output.trim().replace('\n', " ")),
            };
            failed = Some(format!("step {number}, {why}"));
            break;
        }
    }
    drop(replaying);
    let seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    let result = json!({"walk": name, "platform": platform, "device": device, "ok": failed.is_none(), "steps": results, "failed": failed});
    write_file(&file.with_extension(format!("{platform}.replay.json")), py::dumps(&result))?;
    match failed {
        None => {
            say!("replay {name} on {platform} {device}: {} steps passed in {seconds}s", steps.len());
            Ok(0)
        }
        Some(why) => {
            say!("replay {name} on {platform} {device}: FAIL at {why}");
            let shown: Vec<String> = elements_now(&target)
                .unwrap_or_default()
                .iter()
                .filter_map(|e| e["label"].as_str().map(|l| py::dumps(&json!(l))))
                .take(8)
                .collect();
            say!("on screen: {}", if shown.is_empty() { "no labeled elements".to_string() } else { shown.join(", ") });
            Ok(1)
        }
    }
}

/// `ddp walk parity [walk]`: the last replay on iOS against the last on Android, step by step: which passed, and
/// which labels appeared on screen after each step on one and not on the other. Labels, not elements: one platform
/// keys a button by its id and the other by its label, and a renamed button is the same change on both.
fn parity(args: &Args) -> Result<i32> {
    let file = find_walk(args.value.as_deref())?;
    let name = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let read = |platform: &str| read_json(&file.with_extension(format!("{platform}.replay.json")));
    let (Some(ios), Some(android)) = (read("ios"), read("android")) else {
        fail!(
            format!("parity needs a replay of {name} on both platforms"),
            "missing_replay",
            format!("ddp walk replay {name} --device <the other platform's device>")
        );
    };
    let steps = |result: &Value| result["steps"].as_array().cloned().unwrap_or_default();
    let (ios_steps, android_steps) = (steps(&ios), steps(&android));
    let mut differences = 0;
    for ios_step in &ios_steps {
        let Some(android_step) = android_steps.iter().find(|s| s["n"] == ios_step["n"]) else { continue };
        let labels =
            |step: &Value| -> Vec<String> { step["appeared"].as_array().into_iter().flatten().map(py::str).collect() };
        let (on_ios, on_android) = (labels(ios_step), labels(android_step));
        let only = |a: &[String], b: &[String]| a.iter().filter(|l| !b.contains(l)).cloned().collect::<Vec<_>>();
        let (ios_only, android_only) = (only(&on_ios, &on_android), only(&on_android, &on_ios));
        if ios_step["ok"] != android_step["ok"] || !ios_only.is_empty() || !android_only.is_empty() {
            differences += 1;
            say!(
                "step {}: ios {}, android {}{}{}",
                py::str(&ios_step["n"]),
                if ios_step["ok"] == true { "passed" } else { "FAILED" },
                if android_step["ok"] == true { "passed" } else { "FAILED" },
                if ios_only.is_empty() { String::new() } else { format!("; new on iOS only: {}", ios_only.join(", ")) },
                if android_only.is_empty() {
                    String::new()
                } else {
                    format!("; new on Android only: {}", android_only.join(", "))
                },
            );
        }
    }
    let state = |result: &Value| {
        if result["ok"] == true {
            "passed".to_string()
        } else {
            format!("FAILED ({})", py::str(&result["failed"]))
        }
    };
    let differ = if differences == 1 { "1 step differs".to_string() } else { format!("{differences} steps differ") };
    say!("parity {name}: ios {}, android {}; {differ}", state(&ios), state(&android));
    Ok(if differences == 0 && ios["ok"] == true && android["ok"] == true { 0 } else { 1 })
}
