//! `ddp up` and `ddp down`: a simulator or emulator of ddp's own for a run, made from a warm base and removed after.
//!
//! iOS clones a base simulator that has booted once (`simctl clone`), so the clone skips first-boot work.
//! Android starts the base AVD read-only from its quickboot snapshot, so several runs share one AVD and leave
//! nothing on disk. Only devices listed in `.ddp/devices.json` are ever shut down or deleted by `ddp down`.
use crate::args::Args;
use crate::live;
use crate::py;
use crate::state::{adb_path, capture, ctx, failure, now_ms, read_json, write_file, Result};
use crate::{fail, say, warn};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const IOS_BASE: &str = "ddp-base";
const ANDROID_BASE: &str = "ddp_base";
const IOS_RUN_PREFIX: &str = "ddp-run-";
/// Emulator console ports ddp may take. 5554-5558 are left to people and other tools.
const ANDROID_PORTS: std::ops::RangeInclusive<u16> = 5560..=5680;
const BOOT_PATIENCE: Duration = Duration::from_secs(180);
/// How long `up --app` waits for the app's AgentDDP endpoint before saying it has none.
const ENDPOINT_PATIENCE: Duration = Duration::from_secs(8);

fn devices_file() -> PathBuf {
    ctx().dir.join("devices.json")
}

fn owned_devices() -> Vec<Value> {
    read_json(&devices_file()).and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

fn save_owned(devices: &[Value]) -> Result<()> {
    write_file(&devices_file(), py::dumps(&Value::Array(devices.to_vec())))
}

pub fn up(args: &mut Args) -> Result<i32> {
    let Some(platform) = args.platform.clone() else {
        fail!("which platform?", "missing_platform", "ddp up --platform ios");
    };
    let started = Instant::now();
    let base = args.name.clone();
    let device = if platform == "ios" { up_ios(base.as_deref())? } else { up_android(base.as_deref())? };
    let mut owned = owned_devices();
    owned.push(device.clone());
    save_owned(&owned)?;
    let id = py::str(&device["device"]);
    say!("up {platform} {id} in {}s", seconds(started));

    if let Some(build) = args.path.clone() {
        install(&platform, &id, &build)?;
    }
    if let Some(app) = args.app.clone() {
        start_app(&platform, &id, &app)?;
        args.device = Some(id.clone());
        args.platform = Some(platform.clone());
        match live::attach_within(&platform, &id, &app, ENDPOINT_PATIENCE) {
            Ok((session, snapshot)) => return live::keep_session(&session, &snapshot),
            Err(reason) => warn!("ddp: {app} started, not connected: {}", reason.message),
        }
    }
    Ok(0)
}

pub fn down(args: &mut Args) -> Result<i32> {
    let owned = owned_devices();
    let session = read_json(&ctx().session_file).unwrap_or(json!({}));
    let wanted: Vec<Value> = if args.full {
        owned.clone()
    } else {
        let id = args.device.clone().or_else(|| session["device"].as_str().map(String::from));
        let chosen = match id {
            Some(id) => owned.iter().find(|d| d["device"] == id.as_str()).cloned(),
            None if owned.len() == 1 => owned.first().cloned(),
            None => None,
        };
        let Some(chosen) = chosen else {
            let listed: Vec<String> = owned.iter().map(|d| py::str(&d["device"])).collect();
            fail!(
                "no device of ddp's own to shut down",
                "not_owned",
                if listed.is_empty() { "ddp up --platform ios".to_string() } else { format!("ddp down {}", listed[0]) }
            );
        };
        vec![chosen]
    };
    for device in &wanted {
        let id = py::str(&device["device"]);
        if device["platform"] == "ios" {
            // The name check is a second lock: never delete a simulator ddp did not make.
            if !py::str(&device["name"]).starts_with(IOS_RUN_PREFIX) {
                fail!(format!("{id} is not a ddp run simulator"), "not_owned");
            }
            capture("xcrun", &["simctl", "shutdown", &id]);
            let deleted = capture("xcrun", &["simctl", "delete", &id]);
            if deleted.code != 0 && !deleted.stderr.contains("Invalid device") {
                fail!(format!("simctl delete failed: {}", deleted.stderr.trim()), "command_failed");
            }
        } else {
            // Read-only: the emulator saves nothing back to the base AVD.
            capture(&adb_path()?, &["-s", &id, "emu", "kill"]);
        }
        say!("down {} {id}", py::str(&device["platform"]));
    }
    let kept: Vec<Value> = owned.into_iter().filter(|d| !wanted.contains(d)).collect();
    save_owned(&kept)?;
    if wanted.iter().any(|d| session["device"] == d["device"]) {
        let _ = std::fs::remove_file(ctx().session_file);
    }
    Ok(0)
}

fn seconds(since: Instant) -> String {
    format!("{:.1}", since.elapsed().as_secs_f64())
}

// iOS

fn up_ios(base_name: Option<&str>) -> Result<Value> {
    let base_name = base_name.unwrap_or(IOS_BASE);
    let base = match find_simulator(base_name)? {
        Some(found) => found,
        None => make_ios_base(base_name)?,
    };
    if base.1 != "Shutdown" {
        fail!(
            format!("the base simulator {base_name} is {}; a clone needs it shut down", base.1),
            "base_busy",
            format!("xcrun simctl shutdown {}", base.0)
        );
    }
    let name = format!("{IOS_RUN_PREFIX}{}", now_ms() as u64);
    let cloned = capture("xcrun", &["simctl", "clone", &base.0, &name]);
    let id = cloned.stdout.trim().to_string();
    if cloned.code != 0 || id.is_empty() {
        fail!(format!("simctl clone failed: {}", cloned.stderr.trim()), "command_failed");
    }
    let device = json!({"platform": "ios", "device": id, "name": name, "base": base_name});
    if let Err(reason) = boot_ios(&id) {
        capture("xcrun", &["simctl", "delete", &id]);
        return Err(reason);
    }
    Ok(device)
}

/// (udid, state) of the simulator with that name.
fn find_simulator(name: &str) -> Result<Option<(String, String)>> {
    let listed = capture("xcrun", &["simctl", "list", "devices", "-j"]);
    let devices: Value = serde_json::from_str(&listed.stdout)
        .map_err(|_| failure("simctl list failed", "command_failed", Some("xcode-select -p")))?;
    let found = devices["devices"]
        .as_object()
        .into_iter()
        .flat_map(|runtimes| runtimes.values())
        .flat_map(|list| list.as_array().cloned().unwrap_or_default())
        .find(|d| d["name"] == name && d["isAvailable"] != false);
    Ok(found.map(|d| (py::str(&d["udid"]), py::str(&d["state"]))))
}

/// The newest available iOS runtime with its newest iPhone, booted once and shut down so clones start warm.
fn make_ios_base(name: &str) -> Result<(String, String)> {
    let runtimes: Value = serde_json::from_str(&capture("xcrun", &["simctl", "list", "runtimes", "-j"]).stdout)
        .map_err(|_| failure("simctl list failed", "command_failed", None))?;
    let runtime = runtimes["runtimes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["platform"] == "iOS" && r["isAvailable"] == true)
        .max_by_key(|r| version_key(&py::str(&r["version"])));
    let Some(runtime) = runtime else {
        fail!("no iOS runtime installed", "no_runtime", "xcodebuild -downloadPlatform iOS");
    };
    let iphone = runtime["supportedDeviceTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["productFamily"] == "iPhone")
        .max_by_key(|t| {
            let name = py::str(&t["name"]);
            (version_key(&py::str(&t["minRuntimeVersionString"])), name.ends_with(" Pro"), name)
        });
    let Some(iphone) = iphone else {
        fail!("the iOS runtime lists no iPhone", "no_runtime");
    };
    let created = capture(
        "xcrun",
        &["simctl", "create", name, &py::str(&iphone["identifier"]), &py::str(&runtime["identifier"])],
    );
    let id = created.stdout.trim().to_string();
    if created.code != 0 || id.is_empty() {
        fail!(format!("simctl create failed: {}", created.stderr.trim()), "command_failed");
    }
    say!(
        "made the base simulator {name} ({}, iOS {}), booting it once",
        py::str(&iphone["name"]),
        py::str(&runtime["version"])
    );
    boot_ios(&id)?;
    capture("xcrun", &["simctl", "shutdown", &id]);
    Ok((id, "Shutdown".into()))
}

fn version_key(version: &str) -> Vec<u32> {
    version.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

fn boot_ios(id: &str) -> Result<()> {
    let booted = capture("xcrun", &["simctl", "boot", id]);
    if booted.code != 0 && !booted.stderr.contains("current state: Booted") {
        fail!(format!("simctl boot failed: {}", booted.stderr.trim()), "boot_failed");
    }
    // bootstatus -b waits until the device finished booting, not only until launchd runs.
    let ready = capture("xcrun", &["simctl", "bootstatus", id, "-b"]);
    if ready.code != 0 {
        fail!(format!("simulator did not finish booting: {}", ready.stderr.trim()), "boot_failed");
    }
    Ok(())
}

// Android

fn sdk_tool(relative: &str) -> Result<String> {
    for variable in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(sdk) = std::env::var(variable) {
            let tool = Path::new(&sdk).join(relative);
            if tool.exists() {
                return Ok(tool.display().to_string());
            }
        }
    }
    fail!(format!("{relative} not found"), "sdk_missing", "set ANDROID_HOME to the Android SDK")
}

fn avd_exists(name: &str) -> Result<bool> {
    let listed = capture(&sdk_tool("emulator/emulator")?, &["-list-avds"]);
    Ok(listed.stdout.lines().any(|line| line.trim() == name))
}

fn up_android(base_name: Option<&str>) -> Result<Value> {
    let base_name = base_name.unwrap_or(ANDROID_BASE);
    if !avd_exists(base_name)? {
        make_android_base(base_name)?;
    }
    let port = free_emulator_port()?;
    let serial = format!("emulator-{port}");
    start_emulator(base_name, port, true)?;
    if let Err(reason) = wait_android_boot(&serial) {
        capture(&adb_path()?, &["-s", &serial, "emu", "kill"]);
        return Err(reason);
    }
    Ok(json!({"platform": "android", "device": serial, "name": base_name, "base": base_name}))
}

/// The lowest even console port in ddp's range that no emulator holds and nothing listens on (port and port + 1).
fn free_emulator_port() -> Result<u16> {
    let attached = capture(&adb_path()?, &["devices"]).stdout;
    let free = ANDROID_PORTS.step_by(2).find(|port| {
        !attached.contains(&format!("emulator-{port}"))
            && TcpListener::bind(("127.0.0.1", *port)).is_ok()
            && TcpListener::bind(("127.0.0.1", port + 1)).is_ok()
    });
    match free {
        Some(port) => Ok(port),
        None => fail!("no free emulator port between 5560 and 5680", "no_port", "ddp down --all"),
    }
}

fn start_emulator(avd: &str, port: u16, read_only: bool) -> Result<()> {
    let mut command = Command::new(sdk_tool("emulator/emulator")?);
    command.args(["-avd", avd, "-port", &port.to_string(), "-no-window", "-no-audio", "-no-boot-anim"]);
    if read_only {
        command.arg("-read-only");
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command.spawn().map_err(|e| failure(format!("could not start the emulator: {e}"), "boot_failed", None))?;
    Ok(())
}

fn wait_android_boot(serial: &str) -> Result<()> {
    let adb = adb_path()?;
    let deadline = Instant::now() + BOOT_PATIENCE;
    while Instant::now() < deadline {
        let booted = capture(&adb, &["-s", serial, "shell", "getprop", "sys.boot_completed"]);
        if booted.stdout.trim() == "1" {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    fail!(format!("{serial} did not finish booting in 180s"), "boot_failed", format!("adb -s {serial} logcat -d"))
}

/// An AVD on the newest installed google_apis image for this Mac's CPU, cold-booted once and closed so the
/// emulator saves its quickboot snapshot, which read-only runs then start from.
fn make_android_base(name: &str) -> Result<()> {
    let sdk = std::env::var("ANDROID_HOME").or_else(|_| std::env::var("ANDROID_SDK_ROOT")).map_err(|_| {
        failure("ANDROID_HOME is not set", "sdk_missing", Some("export ANDROID_HOME=<Android SDK folder>"))
    })?;
    let abi = if std::env::consts::ARCH == "aarch64" { "arm64-v8a" } else { "x86_64" };
    let images = Path::new(&sdk).join("system-images");
    let newest = std::fs::read_dir(&images)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|api| {
            let level: u32 = api.file_name().to_string_lossy().strip_prefix("android-")?.parse().ok()?;
            ["google_apis", "google_apis_playstore"]
                .into_iter()
                .find(|tag| api.path().join(tag).join(abi).exists())
                .map(|tag| (level, tag))
        })
        .max();
    let Some((level, tag)) = newest else {
        fail!(
            format!("no google_apis {abi} system image installed"),
            "no_image",
            format!("sdkmanager \"system-images;android-35;google_apis;{abi}\"")
        );
    };
    let package = format!("system-images;android-{level};{tag};{abi}");
    let mut create = Command::new(sdk_tool("cmdline-tools/latest/bin/avdmanager")?);
    create.args(["create", "avd", "-n", name, "-k", &package, "-d", "pixel_7"]).stdin(Stdio::piped());
    let mut child = create
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| failure(e.to_string(), "command_failed", None))?;
    // avdmanager asks whether to make a custom hardware profile.
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"no\n");
    }
    let made = child.wait_with_output().map_err(|e| failure(e.to_string(), "command_failed", None))?;
    if !made.status.success() {
        fail!(format!("avdmanager failed: {}", String::from_utf8_lossy(&made.stderr).trim()), "command_failed");
    }
    say!("made the base AVD {name} ({package}), booting it once for its snapshot");
    let port = free_emulator_port()?;
    let serial = format!("emulator-{port}");
    start_emulator(name, port, false)?;
    wait_android_boot(&serial)?;
    capture(&adb_path()?, &["-s", &serial, "emu", "kill"]);
    // The emulator writes the snapshot while it exits: wait until it has let go of its console port.
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && TcpListener::bind(("127.0.0.1", port)).is_err() {
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

// Apps

fn install(platform: &str, device: &str, build: &str) -> Result<()> {
    let started = Instant::now();
    let installed = if platform == "ios" {
        capture("xcrun", &["simctl", "install", device, build])
    } else {
        capture(&adb_path()?, &["-s", device, "install", "-r", build])
    };
    if installed.code != 0 || installed.stdout.contains("Failure") {
        fail!(format!("install failed: {}{}", installed.stderr.trim(), installed.stdout.trim()), "install_failed");
    }
    say!("installed {build} in {}s", seconds(started));
    Ok(())
}

fn start_app(platform: &str, device: &str, app: &str) -> Result<()> {
    let started = if platform == "ios" {
        capture("xcrun", &["simctl", "launch", device, app])
    } else {
        let component = live::launcher_activity(device, app)?;
        capture(&adb_path()?, &["-s", device, "shell", "am", "start", "-W", "-n", &component])
    };
    if started.code != 0 || started.stdout.contains("Error") {
        fail!(format!("could not start {app}: {}{}", started.stderr.trim(), started.stdout.trim()), "launch_failed");
    }
    Ok(())
}
