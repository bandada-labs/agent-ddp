//! Leases: one owner per device on this machine. `ddp up` takes one for the emulator it starts and `ddp lease` for
//! any adb device; a command for a device another owner holds is refused. The files live in the user's home, so every
//! folder and session gets the same answer.
use crate::args::Args;
use crate::state::{adb_path, ctx, failure, local_time, now_ms, read_json, write_file, Failure, Result};
use crate::{fail, py, say};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// How long the device gets to say which boot it is on.
const PATIENCE: Duration = Duration::from_secs(5);

fn lease_file(device: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    // A serial can be an address (192.168.1.5:5555), and a colon is no file name on Windows.
    let name: String =
        device.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    Some(PathBuf::from(home).join(".ddp").join("leases").join(format!("{name}.json")))
}

/// Who this ddp runs for: `DDP_OWNER`, or the folder it runs in.
pub fn owner() -> String {
    std::env::var("DDP_OWNER").ok().filter(|name| !name.is_empty()).unwrap_or_else(|| {
        std::env::current_dir().map(|folder| folder.display().to_string()).unwrap_or_else(|_| ".".into())
    })
}

/// The lease written for this device, whether or not the device still runs.
pub fn held(device: &str) -> Option<Value> {
    read_json(&lease_file(device)?)
}

/// What tells this boot of a device from the next one on the same serial: the kernel's own id for the boot.
fn boot_id(device: &str) -> Option<String> {
    let adb = adb_path().ok()?;
    let words = ["-s", device, "shell", "cat", "/proc/sys/kernel/random/boot_id"];
    let answer = crate::up::capture_within(&adb, &words, PATIENCE)?;
    let id = answer.stdout.trim();
    (answer.code == 0 && id.len() == 36).then(|| id.to_string())
}

/// The lease that stands on this device, whoever holds it. A lease names one boot of the device and stands only
/// while the device answers with that boot: a new emulator on the same port is another device (the old lease is
/// removed), and for a device that does not answer nobody can tell, so no lease is claimed for it.
pub fn standing(device: &str) -> Option<Value> {
    let lease = held(device)?;
    if lease["boot_id"] != boot_id(device)?.as_str() {
        let _ = std::fs::remove_file(lease_file(device)?);
        return None;
    }
    Some(lease)
}

/// Another owner's lease that stands. The owner's own costs no question to the device.
fn foreign(device: &str) -> Option<Value> {
    if held(device)?["owner"] == owner().as_str() {
        return None;
    }
    standing(device)
}

fn refusal(device: &str, lease: &Value) -> Failure {
    let since = local_time("%H:%M", lease["since_ms"].as_f64().unwrap_or(0.0) / 1000.0);
    failure(
        format!("{device} is leased to {} since {since}: one owner per device", py::str(&lease["owner"])),
        "leased",
        Some(&format!(
            "ddp up --platform android for one of your own, or its owner frees it: ddp release {device} (ddp devices lists the leases)"
        )),
    )
}

/// Refuses a command for a device that another owner holds.
pub fn refuse_foreign(device: &str) -> Result<()> {
    foreign(device).map_or(Ok(()), |lease| Err(refusal(device, &lease)))
}

/// Takes the device for this owner, with what `up` knows about it. Refused when another owner holds it.
pub fn take(device: &str, known: &Value) -> Result<Value> {
    if let Some(lease) = foreign(device) {
        return Err(refusal(device, &lease));
    }
    let Some(file) = lease_file(device) else {
        fail!("no home folder to keep leases in (HOME is not set)", "not_supported");
    };
    let Some(boot) = boot_id(device) else {
        fail!(
            format!("{device} did not say which boot it is on, so a lease cannot name it"),
            "device_not_found",
            "ddp devices"
        );
    };
    let mut lease = json!({"device": device, "owner": owner(), "since_ms": now_ms() as u64, "boot_id": boot});
    for (key, value) in known.as_object().into_iter().flatten() {
        lease[key] = value.clone();
    }
    let text = py::dumps(&lease);
    if held(device).is_some_and(|mine| mine["owner"] == owner().as_str()) {
        write_file(&file, &text)?;
        return Ok(lease);
    }
    // Two owners can ask at the same moment: the file is made by exactly one of them.
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let made = std::fs::OpenOptions::new().write(true).create_new(true).open(&file);
    match made.and_then(|mut made| std::io::Write::write_all(&mut made, text.as_bytes())) {
        Ok(()) => Ok(lease),
        Err(_) => match held(device) {
            Some(theirs) if theirs["owner"] != owner().as_str() => Err(refusal(device, &theirs)),
            _ => fail!(format!("can't write the lease {}", file.display()), "write_failed"),
        },
    }
}

/// Forgets the lease, as `ddp down` does for the device it removes.
pub fn drop_lease(device: &str) {
    if let Some(file) = lease_file(device) {
        let _ = std::fs::remove_file(file);
    }
}

/// `ddp lease <device>`.
pub fn lease(args: &mut Args) -> Result<i32> {
    let device = args.device.clone().unwrap_or_default();
    let lease = take(&device, &json!({}))?;
    if ctx().json {
        say!("{}", py::dumps(&lease));
    } else {
        say!("leased {device} to {}; ddp release {device} frees it", owner());
    }
    Ok(0)
}

/// `ddp release <device>`: the owner's own lease, or with `--force` another owner's.
pub fn release(args: &mut Args) -> Result<i32> {
    let device = args.device.clone().unwrap_or_default();
    let Some(lease) = held(&device) else {
        say!("{device} has no lease");
        return Ok(0);
    };
    let theirs = lease["owner"] != owner().as_str();
    if theirs && !args.force {
        fail!(
            format!("{device} is leased to {}, not to {}", py::str(&lease["owner"]), owner()),
            "leased",
            format!("ddp release {device} --force, when its owner is gone")
        );
    }
    drop_lease(&device);
    let whose = if theirs { format!(", it was leased to {}", py::str(&lease["owner"])) } else { String::new() };
    say!("released {device}{whose}");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_serial_with_a_colon_is_a_file_name() {
        let file = lease_file("192.168.1.5:5555").unwrap();
        assert_eq!(file.file_name().unwrap().to_string_lossy(), "192.168.1.5_5555.json");
    }
}
