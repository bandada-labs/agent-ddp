//! For people and other tools: the HTML view, the MCP server, the guides, doctor, batch.
use crate::args::Args;
use crate::live::{android_socket, library_too_old, older_library, platform_of, snapshot_messages};
use crate::model::{element_alpha, element_key, element_keys, element_state, load};
use crate::py;
use crate::state::{self, adb_path, capture, ctx, read_json, which, write_file, Result, IOS_DIR};
use crate::summary::describe_track;
use crate::{fail, say};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

const VIEW_PAGE: &str = include_str!("view.html");

/// Writes a self-contained HTML page: the take frame by frame, and each element's changes. No server.
pub fn view(args: &mut Args) -> Result<i32> {
    let recording = load(args.file.as_deref().unwrap_or(&ctx().last))?;
    let frames = &recording.frames;
    let times: Vec<Value> = frames.iter().map(|f| f.t_value().clone()).collect();
    let keys = element_keys(frames);
    let boxes: Vec<Value> = frames
        .iter()
        .map(|frame| {
            let rows: Vec<Value> = frame
                .elements()
                .iter()
                .map(|element| {
                    let key = element_key(element);
                    let state = element_state(frame, &key).unwrap();
                    let fill = state["fill"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| Value::from(format!("#{}", &s[1..7.min(s.len())])))
                        .unwrap_or(state["fill"].clone());
                    let mut row = vec![Value::from(key.clone())];
                    row.extend(element["frame"].as_array().cloned().unwrap_or_default());
                    row.extend([fill, state["border"].clone(), py::round(&element_alpha(frame, &key), 2)]);
                    Value::Array(row)
                })
                .collect();
            json!({"t": frame.t_value(), "boxes": rows})
        })
        .collect();
    let edges: Vec<[f64; 4]> =
        frames.iter().flat_map(|f| f.elements().iter().map(|e| crate::model::rect(&e["frame"]))).collect();
    let size = if edges.is_empty() {
        json!([400, 800])
    } else {
        let width = edges.iter().map(|r| r[0] + r[2]).fold(f64::NEG_INFINITY, f64::max);
        let height = edges.iter().map(|r| r[1] + r[3]).fold(f64::NEG_INFINITY, f64::max);
        json!([py::fnum(width), py::fnum(height)])
    };
    let tracks: Vec<Value> = keys
        .iter()
        .map(|key| {
            let states: Vec<Option<Value>> = frames.iter().map(|f| element_state(f, key)).collect();
            let lines: Vec<String> = ["frame", "fill", "border", "opacity", "draws", "label"]
                .iter()
                .filter_map(|name| describe_track(name, &states, &times))
                .map(|l| l.trim().to_string())
                .collect();
            json!([key, lines])
        })
        .collect();
    let data = json!({"platform": recording.header["platform"], "unit": recording.header["unit"], "size": size,
                      "frames": boxes, "tracks": tracks});
    // "</" inside the JSON would end the script tag early.
    let page = VIEW_PAGE.replace("__DATA__", &py::dumps(&data).replace("</", "<\\/"));
    write_file(Path::new(&args.out), page)?;
    say!("{}: {} frames, {} elements. Open it in a browser", args.out, frames.len(), keys.len());
    Ok(0)
}

const MCP_PROFILES: [(&str, &[&str]); 3] = [
    ("core", &["doctor", "connect", "tree", "find", "get", "read", "check", "wait", "tap", "type", "press", "skills"]),
    ("live", &["watch", "eval", "time", "net", "record", "logs", "annotate", "history", "prefs", "run"]),
    ("checks", &["check", "baseline", "diff"]),
];

/// A Model Context Protocol server on stdin/stdout: each ddp command is a tool taking the command's
/// arguments as one string. --tools picks small sets so a client's context stays small.
pub fn mcp(args: &mut Args) -> Result<i32> {
    let mut wanted: Vec<&str> = Vec::new();
    for profile in args.tools.split(',') {
        let names: Vec<&str> = if profile == "all" {
            let mut all: Vec<&str> = MCP_PROFILES.iter().flat_map(|(_, names)| names.iter().copied()).collect();
            all.sort();
            all.dedup();
            all
        } else {
            match MCP_PROFILES.iter().find(|(name, _)| *name == profile) {
                Some((_, names)) => names.to_vec(),
                None => fail!(format!("no tool profile '{profile}'"), "bad_profile", "use core, live, checks or all"),
            }
        };
        for name in names {
            if !wanted.contains(&name) {
                wanted.push(name);
            }
        }
    }
    let tools: Vec<Value> = wanted.iter().map(|name| json!({
        "name": format!("ddp_{name}"),
        "description": format!("ddp {name}: see `ddp skills get core`. Pass the arguments as you would after `ddp {name}` on a command line."),
        "inputSchema": {"type": "object", "properties": {"args": {"type": "string"}}},
    })).collect();
    let reply = |id: &Value, result: Option<Value>, error: Option<Value>| {
        let mut body = json!({"jsonrpc": "2.0", "id": id});
        match error {
            Some(error) => body["error"] = error,
            None => body["result"] = result.unwrap_or(Value::Null),
        }
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{}", py::dumps(&body));
        let _ = stdout.flush();
    };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let method = message["method"].as_str().unwrap_or("");
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => reply(
                &id,
                Some(json!({
                    "protocolVersion": message["params"].get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                    "capabilities": {"tools": {}}, "serverInfo": {"name": "ddp", "version": env!("CARGO_PKG_VERSION")},
                })),
                None,
            ),
            "tools/list" => reply(&id, Some(json!({"tools": tools})), None),
            "tools/call" => {
                let full = py::str(&message["params"]["name"]);
                let name = full.strip_prefix("ddp_").unwrap_or(&full).to_string();
                if !wanted.contains(&name.as_str()) {
                    reply(&id, None, Some(json!({"code": -32602, "message": format!("unknown tool {name}")})));
                    continue;
                }
                let line = message["params"]["arguments"]["args"].as_str().unwrap_or("").to_string();
                state::push_capture(true);
                let code = match state::shell_split(&line) {
                    Ok(words) => crate::run_command([vec![name.clone()], words].concat()),
                    Err(error) => {
                        state::report_failure(&error);
                        1
                    }
                };
                let captured = state::pop_capture();
                let text =
                    if captured.trim().is_empty() { "(no output)".to_string() } else { captured.trim().to_string() };
                reply(&id, Some(json!({"content": [{"type": "text", "text": text}], "isError": code != 0})), None);
            }
            _ if !id.is_null() => {
                reply(&id, None, Some(json!({"code": -32601, "message": format!("no method {method}")})))
            }
            _ => {}
        }
    }
    Ok(0)
}

/// The guides, built in so they always match this version.
const SKILLS: [(&str, &str); 3] = [
    ("core", include_str!("../../docs/skills/core.md")),
    ("explore", include_str!("../../docs/skills/explore.md")),
    ("reviewer", include_str!("../../docs/skills/reviewer.md")),
];
const REFERENCE: &str = include_str!("../../docs/agents.md");

/// The `key: value` lines between the leading `---` markers, and the text after them.
fn frontmatter(text: &str) -> (Vec<(String, String)>, String) {
    let Some(rest) = text.strip_prefix("---\n") else { return (Vec::new(), text.to_string()) };
    let Some((head, body)) = rest.split_once("\n---\n") else { return (Vec::new(), String::new()) };
    let fields = head.lines().filter_map(|l| l.split_once(": ")).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    (fields, body.trim_start_matches('\n').to_string())
}

/// The `##` / `###` sections whose heading holds `wanted`, any case, each up to the next heading of its level.
fn sections(text: &str, wanted: &str) -> String {
    let (mut kept, mut keep_level, mut in_code) = (Vec::new(), None::<usize>, false);
    for line in text.lines() {
        in_code ^= line.starts_with("```");
        let level =
            if line.starts_with("##") && !in_code { line.len() - line.trim_start_matches('#').len() } else { 0 };
        if level > 0 && keep_level.is_some_and(|k| level <= k) {
            keep_level = None;
        }
        if level > 0 && keep_level.is_none() && line.to_lowercase().contains(&wanted.to_lowercase()) {
            keep_level = Some(level);
        }
        if keep_level.is_some() {
            kept.push(line);
        }
    }
    kept.join("\n")
}

pub fn skills(args: &mut Args) -> Result<i32> {
    let action = args.action.clone().unwrap_or_else(|| "list".into());
    if action == "list" {
        for (name, text) in SKILLS {
            let description =
                frontmatter(text).0.into_iter().find(|(k, _)| k == "description").map(|(_, v)| v).unwrap_or_default();
            say!("{name:10} {description}");
        }
        return Ok(0);
    }
    let name = args.name.clone().unwrap_or_else(|| "core".into());
    let Some((_, text)) = SKILLS.iter().find(|(n, _)| *n == name) else {
        fail!(format!("no skill '{name}'"), "unknown_skill", "ddp skills list");
    };
    if action == "path" {
        // The guides live inside the binary: write this one out where a tool can open it.
        let path = ctx().dir.join("skills").join(format!("{name}.md"));
        write_file(&path, text)?;
        say!("{}", path.display());
        return Ok(0);
    }
    let (_, mut text) = frontmatter(text);
    if args.full {
        text = format!("{text}\n\n{REFERENCE}");
    }
    if let Some(filter) = &args.filter {
        let found = sections(&text, filter);
        text = if found.is_empty() { format!("no section matching '{filter}'") } else { found };
    }
    say!("{}", text.trim_end());
    Ok(0)
}

type Step = (bool, String, String);

/// (ok, what, fix) for each step from the device to a frame with elements. Stops at the first failure.
fn doctor_checks(platform: &str, device: &str, app: &str) -> Result<Vec<Step>> {
    let mut steps: Vec<Step> = Vec::new();
    let session = if platform == "ios" {
        if which("xcrun").is_none() {
            steps.push((false, "xcrun".into(), "install Xcode and its command line tools".into()));
            return Ok(steps);
        }
        let listing: Value =
            serde_json::from_str(&state::run(&["xcrun", "simctl", "list", "devices", "-j"])?).unwrap_or(json!({}));
        let state = listing["devices"]
            .as_object()
            .into_iter()
            .flat_map(|groups| groups.values())
            .flat_map(|group| group.as_array().cloned().unwrap_or_default())
            .find(|d| d["udid"] == device)
            .map(|d| py::str(&d["state"]));
        let booted = state.as_deref() == Some("Booted");
        steps.push((
            booted,
            format!("simulator {device} {}", state.unwrap_or_else(|| "not found".into())),
            format!("xcrun simctl boot {device}"),
        ));
        if !booted {
            return Ok(steps);
        }
        let result = capture("xcrun", &["simctl", "get_app_container", device, app, "data"]);
        steps.push((result.code == 0, format!("app {app} installed"), "build and install the Debug build".into()));
        if result.code != 0 {
            return Ok(steps);
        }
        let endpoint = read_json(&Path::new(result.stdout.trim()).join(IOS_DIR).join("control.json"));
        steps.push((
            endpoint.is_some(),
            "control endpoint written by AgentDDP.start()".into(),
            "launch the app (xcrun simctl launch); it must be a Debug build that calls AgentDDP.start() at launch"
                .into(),
        ));
        let Some(endpoint) = endpoint else {
            if older_library("ios", device, app) {
                steps.push((false, "library 0.2.0 or newer".into(), library_too_old().next.unwrap_or_default()));
            }
            return Ok(steps);
        };
        json!({"platform": "ios", "app": app, "device": device, "port": endpoint["port"]})
    } else {
        let adb = adb_path()?;
        let state = capture(&adb, &["-s", device, "get-state"]).stdout.trim().to_string();
        steps.push((
            state == "device",
            format!("emulator or device {device} {}", if state.is_empty() { "not found" } else { &state }),
            "boot it, then adb devices".into(),
        ));
        if state != "device" {
            return Ok(steps);
        }
        let installed = capture(&adb, &["-s", device, "shell", "pm", "path", app]);
        let is_installed = installed.stdout.starts_with("package:");
        steps.push((
            installed.code == 0 && is_installed,
            format!("app {app} installed"),
            "install the debug APK (./gradlew installDebug)".into(),
        ));
        if !is_installed {
            return Ok(steps);
        }
        let debuggable = capture(&adb, &["-s", device, "shell", "run-as", app, "true"]);
        steps.push((debuggable.code == 0, "debuggable build".into(), "install the debug variant, not release".into()));
        let package = capture(&adb, &["-s", device, "shell", "dumpsys", "package", app]).stdout;
        steps.push((
            package.contains("dev.agentddp"),
            "agent-ddp library in the build".into(),
            "add debugImplementation(project(\":agent-ddp\")) and reinstall".into(),
        ));
        let running = capture(&adb, &["-s", device, "shell", "pidof", app]).stdout.trim().to_string();
        steps.push((!running.is_empty(), "app running".into(), format!("adb -s {device} shell monkey -p {app} 1")));
        if running.is_empty() {
            return Ok(steps);
        }
        let forwarded = capture(&adb, &["-s", device, "forward", "tcp:0", &android_socket(app)]);
        json!({"platform": "android", "app": app, "device": device, "port": forwarded.stdout.trim().parse::<u64>().unwrap_or(0)})
    };
    let answer = snapshot_messages(&session);
    if platform == "android" && session["port"].as_u64().unwrap_or(0) > 0 {
        capture(&adb_path()?, &["-s", device, "forward", "--remove", &format!("tcp:{}", py::str(&session["port"]))]);
    }
    let messages = match answer {
        Ok(messages) => messages,
        Err(_) if older_library(platform, device, app) => {
            steps.push((false, "library 0.2.0 or newer".into(), library_too_old().next.unwrap_or_default()));
            return Ok(steps);
        }
        Err(failure) => {
            steps.push((
                false,
                format!("control endpoint answers ({})", failure.message),
                "restart the app; then ddp connect".into(),
            ));
            return Ok(steps);
        }
    };
    steps.push((true, "control endpoint answers".into(), String::new()));
    let count = messages
        .iter()
        .find(|m| m["type"] == "frame")
        .and_then(|m| m["elements"].as_array().map(Vec::len))
        .unwrap_or(0);
    steps.push((
        count > 0,
        format!("{count} elements on screen"),
        if platform == "ios" {
            "iOS: the accessibility tree is empty, restart the app; Android: give what you need a testTag".into()
        } else {
            "give the elements you need a testTag, text or contentDescription".into()
        },
    ));
    Ok(steps)
}

pub fn doctor(args: &mut Args) -> Result<i32> {
    let context = ctx();
    let session = read_json(&context.session_file).unwrap_or(json!({}));
    let device = args
        .device
        .clone()
        .or_else(|| session["device"].as_str().map(String::from))
        .or_else(|| std::env::var("DDP_DEVICE").ok());
    let app = args.app.clone().or_else(|| session["app"].as_str().map(String::from));
    let (Some(device), Some(app)) = (device, app) else {
        fail!("doctor needs a device and an app", "missing_device", "ddp doctor DEVICE --app ID");
    };
    let platform = platform_of(args.platform.as_deref(), &device);
    let mut results = doctor_checks(&platform, &device, &app)?;
    if context.last.exists() {
        if let Ok(last) = load(&context.last) {
            if last.frames.last().is_some_and(|f| f.elements().is_empty()) {
                results.push((
                    false,
                    format!("last recording {} has no elements", context.last.display()),
                    "fix the steps above, then record again".into(),
                ));
            }
        }
    }
    if context.json {
        let rows: Vec<Value> = results.iter().map(|(ok, what, fix)| {
            json!({"ok": ok, "check": what, "fix": if *ok { Value::Null } else { Value::from(fix.clone()) }})
        }).collect();
        say!("{}", py::dumps(&json!(rows)));
    } else {
        for (ok, what, fix) in &results {
            say!(
                "{} {what}{}",
                if *ok { "ok  " } else { "FAIL" },
                if *ok { String::new() } else { format!("\n     fix: {fix}") }
            );
        }
    }
    Ok(if results.iter().all(|(ok, _, _)| *ok) { 0 } else { 1 })
}

pub fn batch(args: &mut Args) -> Result<i32> {
    let lines: Vec<String> = if args.lines.is_empty() {
        std::io::stdin().lock().lines().map_while(std::result::Result::ok).filter(|l| !l.trim().is_empty()).collect()
    } else {
        args.lines.clone()
    };
    let mut worst = 0;
    for line in lines {
        say!("$ ddp {line}");
        let code = match state::shell_split(&line) {
            Ok(words) => crate::run_command(words),
            Err(error) => {
                state::report_failure(&error);
                1
            }
        };
        worst = worst.max(code);
        if code != 0 && args.bail {
            break;
        }
    }
    Ok(worst)
}
