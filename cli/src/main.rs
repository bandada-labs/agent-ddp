mod args;
mod checks;
mod compare;
mod control;
mod crash;
mod device;
mod history;
mod hit;
mod inspect;
mod live;
mod model;
mod py;
mod queries;
mod state;
mod summary;
mod tools;

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
        "prefs" => live::prefs,
        "run" => live::run_command,
        "state" => live::app_state,
        "events" => live::app_events,
        "crash" => crash::crash,
        "wait" => device::wait,
        "logs" => device::logs,
        "history" => history::history,
        "set" => device::settings,
        "annotate" => device::annotate,
        "compare" => compare::compare,
        "explore" => device::explore,
        "mcp" => tools::mcp,
        "view" => tools::view,
        "open" => device::open_url,
        "baseline" => checks::baseline,
        "tree" => queries::tree,
        "get" => queries::get,
        "find" => queries::find,
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
    let result = (|| {
        if let Some(id) = &args.id {
            args.id = Some(queries::resolve_ref(id)?);
        }
        handler(&args.command.clone())(&mut args)
    })();
    state::pop_limit();
    match result {
        Ok(code) => code,
        Err(failure) => {
            state::report_failure(&failure);
            1
        }
    }
}

fn main() {
    std::process::exit(run_command(std::env::args().skip(1).collect()));
}
