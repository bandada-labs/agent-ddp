//! `ddp history`: what the app showed in the last minutes, from the flight recorder inside the Debug build.
//! It keeps every frame that changed plus taps and screen changes, so a still screen costs nothing.
use crate::args::Args;
use crate::device::target;
use crate::live::{live_session, open};
use crate::model::{self, contains, decode, element_key, element_state, option_equal, rect, save_recording, Frame};
use crate::py::{self, f};
use crate::state::{adb_path, ctx, local_time, now_ms, parse_ms, Result, ANDROID_DIR, IOS_DIR};
use crate::{fail, say};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::process::Command;

const SEGMENT_MS: f64 = 60_000.0; // the recorder starts a new file every minute
const BURST_GAP_MS: f64 = 500.0; // frames closer than this belong to one change
const NAMES_SHOWN: usize = 4;

/// One file of history: its header and its frame and event lines, each with `at` (epoch ms).
struct Segment {
    header: Value,
    lines: Vec<Value>,
}

pub fn history(args: &mut Args) -> Result<i32> {
    let since = now_ms() - parse_ms(&args.since)?;
    let segments = read_segments(args, since)?;
    if args.action.as_deref() == Some("at") {
        take(args, &segments)?;
    } else {
        timeline(args, &segments, since)?;
    }
    Ok(0)
}

fn start_of(name: &str) -> Option<f64> {
    name.split('.').next().and_then(|s| s.parse::<u64>().ok()).map(|s| s as f64)
}

/// The files that may hold anything after `since`, oldest first.
fn read_segments(args: &Args, since: f64) -> Result<Vec<Segment>> {
    let files = match &args.dir {
        Some(dir) => local_files(Path::new(dir)),
        None => app_files(args, since)?,
    };
    let mut names: Vec<&String> =
        files.keys().filter(|n| start_of(n).is_some_and(|s| s >= since - SEGMENT_MS)).collect();
    names.sort_by(|a, b| start_of(a).partial_cmp(&start_of(b)).unwrap());
    let mut segments = Vec::new();
    for name in names {
        // The last line of a file the app was writing when it died may be cut: skip what does not parse.
        let lines: Vec<Value> = decode(&files[name]).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let Some(header) = lines.iter().find(|l| l["type"] == "header").cloned() else { continue };
        model::check_protocol(&header)?;
        let started = f(&header["startedAt"]);
        let mut kept: Vec<Value> = lines
            .into_iter()
            .filter(|l| l["type"] == "frame" || l["type"] == "event")
            .map(|mut line| {
                let at = started + line.get("t").map(f).unwrap_or(0.0);
                line["at"] = py::fnum(at);
                line
            })
            .collect();
        // A touch is written when it ends but stamped when it went down, so its line can come after later frames.
        kept.sort_by(|a, b| f(&a["at"]).total_cmp(&f(&b["at"])));
        segments.push(Segment { header, lines: kept });
    }
    if segments.is_empty() {
        fail!(
            "no history in that window",
            "no_history",
            "the library keeps history since the app started; use the app, or a longer --since"
        );
    }
    Ok(segments)
}

fn local_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).ok()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// The app's history files: straight from the simulator's container on iOS, through `run-as` on Android.
fn app_files(args: &Args, since: f64) -> Result<BTreeMap<String, Vec<u8>>> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else {
        fail!("no app", "missing_option", "ddp connect DEVICE --app ID first, or pass --app");
    };
    if platform == "ios" {
        let dir = Path::new(&model::app_container(&device, &app)?).join(IOS_DIR).join("history");
        if !dir.exists() {
            fail!(
                "no history in the app",
                "no_history",
                format!("update the library in the app; ddp doctor {device} --app {app}")
            );
        }
        return Ok(local_files(&dir));
    }
    android_files(&device, &app, since)
}

fn android_files(device: &str, app: &str, since: f64) -> Result<BTreeMap<String, Vec<u8>>> {
    let adb = adb_path()?;
    let run_as =
        |command: &[&str]| Command::new(&adb).args(["-s", device, "exec-out", "run-as", app]).args(command).output();
    let listing = run_as(&["ls", &format!("{ANDROID_DIR}/history")])
        .map_err(|e| crate::state::failure(e.to_string(), "command_failed", None))?;
    if !listing.status.success() {
        let detail = String::from_utf8_lossy(if listing.stderr.is_empty() { &listing.stdout } else { &listing.stderr })
            .trim()
            .to_string();
        fail!(
            format!("no history on the device: {detail}"),
            "no_history",
            format!("update the library in the app; ddp doctor {device} --app {app}")
        );
    }
    let mut files = BTreeMap::new();
    for name in String::from_utf8_lossy(&listing.stdout).split_whitespace() {
        if start_of(name).is_some_and(|s| s >= since - SEGMENT_MS) {
            if let Ok(output) = run_as(&["cat", &format!("{ANDROID_DIR}/history/{name}")]) {
                files.insert(name.to_string(), output.stdout);
            }
        }
    }
    Ok(files)
}

fn clock(at: f64) -> String {
    format!("{}.{}", local_time("%H:%M:%S", at / 1000.0), (at.rem_euclid(1000.0) as i64) / 100)
}

fn named(keys: &HashSet<String>) -> String {
    let mut sorted: Vec<&String> = keys.iter().collect();
    sorted.sort();
    let shown: Vec<&str> = sorted.iter().take(NAMES_SHOWN).map(|s| s.as_str()).collect();
    let more = if sorted.len() > NAMES_SHOWN { format!(" +{}", sorted.len() - NAMES_SHOWN) } else { String::new() };
    format!("{}{more}", shown.join(", "))
}

/// The smallest element holding the point: what a tap there hit.
fn element_under(frame: &Frame, point: (f64, f64)) -> Option<String> {
    py::min_first(
        frame.elements().iter().filter(|e| contains(rect(&e["frame"]), [point.0, point.1, 0.0, 0.0], 0.0)),
        |e| {
            let r = rect(&e["frame"]);
            r[2] * r[3]
        },
    )
    .map(element_key)
}

/// One line per change (elements that appeared, went or changed), per tap and screen, per request.
/// Elements that change in most frames (a clock, a spinner) are left out, or every change would merge into one.
fn timeline(args: &Args, segments: &[Segment], since: f64) -> Result<()> {
    let mut entries: Vec<(f64, String)> = Vec::new();
    let mut frames: Vec<(f64, Frame)> = Vec::new();
    for segment in segments {
        let tolerance = 4.0 * segment.header.get("scale").map(f).unwrap_or(1.0);
        for line in &segment.lines {
            let at = f(&line["at"]);
            if line["type"] == "event" {
                if at >= since {
                    entries.push((at, event_text(line, frames.last().map(|(_, f)| f))));
                }
                continue;
            }
            let frame = Frame::from(line.clone(), tolerance);
            if at >= since || frames.is_empty() {
                frames.push((at, frame));
            } else {
                frames[0] = (at, frame); // the screen as it was when the window starts
            }
        }
    }
    let changes: Vec<HashSet<String>> = frames
        .windows(2)
        .map(|pair| {
            let (before, frame) = (&pair[0].1, &pair[1].1);
            frame
                .by_key
                .keys()
                .filter(|key| {
                    before.by_key.contains_key(*key)
                        && !option_equal(&element_state(frame, key), &element_state(before, key))
                })
                .cloned()
                .collect()
        })
        .collect();
    let mut counts: HashMap<&String, usize> = HashMap::new();
    for changed in &changes {
        for key in changed {
            *counts.entry(key).or_default() += 1;
        }
    }
    let mut noisy: HashSet<String> = counts
        .into_iter()
        .filter(|(_, count)| *count as f64 > changes.len() as f64 / 2.0 && changes.len() >= 20)
        .map(|(k, _)| k.clone())
        .collect();
    noisy.extend(args.ignore.iter().cloned());
    entries.extend(bursts(&frames, &changes, &noisy));
    entries.extend(request_entries(args, since));
    entries.retain(|(_, text)| !text.is_empty());
    entries.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let taps = entries.iter().filter(|(_, text)| text.starts_with("tap")).count();
    let first = frames.first().map(|(at, _)| *at).unwrap_or(since);
    say!(
        "history {} to {}: {} frames kept, {taps} taps{}",
        clock(since.max(first)),
        clock(now_ms()),
        frames.len(),
        if noisy.is_empty() { String::new() } else { format!("; always changing, left out: {}", named(&noisy)) }
    );
    let limit = args.limit.unwrap_or(60);
    let hidden = entries.len().saturating_sub(limit);
    if hidden > 0 {
        say!("... {hidden} earlier lines (a shorter --since, or --limit)");
    }
    for (at, text) in &entries[hidden..] {
        say!("{} {text}", clock(*at));
    }
    if !entries.is_empty() {
        say!("next: ddp history at <time> --for 2s, then read/check with --file .ddp/last.jsonl");
    }
    Ok(())
}

struct Burst<'a> {
    start: f64,
    last: f64,
    before: &'a Frame,
    after: &'a Frame,
    changed: HashSet<String>,
}

/// Changes closer than BURST_GAP_MS apart as one entry: what appeared, went and changed in between.
fn bursts(frames: &[(f64, Frame)], changes: &[HashSet<String>], noisy: &HashSet<String>) -> Vec<(f64, String)> {
    let mut entries = Vec::new();
    let mut burst: Option<Burst> = None;
    for (index, changed) in changes.iter().enumerate() {
        let (before, (at, frame)) = (&frames[index].1, &frames[index + 1]);
        let before_keys: HashSet<&String> = before.by_key.keys().collect();
        let after_keys: HashSet<&String> = frame.by_key.keys().collect();
        let mut changed: HashSet<String> = changed.clone();
        changed.extend(before_keys.symmetric_difference(&after_keys).map(|k| (*k).clone()));
        changed.retain(|k| !noisy.contains(k));
        if changed.is_empty() {
            continue;
        }
        if burst.as_ref().is_some_and(|b| at - b.last > BURST_GAP_MS) {
            entries.push(close_burst(burst.take().unwrap(), noisy));
        }
        let current =
            burst.get_or_insert(Burst { start: *at, last: *at, before, after: frame, changed: HashSet::new() });
        current.last = *at;
        current.after = frame;
        current.changed.extend(changed);
    }
    if let Some(burst) = burst {
        entries.push(close_burst(burst, noisy));
    }
    entries
}

fn close_burst(burst: Burst, noisy: &HashSet<String>) -> (f64, String) {
    let before: HashSet<String> = burst.before.by_key.keys().filter(|k| !noisy.contains(*k)).cloned().collect();
    let after: HashSet<String> = burst.after.by_key.keys().filter(|k| !noisy.contains(*k)).cloned().collect();
    let mut appeared: HashSet<String> = after.difference(&before).cloned().collect();
    let mut gone: HashSet<String> = before.difference(&after).cloned().collect();
    let mut parts = Vec::new();
    // An element without an id is named by its label, so a new text reads as one element going and another
    // appearing at the same place: show it as the text change it is.
    let mut gone_sorted: Vec<String> = gone.iter().cloned().collect();
    gone_sorted.sort();
    for old in gone_sorted {
        let place = &burst.before.element(&old).unwrap()["frame"];
        let mut candidates: Vec<&String> = appeared.iter().collect();
        candidates.sort();
        let new = candidates
            .into_iter()
            .find(|key| {
                let other = &burst.after.element(key).unwrap()["frame"];
                py::equal(&place[0], &other[0]) && py::equal(&place[1], &other[1])
            })
            .cloned();
        if let Some(new) = new {
            parts.push(format!("\"{old}\" -> \"{new}\""));
            appeared.remove(&new);
            gone.remove(&old);
        }
    }
    if !appeared.is_empty() {
        parts.push(format!("+{}", named(&appeared)));
    }
    if !gone.is_empty() {
        parts.push(format!("-{}", named(&gone)));
    }
    let both: HashSet<&String> = before.intersection(&after).collect();
    let changed: HashSet<String> = burst.changed.iter().filter(|k| both.contains(k)).cloned().collect();
    if !changed.is_empty() {
        parts.push(format!("changed {}", named(&changed)));
    }
    if parts.is_empty() {
        return (burst.start, String::new());
    }
    let length = burst.last - burst.start;
    let duration = if length >= 100.0 { format!(" ({:.1}s)", length / 1000.0) } else { String::new() };
    (burst.start, format!("{}{duration}", parts.join("  ")))
}

fn event_text(event: &Value, frame: Option<&Frame>) -> String {
    match event["kind"].as_str().unwrap_or("") {
        "tap" => {
            let hit = frame.and_then(|frame| element_under(frame, (f(&event["x"]), f(&event["y"]))));
            format!("tap {} ({:.0},{:.0})", hit.unwrap_or_else(|| "nothing".into()), f(&event["x"]), f(&event["y"]))
        }
        "swipe" => format!(
            "swipe ({:.0},{:.0}) -> ({:.0},{:.0})",
            f(&event["x"]),
            f(&event["y"]),
            f(&event["toX"]),
            f(&event["toY"])
        ),
        "screen" => format!("screen {}", py::str(&event["name"])),
        "launch" => "app started".into(),
        other => other.into(),
    }
}

/// Requests the app reported, while it is connected live (the library keeps the last 200 in memory).
fn request_entries(args: &Args, since: f64) -> Vec<(f64, String)> {
    if args.dir.is_some() {
        return Vec::new();
    }
    let Some(session) = live_session(args) else { return Vec::new() };
    // The app may be gone: frames and taps are still worth showing.
    let Ok(answer) = open(&session).and_then(|mut control| control.call("net.list", json!({}))) else {
        return Vec::new();
    };
    answer["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r.get("startedAt").map(f).unwrap_or(0.0) >= since)
        .map(|r| {
            let status = r.get("status").or_else(|| r.get("error")).map(py::str).unwrap_or_else(|| "failed".into());
            (
                f(&r["startedAt"]),
                format!("{} {status} {}ms {}", py::str(&r["method"]), py::fixed(&r["ms"], 0), py::str(&r["url"])),
            )
        })
        .collect()
}

/// '14:31:02' or '14:31:02.5', today (yesterday when that is still ahead), to epoch ms.
fn parse_clock(text: &str) -> Result<f64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let today = local_time("%Y-%m-%d", now_ms() / 1000.0);
    let fraction: f64 = format!("0.{fraction}").parse().unwrap_or(0.0);
    let Some(seconds) = crate::state::parse_local(&format!("{today} {whole}")) else {
        fail!(format!("can't read time '{text}'"), "bad_time", "use the time from ddp history, e.g. 14:31:02.5");
    };
    let at = seconds * 1000.0 + fraction * 1000.0;
    Ok(if at > now_ms() + 60_000.0 { at - 86_400_000.0 } else { at })
}

/// The history between `at` and `at + --for` as an ordinary take: the frame on screen at the start,
/// every change after it, and the screen held until the end.
fn take(args: &Args, segments: &[Segment]) -> Result<()> {
    let Some(time) = &args.time else {
        fail!("history at needs a time", "missing_option", "ddp history, then ddp history at 14:31:02 --for 2s");
    };
    let start = parse_clock(time)?;
    let end = start + parse_ms(args.duration.as_deref().unwrap_or("2s"))?;
    let header =
        segments.iter().rev().find(|s| f(&s.header["startedAt"]) <= start).unwrap_or(&segments[0]).header.clone();
    let frames: Vec<&Value> = segments.iter().flat_map(|s| s.lines.iter()).filter(|l| l["type"] == "frame").collect();
    let mut shown: Vec<Value> =
        frames.iter().rfind(|l| f(&l["at"]) <= start).map(|l| (*l).clone()).into_iter().collect();
    shown.extend(frames.iter().filter(|l| start < f(&l["at"]) && f(&l["at"]) <= end).map(|l| (*l).clone()));
    if shown.is_empty() {
        fail!(format!("nothing recorded by {}", clock(start)), "no_history", "ddp history to see the times it holds");
    }
    let mut held = shown.last().unwrap().clone();
    held["at"] = py::fnum(end);
    shown.push(held);
    let mut out_header = header;
    out_header["startedAt"] = py::fnum(start);
    out_header["frames"] = json!(shown.len());
    let mut out = vec![py::dumps(&out_header)];
    for (index, frame) in shown.iter().enumerate() {
        let mut line = frame.clone();
        let at = f(&line["at"]);
        line.as_object_mut().unwrap().shift_remove("at");
        line["i"] = json!(index);
        line["t"] = py::fnum((at - start).max(0.0));
        out.push(py::dumps(&line));
    }
    out.push(py::dumps(&json!({"type": "end", "frames": shown.len()})));
    save_recording(&(out.join("\n") + "\n"), args.name.as_deref())?;
    let last = ctx().last;
    say!(
        "take {} to {}: {} frames, only the ones that changed -> {}",
        clock(start),
        clock(end),
        shown.len(),
        last.display()
    );
    say!("next: ddp read --file {0}, ddp check all --file {0}", last.display());
    Ok(())
}
