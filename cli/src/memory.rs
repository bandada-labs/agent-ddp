//! Memory before an emulator boots, and what was left behind. `ddp up` does not start an emulator the machine has
//! no room for, and `ddp clean` ends the Gradle daemons that Gradle itself marked idle. Both read `/proc`, so they
//! are for Linux; elsewhere the amounts are unknown and nothing is refused.
use crate::args::Args;
use crate::state::{capture, Result};
use crate::{fail, say};
use std::path::Path;
use std::time::{Duration, Instant};

/// What an emulator takes beyond its guest's RAM (its own process, the renderer), kept free on top of it.
const HEADROOM_MB: u64 = 2048;
/// How long a Gradle daemon told to stop gets before ddp says it still runs.
const STOP_PATIENCE: Duration = Duration::from_secs(10);

fn field_kb(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|line| line.strip_prefix(name)?.split_whitespace().next()?.parse().ok())
}

fn available_mb() -> Option<u64> {
    field_kb(&std::fs::read_to_string("/proc/meminfo").ok()?, "MemAvailable:").map(|kb| kb / 1024)
}

/// A size as an AVD's config or `--min-free` writes it: `2G`, `2048M`, or a bare number in `bare_mb` megabytes.
fn megabytes(text: &str, bare_mb: f64) -> Option<u64> {
    let text = text.trim();
    let (number, unit_mb) = match text.chars().last()?.to_ascii_lowercase() {
        'g' => (&text[..text.len() - 1], 1024.0),
        'm' => (&text[..text.len() - 1], 1.0),
        _ => (text, bare_mb),
    };
    number.trim().parse::<f64>().ok().filter(|n| *n >= 0.0).map(|n| (n * unit_mb) as u64)
}

/// The guest RAM in an AVD's `config.ini` (`hw.ramSize=2G`, or megabytes without a unit).
fn guest_ram_mb(config: &str) -> Option<u64> {
    let value = config.lines().find_map(|line| line.strip_prefix("hw.ramSize"))?;
    megabytes(value.trim_start().strip_prefix('=')?, 1.0)
}

fn gigabytes(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
}

/// Stops `ddp up` before the boot when less memory is available than the emulator needs: its guest RAM plus
/// headroom, or the bar `--min-free` sets (0 for none). Silent where the amounts cannot be read.
pub fn guard(avd: &str, min_free: Option<&str>) -> Result<()> {
    let guest = crate::up::avd_folder(avd)
        .and_then(|folder| std::fs::read_to_string(folder.join("config.ini")).ok())
        .and_then(|config| guest_ram_mb(&config));
    let (needed, why) = match (min_free, guest) {
        (Some(bar), _) => {
            let Some(bar) = megabytes(bar, 1024.0) else {
                fail!(format!("--min-free takes gigabytes (8, 7.5, 6000m), not '{bar}'"), "bad_value", "--min-free 8");
            };
            (bar, "--min-free".to_string())
        }
        (None, Some(guest)) => (
            guest + HEADROOM_MB,
            format!("{} for {avd}'s guest and {} beside it", gigabytes(guest), gigabytes(HEADROOM_MB)),
        ),
        (None, None) => return Ok(()),
    };
    let Some(available) = available_mb().filter(|available| *available < needed) else {
        return Ok(());
    };
    let idle = idle_gradle_daemons();
    let (held, next) = if idle.is_empty() {
        (String::new(), "free some memory, or set the bar yourself: --min-free <GB> (0 for none)".to_string())
    } else {
        let total = idle.iter().map(|daemon| daemon.resident_mb).sum();
        let plural = if idle.len() == 1 { "" } else { "s" };
        (
            format!(
                "; {} idle Gradle daemon{plural} hold{} {}",
                idle.len(),
                if idle.len() == 1 { "s" } else { "" },
                gigabytes(total)
            ),
            "ddp clean, then the same command".to_string(),
        )
    };
    fail!(
        format!(
            "no emulator started: {} of memory available, {} needed ({why}){held}",
            gigabytes(available),
            gigabytes(needed)
        ),
        "low_memory",
        next
    )
}

struct GradleDaemon {
    pid: u32,
    version: String,
    resident_mb: u64,
}

/// Whether Gradle's own log of a daemon ends on idle: its last "Marking the daemon as ..." line. None without one.
fn marked_idle(daemon_log: &str) -> Option<bool> {
    const MARK: &str = "Marking the daemon as ";
    let last = daemon_log.rfind(MARK)?;
    Some(daemon_log[last + MARK.len()..].starts_with("idle"))
}

/// The Gradle daemons of this user that Gradle marked idle: no build runs in them, and they only hold memory until
/// their own timeout (3 hours). A daemon with a build, or one whose log cannot be read, is not in the list.
fn idle_gradle_daemons() -> Vec<GradleDaemon> {
    let processes = std::fs::read_dir("/proc").into_iter().flatten().flatten();
    processes
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_string_lossy().parse().ok()?;
            let command = std::fs::read(entry.path().join("cmdline")).ok()?;
            if !String::from_utf8_lossy(&command).contains("org.gradle.launcher.daemon.bootstrap.GradleDaemon") {
                return None;
            }
            // A daemon runs in its own folder, ~/.gradle/daemon/<version>, next to its log.
            let folder = std::fs::read_link(entry.path().join("cwd")).ok()?;
            let log = std::fs::read(folder.join(format!("daemon-{pid}.out.log"))).ok()?;
            if marked_idle(&String::from_utf8_lossy(&log)) != Some(true) {
                return None;
            }
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            Some(GradleDaemon {
                pid,
                version: folder.file_name()?.to_string_lossy().into_owned(),
                resident_mb: field_kb(&status, "VmRSS:")? / 1024,
            })
        })
        .collect()
}

/// `ddp clean`: ends the idle Gradle daemons and says what memory that freed.
pub fn clean(_args: &mut Args) -> Result<i32> {
    if !Path::new("/proc/meminfo").exists() {
        fail!("ddp clean reads /proc to find what was left behind: Linux only", "not_supported");
    }
    let idle = idle_gradle_daemons();
    let mut freed = 0;
    for daemon in &idle {
        let process = format!("/proc/{}", daemon.pid);
        // The signal `gradle --stop` ends in too: an idle daemon closes its caches and exits.
        capture("kill", &[&daemon.pid.to_string()]);
        let deadline = Instant::now() + STOP_PATIENCE;
        while Path::new(&process).exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let held = gigabytes(daemon.resident_mb);
        if Path::new(&process).exists() {
            say!(
                "idle Gradle daemon {} (Gradle {}) was told to stop and still runs: {held}",
                daemon.pid,
                daemon.version
            );
        } else {
            freed += daemon.resident_mb;
            say!("ended idle Gradle daemon {} (Gradle {}): {held}", daemon.pid, daemon.version);
        }
    }
    let available = available_mb().map(|mb| format!("; {} available now", gigabytes(mb))).unwrap_or_default();
    if idle.is_empty() {
        say!("clean: no idle Gradle daemon{available}");
    } else {
        say!("clean: {} freed{available}", gigabytes(freed));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sizes_as_the_avd_and_the_option_write_them() {
        assert_eq!(guest_ram_mb("hw.lcd.width=1080\nhw.ramSize=2G\n"), Some(2048));
        assert_eq!(guest_ram_mb("hw.ramSize = 1536\n"), Some(1536));
        assert_eq!(guest_ram_mb("hw.lcd.width=1080\n"), None);
        assert_eq!(megabytes("8", 1024.0), Some(8192));
        assert_eq!(megabytes("7.5g", 1024.0), Some(7680));
        assert_eq!(megabytes("6000m", 1024.0), Some(6000));
        assert_eq!(megabytes("lots", 1024.0), None);
    }

    #[test]
    fn a_daemon_is_idle_only_by_gradles_last_mark() {
        let busy = "[INFO] Marking the daemon as idle, address: [x]\n[INFO] Marking the daemon as busy, address: [x]\n";
        assert_eq!(marked_idle(busy), Some(false));
        assert_eq!(
            marked_idle(&format!("{busy}[DEBUG] build\n[INFO] Marking the daemon as idle, address: [x]\n")),
            Some(true)
        );
        assert_eq!(marked_idle("[DEBUG] daemon started\n"), None);
    }
}
