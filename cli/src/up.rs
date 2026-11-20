//! `ddp up` and `ddp down`: a simulator or emulator of ddp's own for a run, made from a warm base and removed after.
//!
//! iOS clones a base simulator that has booted once (`simctl clone`), so the clone skips first-boot work.
//! Android starts the base AVD read-only from a snapshot, so several runs share one AVD and leave nothing on disk:
//! the base's quickboot snapshot, or on the graphics card a snapshot of its own per renderer, made on first use.
//! Only devices listed in `.ddp/devices.json` are ever shut down or deleted by `ddp down`.
use crate::args::Args;
use crate::live;
use crate::py;
use crate::state::{adb_path, capture, ctx, failure, now_ms, read_json, write_file, Failure, Result};
use crate::{fail, say};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    let gpu = args.gpu.as_deref().map(Gpu::named);
    if gpu.is_some() && platform == "ios" {
        fail!(
            "--gpu is for the Android emulator: a simulator draws with the Mac's own",
            "bad_option",
            "ddp up --platform ios"
        );
    }
    let mut device = if platform == "ios" {
        up_ios(base.as_deref())?
    } else {
        up_android(base.as_deref(), gpu, args.cold, args.min_free.as_deref())?
    };
    device["boot_s"] = json!(py::round_f(started.elapsed().as_secs_f64(), 1));
    // How it was asked for, so a device that died later can be named with the command that brings it back.
    for (key, value) in [("gpu", &args.gpu), ("install", &args.path), ("app", &args.app)] {
        if let Some(value) = value {
            device[key] = json!(value);
        }
    }
    let mut owned = owned_devices();
    owned.push(device.clone());
    save_owned(&owned)?;
    let id = py::str(&device["device"]);
    let draws = device["renderer"].as_str().map(|renderer| format!("; {}", drawn_with(renderer))).unwrap_or_default();
    let how = match device["boot"].as_str() {
        Some("cold") => " (cold boot)".to_string(),
        Some(snapshot) => format!(" (from the snapshot {snapshot})"),
        None => String::new(),
    };
    say!("up {platform} {id} in {}s{how}{draws}", seconds(started));
    if platform == "android" {
        // The emulator is this owner's until `down`. The port was free, so no lease that stands can be in the way.
        if let Err(no_lease) = crate::lease::take(&id, &device) {
            crate::warn!("ddp: {id} runs without a lease: {}", no_lease.message);
        }
    }

    if let Some(build) = args.path.clone() {
        install(&platform, &id, &build)?;
    }
    if let Some(app) = args.app.clone() {
        start_app(&platform, &id, &app)?;
        args.device = Some(id.clone());
        args.platform = Some(platform.clone());
        if let Some((session, snapshot)) = live::session_after_start(&platform, &id, &app, ENDPOINT_PATIENCE)? {
            return live::keep_session(&session, &snapshot);
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
            let next = match (owned.first(), session_holding_devices()) {
                (Some(device), _) => format!("ddp down {}", py::str(&device["device"])),
                (None, Some(Some(name))) => format!("ddp down --session {name}"),
                (None, Some(None)) => "ddp down".to_string(),
                // ddp keeps what up made per folder: `up` in another folder left nothing here to read.
                (None, None) => "run ddp down in the folder where ddp up ran".to_string(),
            };
            fail!(format!("no device of ddp's own to shut down in {}", ctx().dir.display()), "not_owned", next);
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
            capture_within(&adb_path()?, &["-s", &id, "emu", "kill"], ADB_PATIENCE);
            crate::lease::drop_lease(&id);
            if end_emulator_process(device) {
                say!("down android {id}: it did not stop when asked, so its process was ended");
                continue;
            }
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

/// Another session in this folder that has devices `up` made: Some(None) is the one without --session.
fn session_holding_devices() -> Option<Option<String>> {
    let holds = |dir: &Path| {
        read_json(&dir.join("devices.json")).and_then(|v| v.as_array().map(|a| !a.is_empty())) == Some(true)
    };
    let here = ctx().dir;
    if here != Path::new(".ddp") && holds(Path::new(".ddp")) {
        return Some(None);
    }
    let sessions = std::fs::read_dir(".ddp/sessions").ok()?;
    sessions
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| *dir != here && holds(dir))
        .find_map(|dir| dir.file_name().map(|name| Some(name.to_string_lossy().into_owned())))
}

fn seconds(since: Instant) -> String {
    format!("{:.1}", since.elapsed().as_secs_f64())
}

// iOS

fn up_ios(base_name: Option<&str>) -> Result<Value> {
    if !cfg!(target_os = "macos") {
        fail!(
            "iOS simulators run on a Mac only: ddp up --platform ios has to run there",
            "not_supported",
            "ddp up --platform android here, or run ddp on the Mac that has Xcode"
        );
    }
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

/// What `ddp up --gpu` asks the emulator to draw with.
#[derive(Clone, Copy, PartialEq)]
enum Gpu {
    /// The machine's graphics card through OpenGL (`-gpu host`).
    Host,
    /// The machine's graphics card through Vulkan, the guest's OpenGL on ANGLE (`-gpu host -feature GuestAngle,Vulkan`).
    Vulkan,
    /// The emulator's own renderer on the CPU (`-gpu swiftshader_indirect`).
    Software,
}

impl Gpu {
    fn named(name: &str) -> Gpu {
        match name {
            "host" => Gpu::Host,
            "vulkan" => Gpu::Vulkan,
            _ => Gpu::Software,
        }
    }

    /// The emulator's arguments for it. With Vulkan the emulator saves and loads a snapshot only when asked to with
    /// VulkanSnapshots (`avd snapshot save` answers `UNSUPPORTED_VK_APP` without it); a cold boot leaves that out.
    fn emulator_arguments(self, with_snapshot: bool) -> &'static [&'static str] {
        match (self, with_snapshot) {
            (Gpu::Host, _) => &["-gpu", "host"],
            (Gpu::Vulkan, true) => &["-gpu", "host", "-feature", "GuestAngle,Vulkan,VulkanSnapshots"],
            (Gpu::Vulkan, false) => &["-gpu", "host", "-feature", "GuestAngle,Vulkan"],
            (Gpu::Software, _) => &["-gpu", "swiftshader_indirect"],
        }
    }

    /// The snapshot of the base that holds this renderer. The base's own (quickboot) snapshot holds the renderer the
    /// emulator chose by itself, so a run on the card cannot start from that one.
    fn snapshot(self) -> Option<&'static str> {
        match self {
            Gpu::Host => Some("ddp_host"),
            Gpu::Vulkan => Some("ddp_vulkan"),
            Gpu::Software => None,
        }
    }

    fn on_the_card(self) -> bool {
        self != Gpu::Software
    }
}

/// Renderers that draw on the CPU, by the names they give themselves.
const SOFTWARE_RENDERERS: [&str; 4] = ["SwiftShader", "llvmpipe", "softpipe", "Software Rasterizer"];

fn draws_in_software(renderer: &str) -> bool {
    SOFTWARE_RENDERERS.iter().any(|name| renderer.contains(name))
}

/// "draws with <renderer>", marked when it is a software renderer, so a silent fallback from the graphics card shows.
pub fn drawn_with(renderer: &str) -> String {
    let software = if draws_in_software(renderer) { " (software, not the graphics card)" } else { "" };
    format!("draws with {renderer}{software}")
}

/// What the device composes its screen with, as SurfaceFlinger names it (`GLES: vendor, renderer, version`). An
/// emulator's renderer is a translator around the one that draws: `Android Emulator OpenGL ES Translator (<that one>)`.
/// None when SurfaceFlinger prints no such line (a Vulkan render engine): unknown, never a guess.
pub fn android_renderer(adb: &str, device: &str) -> Option<String> {
    let dump = capture_within(adb, &["-s", device, "shell", "dumpsys", "SurfaceFlinger"], ADB_PATIENCE)?.stdout;
    renderer_in(&dump)
}

fn renderer_in(surface_flinger: &str) -> Option<String> {
    const TRANSLATOR: &str = "Android Emulator OpenGL ES Translator (";
    let line = surface_flinger.lines().find_map(|line| line.strip_prefix("GLES: "))?;
    let renderer = line.split(", ").nth(1)?.trim();
    let drawn_by = renderer.strip_prefix(TRANSLATOR).and_then(|inner| inner.strip_suffix(')')).unwrap_or(renderer);
    (!drawn_by.is_empty()).then(|| drawn_by.to_string())
}

fn up_android(base_name: Option<&str>, gpu: Option<Gpu>, cold: bool, min_free: Option<&str>) -> Result<Value> {
    let base_name = base_name.unwrap_or(ANDROID_BASE);
    let display = if gpu == Some(Gpu::Host) { host_display()? } else { None };
    crate::memory::guard(base_name, min_free)?;
    if !avd_exists(base_name)? {
        make_android_base(base_name)?;
    }
    let on_the_card = gpu.filter(|gpu| gpu.on_the_card());
    let mut snapshot = on_the_card.and_then(Gpu::snapshot).filter(|_| !cold);
    if let (Some(gpu), Some(name)) = (on_the_card, snapshot) {
        if !has_snapshot(base_name, name) {
            // Without its snapshot the run still starts, cold as before.
            if let Err(not_made) = make_snapshot(base_name, gpu, name, display.as_ref()) {
                crate::warn!("ddp: no snapshot {name} of {base_name}, so a cold boot: {}", not_made.message);
                snapshot = None;
            }
        }
    }
    let port = free_emulator_port()?;
    let serial = format!("emulator-{port}");
    let mut arguments = vec!["-read-only"];
    arguments.extend(gpu.map(|gpu| gpu.emulator_arguments(snapshot.is_some())).unwrap_or_default());
    match snapshot {
        Some(name) => arguments.extend(["-snapshot", name, "-no-snapshot-save"]),
        // The base's own snapshot holds the renderer it was saved with: on the card without one, the boot is cold.
        None if cold || on_the_card.is_some() => arguments.push("-no-snapshot"),
        None => {}
    }
    let mut emulator = start_emulator(base_name, port, &arguments, gpu, display.as_ref())?;
    let adb = adb_path()?;
    wait_android_boot(&serial, &mut emulator)?;
    let said = std::fs::read_to_string(&emulator.log_file).unwrap_or_default();
    let loaded = loaded_snapshot(&said);
    if let Some(name) = snapshot.filter(|name| loaded.as_deref() != Some(name)) {
        // The emulator turns a snapshot down when it or the driver changed since. Removed, so the next up makes it again.
        if let Some(folder) = avd_folder(base_name) {
            let _ = std::fs::remove_dir_all(folder.join("snapshots").join(name));
        }
        crate::warn!("ddp: the emulator did not load the snapshot {name}, so this was a cold boot; the next up makes the snapshot again");
    }
    let renderer = android_renderer(&adb, &serial).or_else(|| vulkan_device(&said));
    // The emulator falls back to software without a word when it cannot reach the card.
    if let (Some(_), Some(renderer)) = (on_the_card, renderer.as_deref().filter(|r| draws_in_software(r))) {
        capture(&adb, &["-s", &serial, "emu", "kill"]);
        fail!(
            format!("asked for the graphics card, but {serial} {}; it is shut down again", drawn_with(renderer)),
            "gpu_fallback",
            "ddp up --platform android --gpu software, or start it from a session with a display and a working driver"
        );
    }
    let mut device = json!({"platform": "android", "device": serial, "name": base_name, "base": base_name,
        "pid": emulator.process.id(), "log": emulator.log_file.display().to_string(),
        "boot": loaded.unwrap_or_else(|| "cold".into())});
    if let Some(renderer) = renderer {
        device["renderer"] = json!(renderer);
    }
    Ok(device)
}

/// The X display an emulator on the graphics card draws through, when this process has none: the emulator needs
/// one even with no window. On Linux only; elsewhere the emulator reaches the card by itself.
struct Display {
    name: String,
    authority: Option<PathBuf>,
}

fn host_display() -> Result<Option<Display>> {
    if !cfg!(target_os = "linux") || std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        return Ok(None);
    }
    let mut numbers: Vec<u32> = std::fs::read_dir("/tmp/.X11-unix")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|socket| socket.file_name().to_string_lossy().strip_prefix('X')?.parse().ok())
        .collect();
    numbers.sort_unstable();
    let Some(number) = numbers.first() else {
        fail!(
            "the graphics card needs a display: DISPLAY is not set and no X server runs here (/tmp/.X11-unix)",
            "no_display",
            "ddp up --platform android --gpu software"
        );
    };
    let authority = std::env::var_os("XAUTHORITY")
        .is_none()
        .then(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".Xauthority"));
    Ok(Some(Display { name: format!(":{number}"), authority: authority.filter(|file| file.exists()) }))
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

fn start_emulator(
    avd: &str,
    port: u16,
    arguments: &[&str],
    gpu: Option<Gpu>,
    display: Option<&Display>,
) -> Result<Emulator> {
    let mut command = Command::new(sdk_tool("emulator/emulator")?);
    command.args(["-avd", avd, "-port", &port.to_string(), "-no-window", "-no-audio", "-no-boot-anim"]);
    command.args(arguments);
    // Vulkan needs no display, and it is started the way it was measured to hold: without one.
    if gpu == Some(Gpu::Vulkan) && cfg!(target_os = "linux") {
        command.env_remove("DISPLAY");
    }
    if let Some(display) = display {
        command.env("DISPLAY", &display.name);
        if let Some(authority) = &display.authority {
            command.env("XAUTHORITY", authority);
        }
    }
    // What the emulator says while it starts is the only word on why a boot failed.
    let log_file = emulator_log(port);
    let log = std::fs::create_dir_all(ctx().dir).and_then(|_| std::fs::File::create(&log_file));
    let (out, err) = match log.and_then(|file| Ok((file.try_clone()?, file))) {
        Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
        Err(_) => (Stdio::null(), Stdio::null()),
    };
    command.stdin(Stdio::null()).stdout(out).stderr(err);
    let process =
        command.spawn().map_err(|e| failure(format!("could not start the emulator: {e}"), "boot_failed", None))?;
    Ok(Emulator { process, log_file })
}

/// What the emulator writes when it gives up on drawing; it does not end by itself then.
const NO_RENDERER: &str = "Could not start renderer";

/// The emulator process `up` started and the file its own output goes to.
struct Emulator {
    process: Child,
    log_file: PathBuf,
}

fn emulator_log(port: u16) -> PathBuf {
    ctx().dir.join(format!("emulator-{port}.log"))
}

/// The last lines the emulator wrote, for a boot that failed.
fn last_words(log_file: &Path) -> String {
    const LINES: usize = 5;
    let text = std::fs::read_to_string(log_file).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|line| !line.is_empty()).collect();
    lines[lines.len().saturating_sub(LINES)..].iter().map(|line| format!("\n  {line}")).collect()
}

/// Waits until Android says it booted. An emulator that ended, or that did not boot in time, is not left behind:
/// one that never reached adb cannot be told to stop through it, so its process is ended here.
fn wait_android_boot(serial: &str, emulator: &mut Emulator) -> Result<()> {
    let adb = adb_path()?;
    let deadline = Instant::now() + BOOT_PATIENCE;
    let log = emulator.log_file.display().to_string();
    while Instant::now() < deadline {
        let booted = capture(&adb, &["-s", serial, "shell", "getprop", "sys.boot_completed"]);
        if booted.stdout.trim() == "1" {
            return Ok(());
        }
        if let Ok(Some(status)) = emulator.process.try_wait() {
            let how = status.code().map(|code| format!("exit {code}")).unwrap_or_else(|| "ended by a signal".into());
            fail!(
                format!(
                    "the emulator ended before {serial} booted ({how}); it said:{}",
                    last_words(&emulator.log_file)
                ),
                "emulator_died",
                format!("cat {log}")
            );
        }
        // Without its renderer the emulator stays up and never boots: no reason to wait the 180 s out.
        if std::fs::read_to_string(&emulator.log_file).is_ok_and(|said| said.contains(NO_RENDERER)) {
            let _ = emulator.process.kill();
            let _ = emulator.process.wait();
            fail!(
                format!(
                    "the emulator could not start its renderer, so {serial} is stopped. It said:{}",
                    last_words(&emulator.log_file)
                ),
                "no_renderer",
                "ddp up --platform android --gpu software, or check that DISPLAY names a display this user may use"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let _ = emulator.process.kill();
    let _ = emulator.process.wait();
    fail!(
        format!(
            "{serial} did not finish booting in {}s; the emulator is stopped. It said:{}",
            BOOT_PATIENCE.as_secs(),
            last_words(&emulator.log_file)
        ),
        "boot_failed",
        format!("cat {log}")
    )
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
    let mut emulator = start_emulator(name, port, &[], None, None)?;
    wait_android_boot(&serial, &mut emulator)?;
    capture(&adb_path()?, &["-s", &serial, "emu", "kill"]);
    // The emulator writes the snapshot while it exits.
    wait_until_port_is_free(port);
    Ok(())
}

/// Waits until an emulator told to stop has let go of its console port.
fn wait_until_port_is_free(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && TcpListener::bind(("127.0.0.1", port)).is_err() {
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The folder of an AVD: its config and its snapshots.
pub fn avd_folder(name: &str) -> Option<PathBuf> {
    let avds = std::env::var_os("ANDROID_AVD_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".android").join("avd")))?;
    Some(avds.join(format!("{name}.avd")))
}

fn has_snapshot(base: &str, name: &str) -> bool {
    avd_folder(base).is_some_and(|folder| folder.join("snapshots").join(name).join("ram.bin").exists())
}

/// How long the base runs after its boot before the snapshot is taken, so the launcher's first work is in it.
const SETTLE_BEFORE_SNAPSHOT: Duration = Duration::from_secs(3);

/// Boots the base once on this renderer and saves that state under the renderer's snapshot name, next to the base's
/// own. Nothing is saved on exit, so the base's own snapshot stays as it is.
fn make_snapshot(base: &str, gpu: Gpu, name: &str, display: Option<&Display>) -> Result<()> {
    let started = Instant::now();
    let port = free_emulator_port()?;
    let serial = format!("emulator-{port}");
    let mut arguments = gpu.emulator_arguments(true).to_vec();
    arguments.extend(["-no-snapshot-load", "-no-snapshot-save"]);
    let mut emulator = start_emulator(base, port, &arguments, Some(gpu), display)?;
    wait_android_boot(&serial, &mut emulator)?;
    std::thread::sleep(SETTLE_BEFORE_SNAPSHOT);
    let adb = adb_path()?;
    let saved = capture(&adb, &["-s", &serial, "emu", "avd", "snapshot", "save", name]);
    capture(&adb, &["-s", &serial, "emu", "kill"]);
    wait_until_port_is_free(port);
    if !saved.stdout.lines().any(|line| line.trim() == "OK") {
        let answer = format!("{}{}", saved.stdout.trim(), saved.stderr.trim());
        fail!(format!("the emulator did not save it ({answer})"), "snapshot_failed");
    }
    say!(
        "made the snapshot {name} of {base} in {}s, once: the next boots on this renderer start from it",
        seconds(started)
    );
    Ok(())
}

/// The snapshot the emulator says it started from. None is a cold boot.
fn loaded_snapshot(emulator_said: &str) -> Option<String> {
    let name = emulator_said.lines().find_map(|line| line.split("Successfully loaded snapshot '").nth(1))?;
    Some(name.split('\'').next()?.to_string())
}

/// The card the emulator says it draws on through Vulkan, where SurfaceFlinger names no OpenGL renderer.
fn vulkan_device(emulator_said: &str) -> Option<String> {
    let device = emulator_said.lines().find_map(|line| line.split("Selecting Vulkan device: ").nth(1))?;
    Some(format!("{} (Vulkan)", device.split(", Version").next()?.trim()))
}

// An emulator that died or hangs

/// How long adb gets to answer before the device counts as not answering.
const ADB_PATIENCE: Duration = Duration::from_secs(5);
/// How long an emulator told to stop gets before its process is ended.
const STOP_PATIENCE: Duration = Duration::from_secs(5);

/// `capture` that gives up: None when the program did not end in time (it is ended) or did not start.
pub fn capture_within(program: &str, args: &[&str], patience: Duration) -> Option<crate::state::Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + patience;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    Some(crate::state::Output {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Whether the emulator process `up` started still runs: its pid, with its own port on the command line, so a pid
/// the system gave to another process later does not count. None where there is no `ps` to ask (Windows) or the
/// device entry has no pid (made by an older ddp).
fn emulator_runs(device: &Value) -> Option<bool> {
    let pid = device["pid"].as_u64()?;
    let port = py::str(&device["device"]).strip_prefix("emulator-")?.to_string();
    if cfg!(windows) {
        return None;
    }
    let listed = capture("ps", &["-p", &pid.to_string(), "-o", "args="]);
    let zombie = capture("ps", &["-p", &pid.to_string(), "-o", "stat="]).stdout.trim_start().starts_with('Z');
    Some(listed.stdout.contains(&format!("-port {port}")) && !zombie)
}

/// The images an emulator holds open in the system's temp folder (`android-<user>/`). It removes them when it ends by
/// itself; one ended by the kill signal leaves them, and on a temp folder kept in memory they take that memory.
fn held_temp_images(pid: &str) -> Vec<PathBuf> {
    let temp = std::env::temp_dir();
    let its_own = |file: &PathBuf| {
        let inside = file.strip_prefix(&temp).ok().and_then(|rest| rest.components().next());
        inside.is_some_and(|folder| folder.as_os_str().to_string_lossy().starts_with("android-")) && file.is_file()
    };
    let open = std::fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten();
    open.filter_map(|descriptor| std::fs::read_link(descriptor.path()).ok()).filter(its_own).collect()
}

/// Ends the emulator's process when it still runs after it was told to stop. True when it had to.
fn end_emulator_process(device: &Value) -> bool {
    let deadline = Instant::now() + STOP_PATIENCE;
    while emulator_runs(device) == Some(true) {
        if Instant::now() >= deadline {
            // A hung emulator takes no request to stop: only the kill signal ends it. Nothing is lost, it is read-only.
            let pid = py::str(&device["pid"]);
            let left_behind = held_temp_images(&pid);
            capture("kill", &["-9", &pid]);
            for image in left_behind {
                let _ = std::fs::remove_file(image);
            }
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// The `ddp up` line that makes the same device again.
fn up_again(device: &Value) -> String {
    let mut command = "ddp up --platform android".to_string();
    for (option, key) in [("--gpu", "gpu"), ("--install", "install"), ("--app", "app")] {
        if let Some(value) = device[key].as_str() {
            command.push_str(&format!(" {option} {value}"));
        }
    }
    command
}

/// What became of an emulator `ddp up` made in this folder, when it is gone or does not answer: the failure to
/// report instead of the one a command ran into. None when it answers, or when it is not ddp's emulator: then the
/// command's own failure stands.
pub fn lost_emulator(id: &str) -> Option<Failure> {
    let device = owned_devices().into_iter().find(|d| d["device"] == id && d["platform"] == "android")?;
    let pid = py::str(&device["pid"]);
    let said = device["log"].as_str().map(|log| last_words(Path::new(log))).unwrap_or_default();
    let said = if said.is_empty() { String::new() } else { format!(" The emulator's last lines:{said}") };
    let next = format!("ddp down {id}, then {}", up_again(&device));
    if !emulator_runs(&device)? {
        return Some(failure(
            format!("{id} died: the emulator's process (pid {pid}) is gone.{said}"),
            "emulator_died",
            Some(&next),
        ));
    }
    let adb = adb_path().ok()?;
    let answered = capture_within(&adb, &["-s", id, "shell", "echo", "ok"], ADB_PATIENCE);
    if answered.as_ref().is_some_and(|answer| answer.stdout.trim() == "ok") {
        return None;
    }
    let adb_said = match &answered {
        None => format!("adb got no answer in {}s", ADB_PATIENCE.as_secs()),
        Some(answer) => format!("adb says: {}", answer.stderr.lines().next().unwrap_or("nothing").trim()),
    };
    Some(failure(
        format!("{id} is hung: the emulator's process runs (pid {pid}) but {adb_said}.{said}"),
        "emulator_hung",
        Some(&next),
    ))
}

// Apps

fn install(platform: &str, device: &str, build: &str) -> Result<()> {
    let started = Instant::now();
    if platform != "ios" {
        return crate::install::apk(device, build).map(|_| ());
    }
    let installed = capture("xcrun", &["simctl", "install", device, build]);
    if installed.code != 0 || installed.stdout.contains("Failure") {
        fail!(format!("install failed: {}{}", installed.stderr.trim(), installed.stdout.trim()), "install_failed");
    }
    say!("installed {build} in {}s", seconds(started));
    Ok(())
}

/// Starts the app, or brings it to the front if it runs: never a restart.
pub fn start_app(platform: &str, device: &str, app: &str) -> Result<()> {
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

/// `ddp devices`: the booted simulators and the adb devices there are to drive, each with what this folder knows of
/// it: that `ddp up` made it, and the sessions that point at it.
pub fn devices(args: &mut Args) -> Result<i32> {
    let mut found: Vec<(String, String, String)> = Vec::new();
    if cfg!(target_os = "macos") && crate::state::which("xcrun").is_some() {
        let listing = capture("xcrun", &["simctl", "list", "devices", "booted", "-j"]).stdout;
        let listing: Value = serde_json::from_str(&listing).unwrap_or(json!({}));
        for group in listing["devices"].as_object().into_iter().flat_map(|groups| groups.values()) {
            for simulator in group.as_array().into_iter().flatten() {
                found.push(("ios".into(), py::str(&simulator["udid"]), py::str(&simulator["name"])));
            }
        }
    }
    let adb = adb_path().ok();
    if let Some(adb) = &adb {
        for line in capture(adb, &["devices"]).stdout.lines().skip(1) {
            if let Some((serial, state)) = line.split_once('\t') {
                found.push(("android".into(), serial.into(), state.into()));
            }
        }
    }
    // An emulator ddp up made here that adb no longer lists: say what became of it, not nothing.
    for device in owned_devices().iter().filter(|d| d["platform"] == "android") {
        let id = py::str(&device["device"]);
        if found.iter().any(|(_, listed, _)| *listed == id) {
            continue;
        }
        let what = match emulator_runs(device) {
            Some(false) => format!("died, its process is gone (ddp down {id} clears it)"),
            Some(true) => "its process runs but adb does not list it".to_string(),
            None => "not listed by adb".to_string(),
        };
        found.push(("android".into(), id, what));
    }
    // One adb still lists as a device but that does not answer: said once here, and not asked anything more below.
    let mut hung: Vec<String> = Vec::new();
    for (_, id, what) in found.iter_mut().filter(|(platform, _, state)| platform == "android" && state == "device") {
        if lost_emulator(id).is_some_and(|lost| lost.code == "emulator_hung") {
            *what = format!("hung, its process runs but adb gets no answer (ddp down {id} ends it)");
            hung.push(id.clone());
        }
    }
    // The next emulator on a port gets the same serial: a session saved before the device booted named the one before.
    // Asked only of a device a session names, so ddp devices does not reach into every device on the machine.
    let saved_since_boot = |file: &Path, id: &str| {
        let listed = found.iter().any(|(p, d, _)| p == "android" && d == id) && !hung.iter().any(|h| h == id);
        let Some(adb) = adb.as_deref().filter(|_| listed) else {
            return true;
        };
        let Some(uptime) = capture_within(adb, &["-s", id, "shell", "cat", "/proc/uptime"], ADB_PATIENCE) else {
            return true;
        };
        let uptime = uptime.stdout;
        let Some(up_s) = uptime.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()) else { return true };
        let saved = std::fs::metadata(file).and_then(|m| m.modified()).ok();
        let saved_ms =
            saved.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|t| t.as_secs_f64() * 1000.0);
        saved_ms.is_some_and(|saved_ms| saved_ms >= now_ms() - up_s * 1000.0)
    };
    let mut folders = vec![(None, PathBuf::from(".ddp"))];
    if let Ok(sessions) = std::fs::read_dir(".ddp/sessions") {
        folders.extend(
            sessions.flatten().map(|entry| (Some(entry.file_name().to_string_lossy().into_owned()), entry.path())),
        );
    }
    let made = |id: &str| {
        folders.iter().any(|(_, dir)| {
            read_json(&dir.join("devices.json"))
                .and_then(|v| v.as_array().cloned())
                .is_some_and(|owned| owned.iter().any(|d| d["device"] == id))
        })
    };
    let sessions = |id: &str| -> Vec<String> {
        folders
            .iter()
            .filter_map(|(name, dir)| {
                let file = dir.join("session.json");
                let session = read_json(&file).filter(|s| s["device"] == id && saved_since_boot(&file, id))?;
                let how = if session["mode"] == "device" { "without the library" } else { "library" };
                let app = session["app"].as_str().map(|app| format!(" {app}")).unwrap_or_default();
                Some(format!("session {}:{app} ({how})", name.as_deref().unwrap_or("default")))
            })
            .collect()
    };
    // Asked only of a device ddp up made or a session names, as above.
    let renderer = |platform: &str, id: &str, known: bool| {
        let ask = platform == "android" && known && !hung.iter().any(|h| h == id);
        adb.as_deref().filter(|_| ask).and_then(|adb| android_renderer(adb, id))
    };
    // Asked only of a device a lease file names.
    let leased_to = |platform: &str, id: &str| {
        let standing = platform == "android" && !hung.iter().any(|h| h == id);
        standing.then(|| crate::lease::standing(id)).flatten().map(|lease| py::str(&lease["owner"]))
    };
    if args.json {
        let rows: Vec<Value> = found
            .iter()
            .map(|(platform, id, what)| {
                let (up, sessions) = (made(id), sessions(id));
                let mut row = json!({"platform": platform, "device": id, "name": what, "up": up, "sessions": sessions});
                if let Some(owner) = leased_to(platform, id) {
                    row["lease"] = json!(owner);
                }
                if let Some(renderer) = renderer(platform, id, up || !sessions.is_empty()) {
                    row["renderer"] = json!(renderer);
                }
                row
            })
            .collect();
        say!("{}", py::dumps(&json!({"devices": rows})));
        return Ok(0);
    }
    for (platform, id, what) in &found {
        let mut notes = vec![what.clone()];
        let (up, sessions) = (made(id), sessions(id));
        if up {
            notes.push("ddp up made it".into());
        }
        if let Some(renderer) = renderer(platform, id, up || !sessions.is_empty()) {
            notes.push(drawn_with(&renderer));
        }
        if let Some(owner) = leased_to(platform, id) {
            let yours = if owner == crate::lease::owner() { " (you)" } else { "" };
            notes.push(format!("leased to {owner}{yours}"));
        }
        notes.extend(sessions);
        say!("{platform} {id}: {}", notes.join("; "));
    }
    let plural = if found.len() == 1 { "" } else { "s" };
    say!(
        "{} device{plural}; next: ddp connect <device> --app <id>, or any command with --device <device> without the library",
        found.len()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_what_the_emulator_translates_to() {
        let software =
            "------------RE GLES (Ganesh)------------\nGLES: Google (Google Inc.), Android Emulator OpenGL ES \
                        Translator (Google SwiftShader), OpenGL ES 3.0 (OpenGL ES 3.0 SwiftShader 4.0.0.1)\n";
        assert_eq!(renderer_in(software).as_deref(), Some("Google SwiftShader"));
        assert_eq!(drawn_with("Google SwiftShader"), "draws with Google SwiftShader (software, not the graphics card)");
    }

    #[test]
    fn keeps_a_phone_renderer_whole() {
        let phone = "GLES: Qualcomm, Adreno (TM) 610, OpenGL ES 3.2 V@0502.0\n";
        assert_eq!(renderer_in(phone).as_deref(), Some("Adreno (TM) 610"));
        assert_eq!(drawn_with("Adreno (TM) 610"), "draws with Adreno (TM) 610");
    }

    #[test]
    fn says_nothing_without_a_gles_line() {
        assert_eq!(renderer_in("RenderEngine: skiavk\n"), None);
    }
}
