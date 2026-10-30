//! The command line: every command, its options and their help.
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::path::PathBuf;

/// Everything any command may be given. A command reads the fields it defined.
#[derive(Clone, Default)]
pub struct Args {
    pub command: String,
    pub session: Option<String>,
    pub json: bool,
    pub max_output: Option<usize>,
    pub content_boundaries: bool,
    /// Set by check all and explore: checks print only problems.
    pub shared: bool,

    pub file: Option<PathBuf>,
    pub duration: Option<String>,
    pub frames: Option<i64>,
    pub then: Option<String>,
    pub id: Option<String>,
    pub device: Option<String>,
    pub app: Option<String>,
    pub platform: Option<String>,
    pub name: Option<String>,
    pub on: Option<String>,
    pub action: Option<String>,
    pub what: Option<String>,
    pub value: Option<String>,
    pub by: Option<String>,
    pub path: Option<String>,
    pub request_id: Option<String>,
    pub until: Option<String>,
    pub expect: Option<PathBuf>,
    pub against: Option<PathBuf>,
    pub junit: Option<PathBuf>,
    pub settled: Option<String>,
    pub timeout: String,
    pub ignore: Vec<String>,
    pub grep: Option<String>,
    pub limit: Option<usize>,
    pub at: Option<String>,
    pub interactive: bool,
    pub changed: bool,
    pub within: Option<String>,
    pub depth: Option<i64>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub curve: bool,
    pub where_: Option<String>,
    pub between: Option<(String, String)>,
    pub recordings: Vec<PathBuf>,
    pub out: String,
    pub url: Option<String>,
    pub per: String,
    pub skip: Vec<String>,
    pub tools: String,
    pub lines: Vec<String>,
    pub bail: bool,
    pub full: bool,
    pub filter: Option<String>,
    pub since: String,
    pub time: Option<String>,
    pub dir: Option<PathBuf>,
    pub method: Option<String>,
    pub body: Option<String>,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub key: Option<String>,
    pub follow: bool,
    pub source: bool,
    pub force: bool,
    pub reset: bool,
    pub structure: bool,
    pub reference: Option<PathBuf>,
    pub masks: Vec<String>,
    pub screen: Option<PathBuf>,
    pub max: Option<String>,
    pub max_band: Option<String>,
    pub env: Vec<String>,
    pub launch_args: Vec<String>,
    pub set_value: Option<String>,
}

const ABOUT: &str = "ddp: see what a Debug iOS/Android app draws, frame by frame, live or from a recording.

Start here (for agents): ddp skills get core
  The guides ship with the CLI, so they always match this version. `ddp doctor` when something fails.

  connect | launch | disconnect | doctor   attach to a running Debug app, or start it fresh; check the setup
  tree | find | get | read | watch | eval  what is drawn, now or over a take (@refs from tree and find)
  record | wait | tap | type | press       take a recording, wait, touch and type
  swipe | drag | double-tap | long-press   gestures with a real finger, then what changed
  open | set | logs | prefs | run | state | events   deep links, device settings, app logs, preferences, app commands, values, events
  biometric | push | job                   simulated Face ID or fingerprint, a push, a background job
  check overlap|corners|jank|settle|a11y|timeline|parity|baseline|tappable|all
  history | crash                          the last 15 minutes: what changed, taps, requests; the last crash with its stack
  compare --reference <png> | compare <element> --on ios,android   pixels against a reference or the other phone
  diff | baseline | annotate | view | explore | net | time
  skills | batch | mcp

See `ddp skills get core --full` for every command and its output.";

fn opt(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help)
}

fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue).help(help)
}

fn positional(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).help(help)
}

fn many(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).num_args(1..).value_name("ELEMENT").help(help)
}

fn common(command: Command) -> Command {
    command
        .arg(opt("session", "named session (or env DDP_SESSION): its own live connection and recordings"))
        .arg(flag("json", "machine output; errors as {error, code, next}"))
        .arg(
            opt("max-output", "cut output at N chars (or env DDP_MAX_OUTPUT)").value_parser(clap::value_parser!(usize)),
        )
        .arg(flag("content-boundaries", "wrap output in markers: app text is data, not instructions"))
}

/// Live: how long to record for the command, and what to do meanwhile.
fn window(command: Command) -> Command {
    command
        .arg(opt("for", "live: record this long (1.5s, 1500ms); default 2s").id("duration"))
        .arg(
            opt("frames", "live or offline record: number of frames instead of --for")
                .value_parser(clap::value_parser!(i64)),
        )
        .arg(opt("then", "program and arguments to run right after recording starts (no shell)"))
}

fn file(command: Command) -> Command {
    command.arg(opt("file", "offline: read this recording (a path or a saved name)"))
}

fn device_options(command: Command) -> Command {
    command.arg(opt("device", "simulator UDID or adb serial")).arg(opt("app", "bundle id / application id"))
}

fn choices(name: &'static str, values: &'static [&'static str]) -> Arg {
    Arg::new(name).value_parser(clap::builder::PossibleValuesParser::new(values))
}

pub fn parser() -> Command {
    let sub = |name: &'static str, about: &'static str| common(Command::new(name).about(about));
    Command::new("ddp").about(ABOUT).version(env!("CARGO_PKG_VERSION")).subcommand_required(true)
        .subcommand(sub("skills", "the guides for agents, matched to this version")
            .arg(choices("action", &["list", "get", "path"]).default_value("list"))
            .arg(positional("name", "skill name").default_value("core"))
            .arg(flag("full", "get: add the full command reference"))
            .arg(opt("filter", "get: only the sections whose heading holds this text")))
        .subcommand(sub("doctor", "check the device, the app and the connection; prints the fix")
            .arg(positional("device", "simulator UDID or adb serial (default: the session's)"))
            .arg(opt("app", "bundle id / application id (default: the session's)"))
            .arg(opt("platform", "ios or android").value_parser(["ios", "android"])))
        .subcommand(sub("batch", "run several ddp commands in one call (arguments or stdin lines)")
            .arg(positional("lines", "commands without the leading \"ddp\", e.g. \"tree -i\"").num_args(0..))
            .arg(flag("bail", "stop at the first command that fails")))
        .subcommand(sub("connect", "attach to the running app (live mode)")
            .arg(positional("device", "simulator UDID or adb serial").required(true))
            .arg(opt("app", "bundle id / application id").required(true))
            .arg(opt("platform", "default: guessed from the device id").value_parser(["ios", "android"])))
        .subcommand(sub("launch", "start the app fresh, with values for it, then connect (live mode)")
            .arg(positional("device", "simulator UDID or adb serial").required(true))
            .arg(opt("app", "bundle id / application id").required(true))
            .arg(opt("platform", "default: guessed from the device id").value_parser(["ios", "android"]))
            .arg(flag("reset", "clear the app's data first (its files and preferences), like a fresh install"))
            .arg(Arg::new("env").long("env").num_args(1).action(ArgAction::Append).value_name("KEY=VALUE")
                .help("a value for the app: iOS environment variable, Android launch intent string extra; repeat for more"))
            .arg(Arg::new("arg").long("arg").num_args(1).action(ArgAction::Append).allow_hyphen_values(true).value_name("ARG")
                .help("iOS: a launch argument, as an Xcode scheme passes it (--arg -SomeDefault --arg YES); repeat for more")))
        .subcommand(sub("disconnect", "forget the live session"))
        .subcommand(file(window(sub("record", "record frames (live, or offline by notification/broadcast)")
            .arg(opt("platform", "offline only").value_parser(["ios", "android"]))
            .arg(opt("app", "offline only: bundle id / application id"))
            .arg(opt("device", "offline only: simulator UDID or adb serial (or env DDP_DEVICE)"))
            .arg(opt("name", "keep this take under a name (never pruned); use it as --file NAME"))
            .arg(opt("on", "sessions to record at once, e.g. ios,android; --then may use {platform} {device} {app}; \
                            then check parity runs on the two takes")))))
        .subcommand(sub("watch", "live: print an element's changes frame by frame")
            .arg(positional("id", "element id, label or @ref").required(true))
            .arg(opt("until", "stop when true, e.g. 'border>3' or 'present=false' \
                               (fields: t present x y w h alpha fill border borderColor opacity draws)"))
            .arg(opt("for", "stop after this long (1.5s, 1500ms)").id("duration"))
            .arg(opt("then", "program to run once watching starts (no shell)")))
        .subcommand(sub("eval", "live: read one property of an element, or set it (iOS)")
            .arg(positional("id", "element id, label or @ref").required(true))
            .arg(positional("path", "iOS: element.<key path> or layer.<key path>; \
                                     Android: <modifier>.<property> (border.width) or semantics.<Key>").required(true))
            .arg(opt("set", "iOS: set it first, a number, true/false or #RRGGBB(AA); prints what UIKit reads back")
                .allow_hyphen_values(true)))
        .subcommand(sub("net", "live: the app's requests (redacted): see them, send one, or answer them yourself")
            .arg(choices("action", &["list", "get", "watch", "send", "mock", "fail", "cookies", "clear-cookies"]).required(true))
            .arg(positional("request_id", "get: request id from `ddp net list`; send: URL; mock, fail: URL pattern with *"))
            .arg(opt("until", "watch: stop when true, e.g. 'status>=400' or 'url~/api'"))
            .arg(opt("method", "send: GET, POST..."))
            .arg(opt("body", "send: request body; mock: response body"))
            .arg(opt("status", "mock: response status (default 200)").value_parser(clap::value_parser!(u16).range(100..600)))
            .arg(choices("error", &["offline", "host", "timeout", "refused", "unreachable", "lost", "cancelled", "failed"])
                .long("error").help("fail: what the app's client throws (default offline)"))
            .arg(opt("for", "mock, fail: how long to answer requests (default 10s)").id("duration"))
            .arg(opt("limit", "mock, fail: stop after this many requests").value_parser(clap::value_parser!(usize)))
            .arg(opt("then", "mock, fail: program to run meanwhile (no shell)")))
        .subcommand(sub("time", "live: pause, step, slow down or seek the app's animations")
            .arg(choices("action", &["pause", "resume", "step", "rate", "seek", "status"]).required(true))
            .arg(positional("value", "step: frames (default 1); rate: factor (0.1); seek: time from the pause point (500ms, -1s)")
                .allow_hyphen_values(true)))
        .subcommand(file(window(sub("read", "summarize what changed per element (--json: per-frame states)")
            .arg(opt("id", "one element (id, label or @ref): print its per-frame timeline")))))
        .subcommand(file(window(sub("check", "checks over a recording (live: records first)")
            .arg(choices("what", &crate::checks::CHECK_NAMES).required(true))
            .arg(opt("expect", "timeline: expected cases JSON (ddp skills get core --filter timeline)"))
            .arg(opt("against", "parity: the other platform's recording; baseline: its name"))
            .arg(opt("junit", "baseline: also write a JUnit XML report here")))))
        .subcommand(sub("tap", "live: tap an element with a real touch, once the app's hit test says a touch reaches it")
            .arg(positional("id", "element id, label or @ref; or x,y to touch a point").required(true))
            .arg(flag("force", "touch the center without the checks"))
            .arg(opt("device", "without the library: x,y in screen pixels with adb input (Android only)")))
        .subcommand(sub("swipe", "live: a quick finger move from one place to another, then what changed")
            .arg(positional("id", "from: an element (its center) or x,y").required(true))
            .arg(positional("value", "to: an element or x,y").required(true))
            .arg(opt("for", "how long the move takes (default 300ms)").id("duration"))
            .arg(opt("device", "without the library: Android only, adb input in screen pixels")))
        .subcommand(sub("drag", "live: hold, then move slowly to another place and let go, then what changed")
            .arg(positional("id", "from: an element (its center) or x,y").required(true))
            .arg(positional("value", "to: an element or x,y").required(true))
            .arg(opt("for", "how long the move takes after a 500ms hold (default 600ms)").id("duration"))
            .arg(opt("device", "without the library: Android only, adb input in screen pixels")))
        .subcommand(sub("double-tap", "live: two quick taps, then what changed")
            .arg(positional("id", "an element (its center) or x,y").required(true))
            .arg(opt("device", "without the library: Android only, adb input in screen pixels")))
        .subcommand(sub("long-press", "live: hold a finger down, then what changed")
            .arg(positional("id", "an element (its center) or x,y").required(true))
            .arg(opt("for", "how long to hold (default 800ms)").id("duration"))
            .arg(opt("device", "without the library: Android only, adb input in screen pixels")))
        .subcommand(sub("type", "live: type into the focused field, as the keyboard does")
            .arg(positional("value", "the text").required(true))
            .arg(opt("into", "tap this element first (id, label or @ref)").id("id")))
        .subcommand(sub("press", "live: press a key in the focused field")
            .arg(choices("value", &["Enter", "Tab", "Backspace"]).required(true)))
        .subcommand(sub("prefs", "live: the app's UserDefaults (iOS) or SharedPreferences (Android), secrets redacted")
            .arg(choices("action", &["list", "set", "rm", "clear", "files"]).default_value("list"))
            .arg(positional("key", "set, rm: the key"))
            .arg(positional("value", "set: the value as text; the key keeps its type"))
            .arg(opt("in", "a suite (iOS) or preferences file (Android); default the app's own").id("name")))
        .subcommand(sub("storage", "live: the app's files, its SQLite databases read-only, its Keychain/Keystore names")
            .arg(choices("action", &["files", "read", "rm", "query", "keys"]).default_value("files"))
            .arg(positional("key", "files: a folder (default the app's home); read, rm: a file; query: a database file"))
            .arg(positional("value", "query: one SQL statement, run read-only"))
            .arg(opt("limit", "read: lines, query: rows to print").value_parser(clap::value_parser!(usize)).default_value("50")))
        .subcommand(sub("state", "live: values the app added in Debug; read, watch, and set the writable ones")
            .arg(choices("action", &["list", "get", "set", "watch"]).default_value("list"))
            .arg(positional("key", "get, set, watch: the value's name"))
            .arg(positional("value", "set: the new value, as JSON (500, true, \"text\") or plain text"))
            .arg(opt("until", "watch: stop when true, e.g. '>100' or 'total>100 and mode=done' (fields: the names watched)"))
            .arg(opt("for", "watch: stop after this long (default 10s)").id("duration")))
        .subcommand(sub("run", "live: run a command the app added in Debug; without a name, list them")
            .arg(positional("name", "the command's name"))
            .arg(positional("value", "params as a JSON object")))
        .subcommand(sub("wait", "live: until an element is drawn, a label shows, or the screen settles")
            .arg(choices("by", &["id", "text"]))
            .arg(positional("value", "the id or the text"))
            .arg(opt("settled", "until nothing changes for this long (300ms)"))
            .arg(opt("timeout", "give up after this long").default_value("10s"))
            .arg(many("ignore", "--settled: elements that never stop (a spinner, a clock) and should not count")))
        .subcommand(file(device_options(sub("logs", "the app's log lines during the last take, at their time in it")
            .arg(flag("follow", "live: the last 100 lines, then each new one"))
            .arg(flag("source", "--follow: where each line came from (iOS: subsystem and library)"))
            .arg(opt("for", "--follow: stop after this long").id("duration"))
            .arg(opt("grep", "only lines holding this text"))
            .arg(opt("limit", "at most this many lines").value_parser(clap::value_parser!(usize)).default_value("50")))))
        .subcommand(device_options(sub("crash", "the app's last crash as the platform recorded it, with its stack (the app may be gone)")))
        .subcommand(sub("media", "live: the players the platform shows (video layers, now playing), with their state"))
        .subcommand(sub("perf", "live: refreshes the main thread missed and draw time, for whatever is on screen")
            .arg(opt("for", "how long to measure (default 5s)").id("duration")))
        .subcommand(sub("events", "live: events the app sent in Debug (a rule that fired, a sync that ended)")
            .arg(flag("follow", "the kept ones, then each new one"))
            .arg(opt("for", "--follow: stop after this long").id("duration"))
            .arg(opt("grep", "only events whose name or fields hold this text"))
            .arg(opt("limit", "without --follow: the last this many").value_parser(clap::value_parser!(usize)).default_value("50")))
        .subcommand(device_options(sub("history", "the last 15 minutes from the app's flight recorder")
            .arg(choices("action", &["list", "at"]).default_value("list"))
            .arg(positional("time", "at: a time from the list, e.g. 14:31:02.5"))
            .arg(opt("since", "list: how far back (90s, 5m); default 15m").default_value("15m"))
            .arg(opt("for", "at: how long the take is").id("duration").default_value("2s"))
            .arg(opt("limit", "list: the last N lines").value_parser(clap::value_parser!(usize)).default_value("60"))
            .arg(opt("name", "at: keep the take under a name"))
            .arg(many("ignore", "list: elements to leave out (the ones that change in most frames already are)"))
            .arg(opt("dir", "read history files from this folder instead of the device"))))
        .subcommand(device_options(sub("set", "device settings: appearance, text size, reduce motion, locale, location, orientation")
            .arg(choices("what", &["appearance", "text-size", "reduce-motion", "locale", "location", "orientation"]).required(true))
            .arg(positional("value", "appearance dark|light; text-size (iOS: large, extra-large, accessibility-large...; \
                                      Android: small|default|large|largest or a factor); reduce-motion on|off; locale es-ES; \
                                      location LAT,LON; orientation portrait|landscape (Android)")
                .required(true))))
        .subcommand(device_options(sub("profile", "where the app spends its time, from the platform's sampler (sample, simpleperf)")
            .arg(opt("for", "how long to sample (default 3s)").id("duration"))))
        .subcommand(sub("highlight", "live: draw a box over an element in the live app, for a person watching")
            .arg(positional("id", "element id, label or @ref").required(true))
            .arg(opt("for", "how long (default 3s)").id("duration")))
        .subcommand(device_options(sub("inspect", "live: wait for a person to tap an element on the phone, then say which it was")
            .arg(opt("for", "how long to wait (default 30s)").id("duration"))
            .arg(opt("dir", "read history files from this folder instead of the device"))))
        .subcommand(device_options(sub("memory", "the app's memory now, its leaks (iOS) or a heap dump, with the platform's tools")
            .arg(choices("action", &["summary", "leaks", "heap"]).default_value("summary"))))
        .subcommand(device_options(sub("audits", "runtime warnings the platform wrote in the app's log (SwiftUI, Auto Layout, StrictMode)")
            .arg(opt("since", "how far back (default 5m)").default_value("5m"))))
        .subcommand(device_options(sub("back", "go back: the Back key on Android, the edge swipe on iOS")))
        .subcommand(device_options(sub("background", "send the app to the background for a while, then bring it back")
            .arg(opt("for", "how long in the background (default 2s)").id("duration"))))
        .subcommand(device_options(sub("biometric", "simulated Face ID / Touch ID or fingerprint: enroll, then match or fail")
            .arg(choices("action", &["enroll", "match", "fail"]).required(true))))
        .subcommand(device_options(sub("push", "iOS: deliver a push notification to the app, as APNs would")
            .arg(positional("path", "the payload JSON file ({\"aps\": {\"alert\": \"Hi\"}})").required(true))))
        .subcommand(device_options(sub("job", "Android: run one of the app's scheduled jobs now (JobScheduler, WorkManager)")
            .arg(positional("value", "the job id").required(true))))
        .subcommand(sub("compare", "how far the screen is from a reference image, or one element from the other phone")
            .arg(positional("id", "an element: its pixels in the app's windows (without it, the device's whole screen)"))
            .arg(opt("reference", "a reference image (PNG; on macOS any image sips opens), scaled to the screen's width"))
            .arg(opt("on", "with an element: two sessions to compare it across, e.g. ios,android"))
            .arg(opt("device", "without a session: simulator UDID or adb serial"))
            .arg(opt("platform", "default: guessed from the device id").value_parser(["ios", "android"]))
            .arg(many("ignore", "elements to leave out of the scores, such as a clock (needs a session)"))
            .arg(Arg::new("mask").long("mask").num_args(1).action(ArgAction::Append).value_name("X,Y,W,H")
                .help("the whole screen: a rectangle in screenshot pixels to leave out, such as the status bar; repeat for more"))
            .arg(opt("screen", "the whole screen: a saved screenshot instead of the device's, to compare offline"))
            .arg(flag("structure", "with --on and no element: what each phone draws by label, not pixels"))
            .arg(opt("max", "exit 1 when more than this share of pixels differ overall (5 or 5%)"))
            .arg(opt("max-band", "exit 1 when more than this share of pixels differ in any band (20 or 20%)"))
            .arg(opt("out", "where to write the side-by-side PNG").default_value("ddp-compare.png")))
        .subcommand(sub("annotate", "live: a screenshot with numbered boxes, label N is @eN")
            .arg(positional("out", "where to write the PNG").default_value("ddp-annotated.png")))
        .subcommand(sub("explore", "live: tap each control once, record it, run check all on each")
            .arg(opt("per", "how long to record after each tap").default_value("2s"))
            .arg(many("skip", "controls not to tap")))
        .subcommand(sub("mcp", "a Model Context Protocol server on stdio (tools: core, live, checks, all)")
            .arg(opt("tools", "comma-separated profiles: core, live, checks, all").default_value("core")))
        .subcommand(file(sub("view", "a local HTML page of a take, frame by frame, for people")
            .arg(positional("out", "where to write the page").default_value("ddp-take.html"))))
        .subcommand(device_options(sub("open", "open a deep link or URL on the device")
            .arg(positional("url", "the link").required(true))))
        .subcommand(file(sub("baseline", "keep a take under ddp-baselines/ to check against later")
            .arg(choices("action", &["save"]).required(true))
            .arg(positional("name", "the baseline's name").required(true))))
        .subcommand(file(window(sub("tree", "elements and what they drew at one moment (live: now)")
            .arg(opt("at", "offline: time (1.5s, 1500ms, 1500); default the last frame"))
            .arg(opt("id", "only this element"))
            .arg(Arg::new("interactive").short('i').long("interactive").action(ArgAction::SetTrue)
                .help("elements only, no paint nodes"))
            .arg(Arg::new("changed").short('c').long("changed").action(ArgAction::SetTrue)
                .help("only elements that change in the recording (live: records --for first)"))
            .arg(opt("within", "only this element and what is inside it"))
            .arg(Arg::new("depth").short('d').long("depth").value_parser(clap::value_parser!(i64))
                .help("nesting levels to print (0: top level only)")))))
        .subcommand(file(window(sub("get", "one element per frame over a time window")
            .arg(positional("id", "element id, label or @ref").required(true))
            .arg(opt("from", "time (1.5s, 1500ms)").id("start"))
            .arg(opt("to", "time (1.5s, 1500ms)").id("end"))
            .arg(flag("curve", "each move as from -> to, duration, delay and easing (or spring overshoot)")))))
        .subcommand(file(window(sub("find", "elements by text or id, or drawn nodes matching --where")
            .arg(positional("by", "text | id"))
            .arg(positional("value", "part of a label (text) or an element id (id)"))
            .arg(opt("where", "e.g. 'kind=draw and alpha>0'; fields: t z kind op owner source fill border \
                               borderColor corner opacity alpha x y w h; operators = != > < >= <= ~(contains)"))
            .arg(Arg::new("between").long("between").num_args(2).value_names(["FROM", "TO"]).allow_hyphen_values(true))
            .arg(opt("limit", "at most this many rows").value_parser(clap::value_parser!(usize)).default_value("20")))))
        .subcommand(sub("diff", "two recordings (default: the last two takes): the screen, or one element")
            .arg(positional("recordings", "0, 1 (against the last take) or 2 recordings").num_args(0..))
            .arg(opt("id", "one element frame by frame; without it, the screen at the end of each"))
            .arg(Arg::new("ignore").long("ignore").num_args(1..).value_name("FIELD")
                .help("fields to leave out: frame alpha fill border borderColor opacity draws")))
}

fn has(matches: &ArgMatches, id: &str) -> bool {
    matches.try_contains_id(id).unwrap_or(false)
}

fn string(matches: &ArgMatches, id: &str) -> Option<String> {
    if !has(matches, id) {
        return None;
    }
    matches.try_get_one::<String>(id).ok().flatten().cloned()
}

fn strings(matches: &ArgMatches, id: &str) -> Vec<String> {
    if !has(matches, id) {
        return Vec::new();
    }
    matches.try_get_many::<String>(id).ok().flatten().map(|v| v.cloned().collect()).unwrap_or_default()
}

fn boolean(matches: &ArgMatches, id: &str) -> bool {
    has(matches, id) && matches.try_get_one::<bool>(id).ok().flatten().copied().unwrap_or(false)
}

fn number<T: Clone + Send + Sync + 'static>(matches: &ArgMatches, id: &str) -> Option<T> {
    if !has(matches, id) {
        return None;
    }
    matches.try_get_one::<T>(id).ok().flatten().cloned()
}

/// Parses argv (without the program name). Err holds clap's message and exit code.
pub fn parse(argv: &[String]) -> Result<Args, clap::Error> {
    let mut argv = argv.to_vec();
    if argv.len() >= 2 && argv[0] == "time" && argv[1] == "seek" {
        argv.insert(2, "--".into()); // so a rewind like `seek -400ms` is a value, not a flag
    }
    let matches = parser().try_get_matches_from(std::iter::once("ddp".to_string()).chain(argv))?;
    let (command, m) = matches.subcommand().unwrap();
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let between = strings(m, "between");
    Ok(Args {
        command: command.to_string(),
        session: string(m, "session").or_else(|| env("DDP_SESSION")),
        json: boolean(m, "json"),
        max_output: number::<usize>(m, "max-output").or_else(|| env("DDP_MAX_OUTPUT").and_then(|v| v.parse().ok())),
        content_boundaries: boolean(m, "content-boundaries") || env("DDP_CONTENT_BOUNDARIES").is_some(),
        shared: false,
        file: string(m, "file").map(PathBuf::from),
        duration: string(m, "duration"),
        frames: number::<i64>(m, "frames"),
        then: string(m, "then"),
        id: string(m, "id"),
        device: string(m, "device"),
        app: string(m, "app"),
        platform: string(m, "platform"),
        name: string(m, "name"),
        on: string(m, "on"),
        action: string(m, "action"),
        what: string(m, "what"),
        value: string(m, "value"),
        by: string(m, "by"),
        path: string(m, "path"),
        request_id: string(m, "request_id"),
        until: string(m, "until"),
        expect: string(m, "expect").map(PathBuf::from),
        reference: string(m, "reference").map(PathBuf::from),
        masks: strings(m, "mask"),
        screen: string(m, "screen").map(PathBuf::from),
        max: string(m, "max"),
        max_band: string(m, "max-band"),
        env: strings(m, "env"),
        launch_args: strings(m, "arg"),
        set_value: string(m, "set"),
        against: string(m, "against").map(PathBuf::from),
        junit: string(m, "junit").map(PathBuf::from),
        settled: string(m, "settled"),
        timeout: string(m, "timeout").unwrap_or_else(|| "10s".into()),
        ignore: strings(m, "ignore"),
        grep: string(m, "grep"),
        limit: number::<usize>(m, "limit"),
        at: string(m, "at"),
        interactive: boolean(m, "interactive"),
        changed: boolean(m, "changed"),
        within: string(m, "within"),
        depth: number::<i64>(m, "depth"),
        start: string(m, "start"),
        end: string(m, "end"),
        curve: boolean(m, "curve"),
        where_: string(m, "where"),
        between: (between.len() == 2).then(|| (between[0].clone(), between[1].clone())),
        recordings: strings(m, "recordings").into_iter().map(PathBuf::from).collect(),
        out: string(m, "out").unwrap_or_default(),
        url: string(m, "url"),
        per: string(m, "per").unwrap_or_else(|| "2s".into()),
        skip: strings(m, "skip"),
        tools: string(m, "tools").unwrap_or_else(|| "core".into()),
        lines: strings(m, "lines"),
        bail: boolean(m, "bail"),
        full: boolean(m, "full"),
        filter: string(m, "filter"),
        since: string(m, "since").unwrap_or_else(|| "15m".into()),
        time: string(m, "time"),
        dir: string(m, "dir").map(PathBuf::from),
        method: string(m, "method"),
        body: string(m, "body"),
        status: number::<u16>(m, "status"),
        error: string(m, "error"),
        key: string(m, "key"),
        follow: boolean(m, "follow"),
        source: boolean(m, "source"),
        force: boolean(m, "force"),
        reset: boolean(m, "reset"),
        structure: boolean(m, "structure"),
    })
}
