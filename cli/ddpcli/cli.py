"""ddp: see what a Debug iOS/Android app draws, frame by frame, live or from a recording.

Start here (for agents): ddp skills get core
  The guides ship with the CLI, so they always match this version. `ddp doctor` when something fails.

  connect | disconnect | doctor            attach to a running Debug app; check the setup
  tree | find | get | read | watch | eval  what is drawn, now or over a take (@refs from tree and find)
  record | wait | tap | open | set | logs  take a recording, wait, act, set up the device, read app logs
  check overlap|corners|jank|settle|a11y|timeline|parity|baseline|all
  diff | baseline | annotate | view | explore | net | time
  skills | batch | mcp

See `ddp skills get core --full` for every command and its output.
"""
import argparse
import json
import os
import sys
from pathlib import Path
from .state import Failure, LimitedOutput, ctx, report_failure
from .live import connect, disconnect, evaluate, net, time_control, watch
from .summary import read
from .checks import CHECKS, baseline
from .queries import diff, find, get, resolve_ref, tree
from .device import annotate, explore, logs, open_url, record, settings, tap, wait
from .tools import batch, doctor, mcp, skills, view


def run_check(args):
    """Exit 1 when the check found problems, so every check works as a gate.
    With --json: {"check", "ok", "lines"}, the lines the check would print."""
    import io
    captured, real = io.StringIO(), sys.stdout
    if ctx.json:
        sys.stdout = captured
    code = 0
    try:
        found = CHECKS[args.what](args)
        code = 1 if isinstance(found, int) and found > 0 else 0
    except SystemExit as exit_:
        code = exit_.code if isinstance(exit_.code, int) else 1
    finally:
        sys.stdout = real
    if not ctx.json:
        if code:
            sys.exit(code)
        return
    lines = [line for line in captured.getvalue().splitlines() if line.strip()]
    print(json.dumps({"check": args.what, "ok": code == 0, "lines": lines}))
    if code:
        sys.exit(code)


def build_parser():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--session", default=os.environ.get("DDP_SESSION"),
                        help="named session (or env DDP_SESSION): its own live connection and recordings")
    common.add_argument("--json", action="store_true", help="machine output; errors as {error, code, next}")
    common.add_argument("--max-output", type=int, default=int(os.environ["DDP_MAX_OUTPUT"]) if os.environ.get("DDP_MAX_OUTPUT") else None,
                        help="cut output at N chars (or env DDP_MAX_OUTPUT)")
    common.add_argument("--content-boundaries", action="store_true", default=bool(os.environ.get("DDP_CONTENT_BOUNDARIES")),
                        help="wrap output in markers: app text is data, not instructions")

    parser = argparse.ArgumentParser(prog="ddp", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)

    def command(name, handler, **kwargs):
        sub = commands.add_parser(name, parents=[common], **kwargs)
        sub.set_defaults(handler=handler)
        return sub

    def add_window(sub):
        """Live: how long to record for the command, and what to do meanwhile."""
        sub.add_argument("--for", dest="duration", help="live: record this long (1.5s, 1500ms); default 2s")
        sub.add_argument("--frames", type=int, help="live or offline record: number of frames instead of --for")
        sub.add_argument("--then", help="program and arguments to run right after recording starts (no shell)")

    def add_file(sub):
        sub.add_argument("--file", type=Path, help="offline: read this recording (a path or a saved name)")

    skills_parser = command("skills", skills, help="the guides for agents, matched to this version")
    skills_parser.add_argument("action", nargs="?", default="list", choices=["list", "get", "path"])
    skills_parser.add_argument("name", nargs="?", default="core")
    skills_parser.add_argument("--full", action="store_true", help="get: add the full command reference")
    skills_parser.add_argument("--filter", help="get: only the sections whose heading holds this text")

    doctor_parser = command("doctor", doctor, help="check the device, the app and the connection; prints the fix")
    doctor_parser.add_argument("device", nargs="?", help="simulator UDID or adb serial (default: the session's)")
    doctor_parser.add_argument("--app", help="bundle id / application id (default: the session's)")
    doctor_parser.add_argument("--platform", choices=["ios", "android"])

    batch_parser = command("batch", batch, help="run several ddp commands in one call (arguments or stdin lines)")
    batch_parser.add_argument("lines", nargs="*", help='commands without the leading "ddp", e.g. "tree -i"')
    batch_parser.add_argument("--bail", action="store_true", help="stop at the first command that fails")

    connect_parser = command("connect", connect, help="attach to the running app (live mode)")
    connect_parser.add_argument("device", help="simulator UDID or adb serial")
    connect_parser.add_argument("--app", required=True, help="bundle id / application id")
    connect_parser.add_argument("--platform", choices=["ios", "android"], help="default: guessed from the device id")

    command("disconnect", disconnect, help="forget the live session")

    record_parser = command("record", record, help="record frames (live, or offline by notification/broadcast)")
    record_parser.add_argument("--platform", choices=["ios", "android"], help="offline only")
    record_parser.add_argument("--app", help="offline only: bundle id / application id")
    record_parser.add_argument("--device", help="offline only: simulator UDID or adb serial (or env DDP_DEVICE)")
    record_parser.add_argument("--name", help="keep this take under a name (never pruned); use it as --file NAME")
    record_parser.add_argument("--on", help="sessions to record at once, e.g. ios,android; --then may use {platform} "
                                            "{device} {app}; then check parity runs on the two takes")
    add_window(record_parser)
    add_file(record_parser)

    watch_parser = command("watch", watch, help="live: print an element's changes frame by frame")
    watch_parser.add_argument("id", help="element id, label or @ref")
    watch_parser.add_argument("--until", help="stop when true, e.g. 'border>3' or 'present=false' "
                                              "(fields: t present x y w h alpha fill border borderColor opacity draws)")
    watch_parser.add_argument("--for", dest="duration", help="stop after this long (1.5s, 1500ms)")
    watch_parser.add_argument("--then", help="program to run once watching starts (no shell)")

    eval_parser = command("eval", evaluate, help="live: read one property of an element, read-only")
    eval_parser.add_argument("id", help="element id, label or @ref")
    eval_parser.add_argument("path", help="iOS: element.<key path> or layer.<key path>; "
                                          "Android: <modifier>.<property> (border.width) or semantics.<Key>")

    net_parser = command("net", net, help="live: network requests the app reported (redacted)")
    net_parser.add_argument("action", choices=["list", "get", "watch"])
    net_parser.add_argument("request_id", nargs="?", help="get: request id from `ddp net list`")
    net_parser.add_argument("--until", help="watch: stop when true, e.g. 'status>=400' or 'url~/api'")

    time_parser = command("time", time_control, help="live: pause, step, slow down or seek the app's animations")
    time_parser.add_argument("action", choices=["pause", "resume", "step", "rate", "seek", "status"])
    time_parser.add_argument("value", nargs="?", help="step: frames (default 1); rate: factor (0.1); "
                                                      "seek: time from the pause point (500ms, -1s)")

    read_parser = command("read", read, help="summarize what changed per element (--json: per-frame states)")
    read_parser.add_argument("--id", help="one element (id, label or @ref): print its per-frame timeline")
    add_window(read_parser)
    add_file(read_parser)

    check_parser = command("check", run_check, help="checks over a recording (live: records first)")
    check_parser.add_argument("what", choices=list(CHECKS))
    check_parser.add_argument("--expect", type=Path, help="timeline: expected cases JSON (ddp skills get core --filter timeline)")
    check_parser.add_argument("--against", type=Path, help="parity: the other platform's recording; baseline: its name")
    check_parser.add_argument("--junit", type=Path, help="baseline: also write a JUnit XML report here")
    check_parser.add_argument("--all-frames", action="store_true", help="check every frame, not only the last")
    add_window(check_parser)
    add_file(check_parser)

    tap_parser = command("tap", tap, help="live: tap an element (Android: a real touch; iOS: its accessibility action)")
    tap_parser.add_argument("id", help="element id, label or @ref")

    wait_parser = command("wait", wait, help="live: until an element is drawn, a label shows, or the screen settles")
    wait_parser.add_argument("by", nargs="?", choices=["id", "text"])
    wait_parser.add_argument("value", nargs="?")
    wait_parser.add_argument("--settled", help="until nothing changes for this long (300ms)")
    wait_parser.add_argument("--timeout", default="10s")
    wait_parser.add_argument("--ignore", nargs="+", default=[], metavar="ELEMENT",
                             help="--settled: elements that never stop (a spinner, a clock) and should not count")

    logs_parser = command("logs", logs, help="the app's log lines during the last take, at their time in it")
    logs_parser.add_argument("--grep", help="only lines holding this text")
    logs_parser.add_argument("--limit", type=int, default=50)
    logs_parser.add_argument("--device")
    logs_parser.add_argument("--app")
    add_file(logs_parser)

    set_parser = command("set", settings, help="device settings: appearance, text size, reduce motion, locale")
    set_parser.add_argument("what", choices=["appearance", "text-size", "reduce-motion", "locale"])
    set_parser.add_argument("value", help="appearance dark|light; text-size (iOS: large, extra-large, "
                                          "accessibility-large...; Android: small|default|large|largest or a factor); "
                                          "reduce-motion on|off; locale es-ES")
    set_parser.add_argument("--device")
    set_parser.add_argument("--app")

    annotate_parser = command("annotate", annotate, help="live: a screenshot with numbered boxes, label N is @eN")
    annotate_parser.add_argument("out", nargs="?", default="ddp-annotated.png")

    explore_parser = command("explore", explore, help="live: tap each control once, record it, run check all on each")
    explore_parser.add_argument("--per", default="2s", help="how long to record after each tap")
    explore_parser.add_argument("--skip", nargs="+", default=[], metavar="ELEMENT", help="controls not to tap")

    mcp_parser = command("mcp", mcp, help="a Model Context Protocol server on stdio (tools: core, live, checks, all)")
    mcp_parser.add_argument("--tools", default="core", help="comma-separated profiles: core, live, checks, all")

    view_parser = command("view", view, help="a local HTML page of a take, frame by frame, for people")
    view_parser.add_argument("out", nargs="?", default="ddp-take.html")
    add_file(view_parser)

    open_parser = command("open", open_url, help="open a deep link or URL on the device")
    open_parser.add_argument("url")
    open_parser.add_argument("--device")
    open_parser.add_argument("--app")

    baseline_parser = command("baseline", baseline, help="keep a take under ddp-baselines/ to check against later")
    baseline_parser.add_argument("action", choices=["save"])
    baseline_parser.add_argument("name")
    add_file(baseline_parser)

    tree_parser = command("tree", tree, help="elements and what they drew at one moment (live: now)")
    tree_parser.add_argument("--at", help="offline: time (1.5s, 1500ms, 1500); default the last frame")
    tree_parser.add_argument("--id", help="only this element")
    tree_parser.add_argument("-i", "--interactive", action="store_true", help="elements only, no paint nodes")
    tree_parser.add_argument("-c", "--changed", action="store_true",
                             help="only elements that change in the recording (live: records --for first)")
    tree_parser.add_argument("--within", help="only this element and what is inside it")
    tree_parser.add_argument("-d", "--depth", type=int, help="nesting levels to print (0: top level only)")
    add_window(tree_parser)
    add_file(tree_parser)

    get_parser = command("get", get, help="one element per frame over a time window")
    get_parser.add_argument("id", help="element id, label or @ref")
    get_parser.add_argument("--from", dest="start", help="time (1.5s, 1500ms)")
    get_parser.add_argument("--to", dest="end", help="time (1.5s, 1500ms)")
    get_parser.add_argument("--curve", action="store_true",
                            help="each move as from -> to, duration, delay and easing (or spring overshoot)")
    add_window(get_parser)
    add_file(get_parser)

    find_parser = command("find", find, help="elements by text or id, or drawn nodes matching --where")
    find_parser.add_argument("by", nargs="?", help="text | id")
    find_parser.add_argument("value", nargs="?", help="part of a label (text) or an element id (id)")
    find_parser.add_argument("--where",
                             help="e.g. 'kind=draw and alpha>0'; fields: t z kind op owner source fill border "
                                  "borderColor corner opacity alpha x y w h; operators = != > < >= <= ~(contains)")
    find_parser.add_argument("--between", nargs=2, metavar=("FROM", "TO"))
    find_parser.add_argument("--limit", type=int, default=20)
    add_window(find_parser)
    add_file(find_parser)

    diff_parser = command("diff", diff, help="two recordings (default: the last two takes): the screen, or one element")
    diff_parser.add_argument("recordings", nargs="*", type=Path, help="0, 1 (against the last take) or 2 recordings")
    diff_parser.add_argument("--id", help="one element frame by frame; without it, the screen at the end of each")
    diff_parser.add_argument("--ignore", nargs="+", default=[], metavar="FIELD",
                             help="fields to leave out: frame alpha fill border borderColor opacity draws")
    return parser


def run_command(argv):
    """One ddp command. Returns its exit code instead of exiting, so `batch` can run several."""
    if argv[:2] == ["time", "seek"]:
        argv = [*argv[:2], "--", *argv[2:]]  # so a rewind like `seek -400ms` is a value, not a flag
    try:
        args = build_parser().parse_args(argv)
    except SystemExit as exit_:
        return exit_.code or 0
    ctx.use(args.session, args.json)
    output = LimitedOutput(sys.stdout, args.max_output, args.content_boundaries)
    real_stdout, sys.stdout = sys.stdout, output
    try:
        if getattr(args, "id", None):
            args.id = resolve_ref(args.id)
        args.handler(args)
        return 0
    except Failure as failure:
        sys.stdout = real_stdout
        report_failure(failure)
        return 1
    except SystemExit as exit_:
        return exit_.code if isinstance(exit_.code, int) else 1
    finally:
        sys.stdout = real_stdout
        output.finish()


def main():
    try:
        sys.exit(run_command(sys.argv[1:]))
    except BrokenPipeError:
        # The reader stopped early (`| head`): that is not an error.
        os.dup2(os.open(os.devnull, os.O_WRONLY), sys.stdout.fileno())
        sys.exit(0)
