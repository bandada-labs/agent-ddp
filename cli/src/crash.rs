//! `ddp crash`: the app's last crash, as the platform recorded it. iOS: the crash report macOS writes for a simulator
//! app (`~/Library/Logs/DiagnosticReports/*.ips`, already symbolicated), plus the message the app logged as it died.
//! Android: the crash log buffer (`logcat -b crash`), which keeps Java exceptions and native crashes of every app.
//! Nothing runs in the app: it is gone, and a handler inside it could not see a Swift trap safely.
use crate::args::Args;
use crate::device::target;
use crate::py;
use crate::state::{self, adb_path, capture, Result};
use crate::{fail, say};
use serde_json::Value;
use std::path::PathBuf;

/// Stack frames printed; the report has them all.
const FRAMES: usize = 15;

pub fn crash(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else {
        fail!("no app", "missing_option", "pass --app, or ddp connect first");
    };
    let found = if platform == "ios" { ios_crash(&device, &app)? } else { android_crash(&device, &app)? };
    match found {
        Some(lines) => lines.iter().for_each(|line| say!("{line}")),
        None => say!("no crash of {app} on {device} in what the platform kept"),
    }
    Ok(0)
}

fn ios_crash(device: &str, app: &str) -> Result<Option<Vec<String>>> {
    let folder = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Logs/DiagnosticReports");
    let mut reports: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&folder)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "ips"))
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    reports.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in reports {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Some((header, body)) = text.split_once('\n') else { continue };
        let Ok(header) = serde_json::from_str::<Value>(header) else { continue };
        if header["bundleID"] != app {
            continue;
        }
        let Ok(report) = serde_json::from_str::<Value>(body) else { continue };
        // The same app on another simulator writes to the same folder.
        if !py::str(&report["procPath"]).contains(&format!("/Devices/{device}/")) {
            continue;
        }
        return Ok(Some(ios_lines(&report, device, &path)));
    }
    Ok(None)
}

fn ios_lines(report: &Value, device: &str, path: &std::path::Path) -> Vec<String> {
    let exception = &report["exception"];
    let time = py::str(&report["captureTime"]).chars().take(19).collect::<String>();
    let mut lines = vec![format!(
        "crash at {time}{} on ios {device}: {} ({})",
        ago(&time),
        py::str(&exception["type"]),
        py::str(&exception["signal"])
    )];
    if let Some(message) = logged_message(device, &py::str(&report["procName"]), &time) {
        lines.push(message);
    }
    let images = report["usedImages"].as_array().cloned().unwrap_or_default();
    // The app's own code is what the platform loaded from inside the app's bundle.
    let bundle =
        py::str(&report["procPath"]).rsplit_once('/').map(|(folder, _)| folder.to_string()).unwrap_or_default();
    let threads = report["threads"].as_array().cloned().unwrap_or_default();
    let Some(thread) = threads.iter().find(|thread| thread["triggered"] == true) else { return lines };
    let frames = thread["frames"].as_array().cloned().unwrap_or_default();
    let text = |frame: &Value| {
        let image = &images[frame["imageIndex"].as_u64().unwrap_or(0) as usize % images.len().max(1)];
        let place = match (frame["sourceFile"].as_str(), frame["sourceLine"].as_u64()) {
            (Some(file), Some(line)) => format!(" {file}:{line}"),
            _ => String::new(),
        };
        let symbol = frame.get("symbol").map(py::str).unwrap_or_else(|| "?".into());
        (py::str(&image["path"]).starts_with(&bundle), format!("{} {symbol}{place}", py::str(&image["name"])))
    };
    if let Some((_, first)) = frames.iter().map(text).find(|(in_app, _)| *in_app) {
        lines.push(format!("first frame in the app: {first}"));
    }
    let queue = thread.get("queue").map(|queue| format!(", {}", py::str(queue))).unwrap_or_default();
    lines.push(format!("thread {}{queue}:", py::str(&report["faultingThread"])));
    lines.extend(frames.iter().take(FRAMES).map(|frame| format!("  {}", text(frame).1)));
    lines.push(format!("report: {}", path.display()));
    lines
}

/// What the app logged as it died: a Swift `Fatal error: ...`, or an uncaught Objective-C exception's reason.
fn logged_message(device: &str, process: &str, crashed_at: &str) -> Option<String> {
    let crashed = state::parse_local(crashed_at)?;
    let (start, end) =
        (state::local_time("%Y-%m-%d %H:%M:%S", crashed - 10.0), state::local_time("%Y-%m-%d %H:%M:%S", crashed + 1.0));
    let predicate = format!(
        "process == \"{process}\" AND (eventMessage CONTAINS \"Fatal error\" OR eventMessage CONTAINS \"Terminating app due to uncaught exception\")"
    );
    let output = capture(
        "xcrun",
        &[
            "simctl",
            "spawn",
            device,
            "log",
            "show",
            "--style",
            "compact",
            "--start",
            &start,
            "--end",
            &end,
            "--predicate",
            &predicate,
        ],
    );
    output.stdout.lines().rev().find_map(|line| {
        ["Fatal error", "Terminating app due to uncaught exception"]
            .iter()
            .find_map(|marker| line.find(marker).map(|at| line[at..].trim().to_string()))
    })
}

fn android_crash(device: &str, app: &str) -> Result<Option<Vec<String>>> {
    let output = capture(&adb_path()?, &["-s", device, "logcat", "-b", "crash", "-d", "-v", "epoch"]);
    if output.code != 0 {
        fail!(
            format!("adb logcat failed: {}", output.stderr.trim()),
            "command_failed",
            format!("ddp doctor {device} --app {app}")
        );
    }
    Ok(android_blocks(&output.stdout, app).pop().map(|block| {
        let mut lines =
            vec![format!("crash at {}{} on android {device}: {}", block.time, ago(&block.time), block.title)];
        lines.extend(block.lines.iter().take(FRAMES).map(|line| format!("  {line}")));
        let causes: Vec<String> =
            block.lines.iter().skip(FRAMES).filter(|line| line.starts_with("Caused by:")).cloned().collect();
        lines.extend(causes.into_iter().map(|line| format!("  {line}")));
        lines
    }))
}

/// " (12 min ago)": the last crash may be old, and an agent must not take it for the one it just caused.
fn ago(local: &str) -> String {
    let Some(at) = state::parse_local(local) else { return String::new() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(at);
    match (now - at).max(0.0) {
        seconds if seconds < 90.0 => format!(" ({seconds:.0} s ago)"),
        seconds if seconds < 5400.0 => format!(" ({:.0} min ago)", seconds / 60.0),
        seconds if seconds < 172_800.0 => format!(" ({:.0} h ago)", seconds / 3600.0),
        seconds => format!(" ({:.0} days ago)", seconds / 86_400.0),
    }
}

struct Block {
    time: String,
    title: String,
    lines: Vec<String>,
}

/// The app's crashes in the crash buffer, oldest first: Java ones (`AndroidRuntime: FATAL EXCEPTION`, with
/// `Process: <app>`) and native ones (a `DEBUG` tombstone naming `>>> <app> <<<`).
fn android_blocks(log: &str, app: &str) -> Vec<Block> {
    let mut blocks: Vec<(Block, bool)> = Vec::new();
    for line in log.lines() {
        // "1729700000.123  4321  4321 E AndroidRuntime: text"
        let mut parts = line.split_whitespace();
        let (Some(seconds), Some(_), Some(_), Some(_), Some(tag)) =
            (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // "DEBUG   : text" puts the colon apart from the tag.
        let Some(text) =
            line.split_once(&format!("{tag} ")).map(|(_, text)| text.trim().trim_start_matches(':').trim())
        else {
            continue;
        };
        let tag = tag.trim_end_matches(':');
        let starts = (tag == "AndroidRuntime" && text.starts_with("FATAL EXCEPTION"))
            || (tag == "DEBUG" && text.starts_with("*** *** ***"));
        if starts {
            let time = seconds.parse::<f64>().map(|s| state::local_time("%Y-%m-%d %H:%M:%S", s)).unwrap_or_default();
            blocks.push((Block { time, title: String::new(), lines: Vec::new() }, false));
            continue;
        }
        let Some((block, ours)) = blocks.last_mut() else { continue };
        if tag != "AndroidRuntime" && tag != "DEBUG" {
            continue;
        }
        if text.starts_with("Process: ") {
            *ours = text.trim_start_matches("Process: ").split(',').next() == Some(app);
        } else if text.contains(&format!(">>> {app} <<<")) {
            *ours = true;
        } else if block.title.is_empty() && (tag == "AndroidRuntime" || text.starts_with("signal ")) {
            block.title = text.to_string();
        } else if !block.title.is_empty() {
            // A tombstone also lists registers and memory: its backtrace and abort message are what tell.
            let telling =
                tag == "AndroidRuntime" || ["#", "Abort message", "Cause:"].iter().any(|start| text.starts_with(start));
            if telling {
                block.lines.push(text.to_string());
            }
        }
    }
    blocks.into_iter().filter(|(_, ours)| *ours).map(|(block, _)| block).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_s_java_crash_is_read_and_another_app_s_is_not() {
        let log = "\
1729700000.100  4321  4321 E AndroidRuntime: FATAL EXCEPTION: main
1729700000.100  4321  4321 E AndroidRuntime: Process: dev.agentddp.sample, PID: 4321
1729700000.100  4321  4321 E AndroidRuntime: java.lang.IllegalStateException: the sample crashed on purpose
1729700000.100  4321  4321 E AndroidRuntime: \tat dev.agentddp.sample.CommandsCompositionKt.crash(CommandsComposition.kt:20)
1729700000.100  4321  4321 E AndroidRuntime: Caused by: java.lang.NullPointerException
1729700009.000  9999  9999 E AndroidRuntime: FATAL EXCEPTION: main
1729700009.000  9999  9999 E AndroidRuntime: Process: com.other.app, PID: 9999
1729700009.000  9999  9999 E AndroidRuntime: java.lang.RuntimeException: not ours
";
        let blocks = android_blocks(log, "dev.agentddp.sample");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].title, "java.lang.IllegalStateException: the sample crashed on purpose");
        assert_eq!(blocks[0].lines[0], "at dev.agentddp.sample.CommandsCompositionKt.crash(CommandsComposition.kt:20)");
        assert_eq!(blocks[0].lines[1], "Caused by: java.lang.NullPointerException");
    }

    #[test]
    fn a_native_crash_names_its_signal() {
        let log = "\
1729700000.100  5000  5000 F DEBUG   : *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***
1729700000.100  5000  5000 F DEBUG   : pid: 4321, tid: 4321, name: sample  >>> dev.agentddp.sample <<<
1729700000.100  5000  5000 F DEBUG   : signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0
1729700000.100  5000  5000 F DEBUG   :       #00 pc 0000000000012345  /data/app/lib/arm64/libsample.so (crash+12)
";
        let blocks = android_blocks(log, "dev.agentddp.sample");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].title.starts_with("signal 11 (SIGSEGV)"));
        assert!(blocks[0].lines[0].starts_with("#00 pc"));
    }
}
