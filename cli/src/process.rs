//! The app's process from the host, with each platform's own tools: memory now, leaks and the heap. A simulator app is
//! a macOS process, so `footprint`, `leaks` and `heap` read it; on Android `dumpsys meminfo` and `am dumpheap`.
use crate::args::Args;
use crate::device::target;
use crate::state::{self, adb_path, capture, Result};
use crate::{fail, say};
use std::time::{Duration, Instant};

/// Lines printed from a tool's table; the tool has them all.
const ROWS: usize = 12;

pub fn memory(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    let action = args.action.clone().unwrap_or_else(|| "summary".into());
    if platform == "ios" {
        let pid = ios_pid(&device, &app)?;
        match action.as_str() {
            "leaks" => ios_leaks(&pid),
            "heap" => ios_heap(&pid),
            _ => ios_footprint(&pid),
        }
    } else {
        match action.as_str() {
            "leaks" => fail!(
                "Android has no leak detector of its own",
                "not_supported",
                "add LeakCanary to the Debug build, or ddp memory heap and open the .hprof in Android Studio"
            ),
            "heap" => android_heap(&device, &app),
            _ => android_meminfo(&device, &app),
        }
    }
}

/// The app's pid on the simulator, from its launchd job (`UIKitApplication:<bundle id>[...]`).
fn ios_pid(device: &str, app: &str) -> Result<String> {
    let jobs = state::run(&["xcrun", "simctl", "spawn", device, "launchctl", "list"])?;
    let label = format!("UIKitApplication:{app}[");
    match jobs.lines().find(|line| line.contains(&label)).and_then(|line| line.split_whitespace().next()) {
        Some(pid) if pid != "-" => Ok(pid.to_string()),
        _ => fail!(format!("{app} is not running"), "not_running", format!("ddp launch {device} --app {app}")),
    }
}

fn ios_footprint(pid: &str) -> Result<i32> {
    match footprint_lines(&capture("footprint", &[pid]).stdout) {
        Some(lines) => lines.iter().for_each(|line| say!("{line}")),
        None => fail!("footprint gave no answer for the app", "command_failed", "ddp doctor"),
    }
    Ok(0)
}

fn ios_leaks(pid: &str) -> Result<i32> {
    let Some((lines, leaked)) = leaks_lines(&capture("leaks", &[pid]).stdout, pid) else {
        fail!("leaks gave no answer for the app", "command_failed", "ddp doctor");
    };
    lines.iter().for_each(|line| say!("{line}"));
    Ok(i32::from(leaked))
}

fn ios_heap(pid: &str) -> Result<i32> {
    match heap_lines(&capture("heap", &[pid]).stdout, pid) {
        Some(lines) => lines.iter().for_each(|line| say!("{line}")),
        None => fail!("heap gave no answer for the app", "command_failed", "ddp doctor"),
    }
    Ok(0)
}

fn android_meminfo(device: &str, app: &str) -> Result<i32> {
    let output = capture(&adb_path()?, &["-s", device, "shell", "dumpsys", "meminfo", app]).stdout;
    match meminfo_lines(&output) {
        Some(lines) => lines.iter().for_each(|line| say!("{line}")),
        None => fail!(format!("{app} is not running"), "not_running", format!("ddp launch {device} --app {app}")),
    }
    Ok(0)
}

/// Where the app spends its time for `--for`, from the platform's own sampler: `sample` on the simulator process,
/// `simpleperf` on Android. The top functions by their own samples. A measurement: it exits 0.
pub fn profile(args: &mut Args) -> Result<i32> {
    let (platform, device, app) = target(args)?;
    let Some(app) = app else { fail!("no app", "missing_option", "pass --app, or ddp connect first") };
    let seconds = (state::parse_ms(args.duration.as_deref().unwrap_or("3s"))? / 1000.0).ceil().max(1.0) as u64;
    let lines = if platform == "ios" {
        let pid = ios_pid(&device, &app)?;
        let output = capture("sample", &[&pid, &seconds.to_string()]).stdout;
        sample_lines(&output)
            .ok_or_else(|| state::failure("sample gave no answer for the app", "command_failed", Some("ddp doctor")))?
    } else {
        let adb = adb_path()?;
        let remote = "/data/local/tmp/ddp.perf";
        let recorded = capture(
            &adb,
            &[
                "-s",
                &device,
                "shell",
                "simpleperf",
                "record",
                "--app",
                &app,
                // The kernel's timer, there on every device: emulators and some phones have no cpu-cycles counter.
                "-e",
                "cpu-clock",
                "--duration",
                &seconds.to_string(),
                "-o",
                remote,
            ],
        );
        if recorded.code != 0 {
            fail!(
                format!("simpleperf failed: {}", recorded.stderr.trim()),
                "command_failed",
                "simpleperf ships with Android 10 and later; the app must be debuggable"
            );
        }
        let report = capture(
            &adb,
            &[
                "-s",
                &device,
                "shell",
                "simpleperf",
                "report",
                "-i",
                remote,
                "--sort",
                "symbol,dso",
                "--percent-limit",
                "1",
            ],
        );
        simpleperf_lines(&report.stdout)
            .ok_or_else(|| state::failure("simpleperf gave no report", "command_failed", Some("ddp doctor")))?
    };
    say!("{seconds}s on {platform} {device}, the functions the app was in, by their own samples:");
    lines.iter().for_each(|line| say!("{line}"));
    Ok(0)
}

/// `sample`'s "Sort by top of stack": function, library, samples. Kernel waits (a thread asleep in a system call)
/// are idle time, so they are counted apart rather than listed.
fn sample_lines(output: &str) -> Option<Vec<String>> {
    let rows: Vec<(String, String, u64)> = output
        .lines()
        .skip_while(|line| !line.starts_with("Sort by top of stack"))
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let (name, rest) = line.trim().split_once("  (in ")?;
            let (library, count) = rest.split_once(')')?;
            Some((name.trim().to_string(), library.to_string(), count.trim().parse().ok()?))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let (waiting, working): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|(_, library, _)| library == "libsystem_kernel.dylib");
    let total: u64 = working.iter().map(|(_, _, count)| count).sum::<u64>().max(1);
    let mut lines: Vec<String> = working
        .iter()
        .take(ROWS)
        .map(|(name, library, count)| format!("  {:>5.1}%  {name}  ({library})", *count as f64 * 100.0 / total as f64))
        .collect();
    let idle: u64 = waiting.iter().map(|(_, _, count)| count).sum();
    if idle > 0 {
        lines.push(format!("  ({idle} samples of threads waiting in the kernel left out)"));
    }
    if working.is_empty() {
        lines.insert(0, "  only threads waiting: the app was idle".into());
    }
    Some(lines)
}

/// `simpleperf report`: "Overhead  Symbol  Shared Object" rows. Kernel samples (`[kernel.kallsyms]`, no symbols for
/// an app) are added up on one line, as on iOS, so the app's own functions are what is listed.
fn simpleperf_lines(output: &str) -> Option<Vec<String>> {
    let rows: Vec<(f64, String)> = output
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            let share = line.split_whitespace().next()?.strip_suffix('%')?.parse::<f64>().ok()?;
            Some((share, line.split_whitespace().skip(1).collect::<Vec<_>>().join(" ")))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let (kernel, app): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, row)| row.contains("[kernel.kallsyms]"));
    let mut lines: Vec<String> = app.iter().take(ROWS).map(|(share, row)| format!("  {share:>5.1}%  {row}")).collect();
    let in_kernel: f64 = kernel.iter().map(|(share, _)| share).sum();
    if in_kernel > 0.0 {
        lines.push(format!("  ({in_kernel:.1}% in the kernel, left out)"));
    }
    if app.is_empty() {
        lines.insert(0, "  no app function over 1%: the app was mostly idle".into());
    }
    Some(lines)
}

/// `footprint <pid>`: its total, then the biggest categories of its table.
fn footprint_lines(output: &str) -> Option<Vec<String>> {
    let mut lines = output.lines().map(str::trim);
    let total = lines.find(|line| line.contains("Footprint:"))?;
    let size = total.split_once("Footprint:")?.1.trim();
    let size = size.split(" (").next().unwrap_or(size);
    // "Dirty  Clean  Reclaimable  Regions  Category", a --- line, then the rows, biggest first.
    let rows = lines
        .skip_while(|line| !line.starts_with("---"))
        .skip(1)
        .take_while(|line| !line.is_empty() && !line.starts_with("---"));
    Some(std::iter::once(format!("footprint {size}")).chain(rows.take(ROWS).map(|row| format!("  {row}"))).collect())
}

/// `leaks <pid>`: its counts and the first leaks; true when there is one.
fn leaks_lines(output: &str, pid: &str) -> Option<(Vec<String>, bool)> {
    let prefix = format!("Process {pid}: ");
    let counts: Vec<&str> = output.lines().filter_map(|line| line.strip_prefix(&prefix)).collect();
    let leaked = counts.iter().find(|line| line.contains("leaks for"))?;
    let has_leaks = !leaked.starts_with("0 leaks");
    let found: Vec<&str> = output.lines().map(str::trim).filter(|line| line.starts_with("Leak: ")).collect();
    let mut lines: Vec<String> = counts.iter().map(|line| line.to_string()).collect();
    lines.extend(found.iter().take(ROWS).map(|line| format!("  {line}")));
    if found.len() > ROWS {
        lines.push(format!("  ... {} more: leaks {pid} has them all", found.len() - ROWS));
    }
    Some((lines, has_leaks))
}

/// `heap <pid>`: its classes sorted by bytes, biggest first.
fn heap_lines(output: &str, pid: &str) -> Option<Vec<String>> {
    // "COUNT BYTES AVG CLASS_NAME TYPE BINARY", then a ==== line, then the rows.
    let mut rows: Vec<(u64, String)> = output
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("COUNT"))
        .skip(2)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let bytes = parts.get(1)?.parse::<u64>().ok()?;
            Some((bytes, format!("  {bytes:>10} bytes {:>7}x  {}", parts[0], parts.get(3..)?.join(" "))))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    rows.sort_by_key(|(bytes, _)| std::cmp::Reverse(*bytes));
    let header = format!("the heap by bytes, biggest first (heap {pid} has every class):");
    Some(std::iter::once(header).chain(rows.into_iter().take(ROWS).map(|(_, row)| row)).collect())
}

/// `dumpsys meminfo <app>`: its App Summary rows (Java Heap, Native Heap, Graphics, TOTAL PSS).
fn meminfo_lines(output: &str) -> Option<Vec<String>> {
    let rows: Vec<String> = output
        .lines()
        .skip_while(|line| !line.contains("App Summary"))
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with("Objects"))
        .map(str::trim)
        .filter(|line| line.contains(':') && !line.starts_with("Pss("))
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    (!rows.is_empty()).then_some(rows)
}

/// `am dumpheap` writes the .hprof on the device; it is pulled into .ddp for Android Studio or another reader.
fn android_heap(device: &str, app: &str) -> Result<i32> {
    let adb = adb_path()?;
    let remote = "/data/local/tmp/ddp-heap.hprof";
    capture(&adb, &["-s", device, "shell", "rm", "-f", remote]);
    let dumped = capture(&adb, &["-s", device, "shell", "am", "dumpheap", app, remote]);
    if dumped.code != 0 {
        fail!(
            format!("am dumpheap failed: {}", dumped.stderr.trim()),
            "command_failed",
            format!("ddp launch {device} --app {app}")
        );
    }
    // dumpheap returns before the file is complete: wait until its size stops growing.
    let (deadline, mut last) = (Instant::now() + Duration::from_secs(60), String::new());
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let size = capture(&adb, &["-s", device, "shell", "stat", "-c", "%s", remote]).stdout.trim().to_string();
        if (!size.is_empty() && size == last && size != "0") || Instant::now() >= deadline {
            break;
        }
        last = size;
    }
    let local =
        state::scratch_file(&format!("heap-{}.hprof", state::local_time("%Y%m%d-%H%M%S", state::now_ms() / 1000.0)));
    let pulled = capture(&adb, &["-s", device, "pull", remote, &local.display().to_string()]);
    if pulled.code != 0 {
        fail!(format!("adb pull failed: {}", pulled.stderr.trim()), "command_failed");
    }
    let bytes = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
    say!("{}: {bytes} bytes; open it in Android Studio (Profiler > Load from file)", local.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprint_says_the_total_and_the_biggest_categories() {
        let lines = footprint_lines(include_str!("testdata/footprint.txt")).unwrap();
        assert!(lines[0].starts_with("footprint ") && lines[0].ends_with("KB"), "{lines:?}");
        assert!(lines[1].contains("Malloc"), "{lines:?}");
        assert!(lines.len() <= ROWS + 1);
    }

    #[test]
    fn leaks_says_its_counts_and_whether_there_is_one() {
        let pid = include_str!("testdata/leaks.txt").lines().find_map(|l| l.strip_prefix("Process:")).unwrap();
        let pid = pid.split('[').nth(1).unwrap().trim_end_matches(']');
        let (lines, leaked) = leaks_lines(include_str!("testdata/leaks.txt"), pid).unwrap();
        assert!(lines.iter().any(|l| l.contains("leaks for")) && !leaked, "{lines:?}");
        assert!(leaks_lines("nothing", "1").is_none());
    }

    #[test]
    fn heap_is_sorted_by_bytes() {
        let lines = heap_lines(include_str!("testdata/heap.txt"), "1").unwrap();
        let bytes: Vec<u64> =
            lines[1..].iter().map(|l| l.split_whitespace().next().unwrap().parse().unwrap()).collect();
        assert!(bytes.windows(2).all(|w| w[0] >= w[1]) && !bytes.is_empty(), "{lines:?}");
    }

    #[test]
    fn a_profile_leaves_kernel_waits_out_and_says_so() {
        let lines = sample_lines(include_str!("testdata/sample.txt")).unwrap();
        assert!(lines.iter().any(|l| l.contains("waiting in the kernel left out")), "{lines:?}");
        let made = "Sort by top of stack, same collapsed (when >= 5):\n        layoutSubviews  (in UIKitCore)        30\n        __psynch_cvwait  (in libsystem_kernel.dylib)        900\n        render  (in DDPSample.debug.dylib)        10\n\n";
        assert_eq!(
            sample_lines(made).unwrap(),
            vec![
                "   75.0%  layoutSubviews  (UIKitCore)",
                "   25.0%  render  (DDPSample.debug.dylib)",
                "  (900 samples of threads waiting in the kernel left out)"
            ]
        );
        let report = "Cmdline: /system/bin/simpleperf record\nOverhead  Symbol  Shared Object\n31.20%  art::Thread  libart.so\n5.01%   dev.agentddp.Capture.walkLayout  base.apk\n40.00%  [kernel.kallsyms][+ffffffc07a5f9500]  [kernel.kallsyms]\n";
        assert_eq!(
            simpleperf_lines(report).unwrap(),
            vec![
                "   31.2%  art::Thread libart.so",
                "    5.0%  dev.agentddp.Capture.walkLayout base.apk",
                "  (40.0% in the kernel, left out)"
            ]
        );
    }

    #[test]
    fn meminfo_keeps_the_app_summary() {
        let output = " App Summary\n                       Pss(KB)                        Rss(KB)\n                        ------                         ------\n           Java Heap:     4248                          21468\n         Native Heap:    12564                          13200\n           TOTAL PSS:    55555            TOTAL RSS:   120000\n\n Objects\n               Views:       12\n";
        assert_eq!(
            meminfo_lines(output).unwrap(),
            vec!["Java Heap: 4248 21468", "Native Heap: 12564 13200", "TOTAL PSS: 55555 TOTAL RSS: 120000"]
        );
    }
}
