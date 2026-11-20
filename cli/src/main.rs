mod args;
mod checks;
mod compare;
mod control;
mod crash;
mod device;
mod history;
mod hit;
mod inspect;
mod install;
mod lease;
mod live;
mod memory;
mod model;
mod process;
mod py;
mod queries;
mod runner;
#[cfg(target_os = "macos")]
mod simulator;
mod state;
mod summary;
mod swift_source;
mod tools;
mod up;
mod walk;

use args::Args;
use serde_json::json;
use state::Result;

/// Exit 1 when the check found problems, so every check works as a gate.
/// With --json: {"check", "ok", "lines"}, the lines the check would print.
fn run_check(args: &mut Args) -> Result<i32> {
    let json_mode = state::ctx().json;
    if json_mode {
        state::push_capture(false);
    }
    let result = checks::run_named(args);
    let captured = if json_mode { state::pop_capture() } else { String::new() };
    let code = if result.as_ref().is_ok_and(|found| *found > 0) { 1 } else { 0 };
    result?;
    if json_mode {
        let lines: Vec<&str> = captured.lines().filter(|l| !l.trim().is_empty()).collect();
        say!("{}", py::dumps(&json!({"check": args.what, "ok": code == 0, "lines": lines})));
    }
    Ok(code)
}

fn handler(command: &str) -> fn(&mut Args) -> Result<i32> {
    match command {
        "skills" => tools::skills,
        "doctor" => tools::doctor,
        "batch" => tools::batch,
        "connect" => live::connect,
        "launch" => live::launch,
        "disconnect" => live::disconnect,
        "up" => up::up,
        "down" => up::down,
        "record" => device::record,
        "watch" => live::watch,
        "eval" => live::evaluate,
        "net" => live::net,
        "time" => live::time_control,
        "read" => summary::read,
        "check" => run_check,
        "tap" => hit::tap,
        "type" => device::type_text,
        "press" => device::press,
        "permission" => device::permission,
        "devices" => up::devices,
        "lease" => lease::lease,
        "release" => lease::release,
        "clean" => memory::clean,
        "version" => tools::version,
        "scroll" => device::scroll,
        "prefs" => live::prefs,
        "run" => live::run_command,
        "state" => live::app_state,
        "storage" => live::storage,
        "events" => live::app_events,
        "perf" => live::perf,
        "media" => live::media,
        "animations" => live::animations,
        "source" => live::source,
        "recompositions" => live::recompositions,
        "swipe" | "drag" | "double-tap" | "long-press" => hit::gesture,
        "crash" => crash::crash,
        "wait" => device::wait,
        "logs" => device::logs,
        "history" => history::history,
        "set" => device::settings,
        "biometric" => device::biometric,
        "back" => device::back,
        "audits" => device::audits,
        "memory" => process::memory,
        "inspect" => history::inspect,
        "highlight" => hit::highlight,
        "profile" => process::profile,
        "debug" => process::debug,
        "background" => device::background,
        "push" => device::push,
        "job" => device::job,
        "stop" => device::stop,
        "clear" => device::clear,
        "test" => device::test,
        "annotate" => device::annotate,
        "screenshot" => device::screenshot,
        "compare" => compare::compare,
        "explore" => device::explore,
        "mcp" => tools::mcp,
        "view" => tools::view,
        "open" => device::open_url,
        "baseline" => checks::baseline,
        "tree" => queries::tree,
        "get" => queries::get,
        "find" => queries::find,
        "walk" => walk::walk,
        _ => queries::diff,
    }
}

/// One ddp command. Returns its exit code instead of exiting, so `batch` and `mcp` can run several.
pub fn run_command(argv: Vec<String>) -> i32 {
    let mut args = match args::parse(&argv) {
        Ok(args) => args,
        Err(error) => {
            let text = error.render().to_string();
            if error.use_stderr() {
                state::write_err(&text)
            } else {
                state::write_out(&text)
            }
            return error.exit_code();
        }
    };
    state::use_context(args.session.as_deref(), args.json);
    state::push_limit(args.max_output, args.content_boundaries);
    // An open walk keeps every action as a step, with the line the action printed.
    let walking = walk::records(&args.command);
    if walking {
        state::push_capture(false);
    }
    let result = (|| {
        if let Some(id) = &args.id {
            args.id_from_ref = id.starts_with("@e");
            args.id = Some(queries::resolve_ref(id)?);
        }
        refuse_leased_devices(&args)?;
        handler(&args.command.clone())(&mut args)
    })();
    if walking {
        let printed = state::pop_capture();
        state::write_out(&printed);
        if matches!(result, Ok(0)) {
            if let Err(failure) = walk::record(&argv, &printed) {
                state::report_failure(&failure);
            }
        }
    }
    state::pop_limit();
    match result {
        Ok(code) => code,
        Err(failure) => {
            // A command that failed on an emulator ddp made says so when the emulator itself is gone or hung:
            // that is the cause, whatever the command ran into.
            let lost = lost_device(&args, &failure);
            state::report_failure(lost.as_ref().unwrap_or(&failure));
            1
        }
    }
}

/// One owner per device: a command for a device another owner leased is refused before it does anything. Not the
/// commands that name no device, show or change leases, or only act on what this folder made (`up`, `down`); `mcp`
/// and `batch` are asked per command they run.
fn refuse_leased_devices(args: &args::Args) -> Result<()> {
    const WITHOUT_A_DEVICE: [&str; 11] =
        ["devices", "lease", "release", "clean", "version", "skills", "up", "down", "disconnect", "mcp", "batch"];
    if WITHOUT_A_DEVICE.contains(&args.command.as_str()) {
        return Ok(());
    }
    let named: Vec<String> = match (&args.on, &args.device) {
        (Some(on), _) if args.command == "test" => on.split(',').map(|device| device.trim().to_string()).collect(),
        (_, Some(device)) => vec![device.clone()],
        _ => {
            let session = state::read_json(&state::ctx().session_file);
            let device = session.and_then(|s| s["device"].as_str().map(String::from));
            device.or_else(|| std::env::var("DDP_DEVICE").ok()).into_iter().collect()
        }
    };
    named.iter().try_for_each(|device| lease::refuse_foreign(device))
}

/// The emulator the failed command was for, when `ddp up` made it and it died or hangs. Asked only after a
/// failure, and not of `up` and `down`, which report on the emulator themselves.
fn lost_device(args: &args::Args, failure: &state::Failure) -> Option<state::Failure> {
    if matches!(&args.command[..], "up" | "down" | "devices") || failure.code.starts_with("emulator_") {
        return None;
    }
    let session = state::read_json(&state::ctx().session_file);
    let device = args.device.clone().or_else(|| session.and_then(|s| s["device"].as_str().map(String::from)))?;
    up::lost_emulator(&device)
}

/// How often a command that is still running asks whether its emulator is.
const LOST_DEVICE_WATCH: std::time::Duration = std::time::Duration::from_secs(15);

/// A command waiting on an emulator ddp made that died or froze would wait forever: adb does not give up by itself.
/// While the command runs, its emulator is asked now and then, and the command ends with what became of it. Not for
/// `mcp` and `batch`, which run many commands in one process, nor for the commands that watch the emulator themselves.
fn watch_for_lost_device(argv: &[String]) {
    let command = argv.first().map(String::as_str).unwrap_or_default();
    if matches!(command, "mcp" | "batch" | "up" | "down" | "devices" | "test" | "doctor" | "skills") {
        return;
    }
    let named = argv.iter().position(|word| word == "--device").and_then(|at| argv.get(at + 1)).cloned();
    std::thread::spawn(move || loop {
        std::thread::sleep(LOST_DEVICE_WATCH);
        let session = state::read_json(&state::ctx().session_file);
        let device = named.clone().or_else(|| session.and_then(|s| s["device"].as_str().map(String::from)));
        if let Some(lost) = device.and_then(|device| up::lost_emulator(&device)) {
            state::report_failure(&lost);
            std::process::exit(1);
        }
    });
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    watch_for_lost_device(&argv);
    std::process::exit(run_command(argv));
}
