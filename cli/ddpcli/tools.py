import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from .state import docs_root, Failure, IOS_DIR, adb_path, ctx, fail, run
from .model import element_alpha, element_key, element_keys, element_state, load
from .live import LiveClient
from .summary import describe_track


VIEW_PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>ddp take</title>
<style>
:root { --bg: #fafafa; --fg: #1d1d1f; --muted: #6e6e73; --line: #d2d2d7; --pick: #0a66d8; }
@media (prefers-color-scheme: dark) { :root { --bg: #161617; --fg: #f5f5f7; --muted: #a1a1a6; --line: #3a3a3c; --pick: #4a9eff; } }
body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 14px/1.4 -apple-system, system-ui, sans-serif; }
main { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr); gap: 24px; }
@media (max-width: 720px) { main { grid-template-columns: 1fr; } }
svg.screen { width: 100%; max-height: 80vh; border: 1px solid var(--line); background: var(--bg); }
.row { display: grid; grid-template-columns: 9em 1fr; gap: 8px; padding: 4px 0; border-bottom: 1px solid var(--line); cursor: pointer; }
.row.on { color: var(--pick); }
.track { font: 12px ui-monospace, monospace; color: var(--muted); }
input[type=range] { width: 100%; }
h1 { font-size: 16px; margin: 0 0 8px; } .meta { color: var(--muted); margin-bottom: 12px; }
</style></head><body>
<h1>ddp take</h1><div class="meta" id="meta"></div>
<input type="range" id="t" min="0" value="0"><div class="meta" id="now"></div>
<main><svg class="screen" id="screen"></svg><div id="list"></div></main>
<script>
const take = __DATA__;
const slider = document.getElementById("t"), screen = document.getElementById("screen");
let picked = null;
slider.max = take.frames.length - 1;
document.getElementById("meta").textContent =
  `${take.platform}, ${take.frames.length} frames over ${Math.round(take.frames.at(-1).t)} ms, unit ${take.unit}`;
const [width, height] = take.size;
screen.setAttribute("viewBox", `0 0 ${width} ${height}`);
function draw() {
  const frame = take.frames[slider.value];
  document.getElementById("now").textContent = `t=${Math.round(frame.t)} ms (frame ${slider.value})`;
  screen.innerHTML = frame.boxes.map(([key, x, y, w, h, fill, border, alpha]) =>
    `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${fill || "none"}" fill-opacity="${fill ? alpha : 0}"
      stroke="${key === picked ? "var(--pick)" : (border ? "#e5484d" : "var(--muted)")}"
      stroke-width="${key === picked ? 4 : 1 + border}"><title>${key}</title></rect>`).join("");
}
document.getElementById("list").innerHTML = take.tracks.map(([key, lines]) =>
  `<div class="row" data-key="${key}"><div>${key}</div><div class="track">${lines.join("<br>") || "no change"}</div></div>`).join("");
document.querySelectorAll(".row").forEach(row => row.onclick = () => {
  picked = picked === row.dataset.key ? null : row.dataset.key;
  document.querySelectorAll(".row").forEach(r => r.classList.toggle("on", r.dataset.key === picked));
  draw();
});
slider.oninput = draw;
draw();
</script></body></html>
"""


def view(args):
    """Writes a self-contained HTML page: the take frame by frame, and each element's changes. No server."""
    recording = load(args.file or ctx.last)
    frames, header = recording["frames"], recording["header"]
    times = [f["t"] for f in frames]
    keys = element_keys(frames)
    boxes = []
    for frame in frames:
        rows = []
        for element in frame["elements"]:
            state = element_state(frame, element_key(element))
            fill = state["fill"]
            rows.append([element_key(element), *element["frame"], fill and f"#{fill[1:7]}", state["border"],
                         round(element_alpha(frame, element_key(element)), 2)])
        boxes.append({"t": frame["t"], "boxes": rows})
    edges = [e["frame"] for f in frames for e in f["elements"]]
    size = [max((x + w for x, _, w, _ in edges), default=400), max((y + h for _, y, _, h in edges), default=800)]
    tracks = []
    for key in keys:
        states = [element_state(f, key) for f in frames]
        lines = [describe_track(name, states, times) for name in ("frame", "fill", "border", "opacity", "draws", "label")]
        tracks.append([key, [line.strip() for line in lines if line]])
    data = {"platform": header["platform"], "unit": header["unit"], "size": size, "frames": boxes, "tracks": tracks}
    # "</" inside the JSON would end the script tag early.
    page = VIEW_PAGE.replace("__DATA__", json.dumps(data).replace("</", "<\\/"))
    Path(args.out).write_text(page)
    print(f"{args.out}: {len(frames)} frames, {len(keys)} elements. Open it in a browser")


MCP_PROFILES = {
    "core": ["doctor", "connect", "tree", "find", "get", "read", "check", "wait", "tap", "skills"],
    "live": ["watch", "eval", "time", "net", "record", "logs", "annotate"],
    "checks": ["check", "baseline", "diff"],
}


def mcp(args):
    """A Model Context Protocol server on stdin/stdout: each ddp command is a tool taking the command's
    arguments as one string. --tools picks small sets so a client's context stays small."""
    import io
    from .cli import run_command  # cli imports this module, so not at the top
    wanted = []
    for profile in args.tools.split(","):
        names = sorted({n for names in MCP_PROFILES.values() for n in names}) if profile == "all" else MCP_PROFILES[profile]
        wanted += [n for n in names if n not in wanted]
    real_stdout = sys.__stdout__
    tools = [{"name": f"ddp_{name}", "description": f"ddp {name}: see `ddp skills get core`. Pass the arguments as "
              f"you would after `ddp {name}` on a command line.",
              "inputSchema": {"type": "object", "properties": {"args": {"type": "string"}}}} for name in wanted]

    def reply(message_id, result=None, error=None):
        body = {"jsonrpc": "2.0", "id": message_id}
        body.update({"error": error} if error else {"result": result})
        real_stdout.write(json.dumps(body) + "\n")
        real_stdout.flush()

    for line in sys.stdin:
        if not line.strip():
            continue
        message = json.loads(line)
        method, message_id = message.get("method"), message.get("id")
        if method == "initialize":
            reply(message_id, {"protocolVersion": message.get("params", {}).get("protocolVersion", "2025-06-18"),
                               "capabilities": {"tools": {}}, "serverInfo": {"name": "ddp", "version": "1"}})
        elif method == "tools/list":
            reply(message_id, {"tools": tools})
        elif method == "tools/call":
            name = message["params"]["name"].removeprefix("ddp_")
            if name not in wanted:
                reply(message_id, error={"code": -32602, "message": f"unknown tool {name}"})
                continue
            captured, errors = io.StringIO(), io.StringIO()
            saved = sys.stdout, sys.stderr
            sys.stdout, sys.stderr = captured, errors
            try:
                code = run_command([name, *shlex.split(message["params"].get("arguments", {}).get("args", ""))])
            finally:
                sys.stdout, sys.stderr = saved
            text = (captured.getvalue() + errors.getvalue()).strip() or "(no output)"
            reply(message_id, {"content": [{"type": "text", "text": text}], "isError": code != 0})
        elif message_id is not None:
            reply(message_id, error={"code": -32601, "message": f"no method {method}"})


def skill_files():
    return {path.name[:-3]: path for path in sorted((docs_root() / "skills").iterdir(), key=lambda p: p.name)
            if path.name.endswith(".md")}


def frontmatter(text):
    """The `key: value` lines between the leading `---` markers, and the text after them."""
    if not text.startswith("---\n"):
        return {}, text
    head, _, body = text[4:].partition("\n---\n")
    return dict(line.split(": ", 1) for line in head.splitlines() if ": " in line), body.lstrip("\n")


def sections(text, wanted):
    """The `##` / `###` sections whose heading holds `wanted`, any case, each up to the next heading of its level."""
    kept, keep_level, in_code = [], None, False
    for line in text.splitlines():
        in_code ^= line.startswith("```")
        level = len(line) - len(line.lstrip("#")) if line.startswith("##") and not in_code else 0
        if level and keep_level and level <= keep_level:
            keep_level = None
        if level and not keep_level and wanted.lower() in line.lower():
            keep_level = level
        if keep_level:
            kept.append(line)
    return "\n".join(kept)


def skills(args):
    files = skill_files()
    if args.action == "list":
        for name, path in files.items():
            print(f"{name:10} {frontmatter(path.read_text())[0].get('description', '')}")
        return
    if args.name not in files:
        fail(f"no skill '{args.name}'", "unknown_skill", "ddp skills list")
    if args.action == "path":
        print(files[args.name])
        return
    _, text = frontmatter(files[args.name].read_text())
    if args.full:
        text += "\n\n" + (docs_root() / "agents.md").read_text()
    if args.filter:
        text = sections(text, args.filter) or f"no section matching '{args.filter}'"
    print(text.rstrip())


def doctor_checks(platform, device, app):
    """(ok, what, fix) for each step from the device to a frame with elements. Stops at the first failure."""
    if platform == "ios":
        if not shutil.which("xcrun"):
            yield False, "xcrun", "install Xcode and its command line tools"
            return
        devices = json.loads(run(["xcrun", "simctl", "list", "devices", "-j"]))["devices"]
        state = next((d["state"] for group in devices.values() for d in group if d["udid"] == device), None)
        yield state == "Booted", f"simulator {device} {state or 'not found'}", f"xcrun simctl boot {device}"
        if state != "Booted":
            return
        result = subprocess.run(["xcrun", "simctl", "get_app_container", device, app, "data"], capture_output=True, text=True)
        yield result.returncode == 0, f"app {app} installed", "build and install the Debug build"
        if result.returncode != 0:
            return
        endpoint_file = Path(result.stdout.strip(), IOS_DIR, "live.json")
        yield endpoint_file.exists(), "live endpoint written by AgentDDP.start()", \
            "launch the app (xcrun simctl launch); it must be a Debug build that calls AgentDDP.start() at launch"
        if not endpoint_file.exists():
            return
        endpoint = json.loads(endpoint_file.read_text())
        session = {"platform": "ios", "app": app, "device": device, "port": endpoint["port"], "token": endpoint["token"]}
    else:
        adb = adb_path()
        state = subprocess.run([adb, "-s", device, "get-state"], capture_output=True, text=True).stdout.strip()
        yield state == "device", f"emulator or device {device} {state or 'not found'}", "boot it, then adb devices"
        if state != "device":
            return
        installed = subprocess.run([adb, "-s", device, "shell", "pm", "path", app], capture_output=True, text=True)
        yield installed.returncode == 0 and installed.stdout.startswith("package:"), f"app {app} installed", \
            "install the debug APK (./gradlew installDebug)"
        if not installed.stdout.startswith("package:"):
            return
        debuggable = subprocess.run([adb, "-s", device, "shell", "run-as", app, "true"], capture_output=True, text=True)
        yield debuggable.returncode == 0, "debuggable build", "install the debug variant, not release"
        package = subprocess.run([adb, "-s", device, "shell", "dumpsys", "package", app], capture_output=True, text=True).stdout
        yield "dev.agentddp" in package, "agent-ddp library in the build", \
            'add debugImplementation(project(":agent-ddp")) and reinstall'
        running = subprocess.run([adb, "-s", device, "shell", "pidof", app], capture_output=True, text=True).stdout.strip()
        yield bool(running), "app running", f"adb -s {device} shell monkey -p {app} 1"
        if not running:
            return
        forwarded = subprocess.run([adb, "-s", device, "forward", "tcp:0", f"localabstract:agentddp.{app}"],
                                   capture_output=True, text=True)
        session = {"platform": "android", "app": app, "device": device, "port": int(forwarded.stdout.strip() or 0)}
    try:
        frame = next(m for m in LiveClient(session, {"cmd": "snapshot"}) if m["type"] == "frame")
    except Failure as failure:
        yield False, f"live endpoint answers ({failure})", "restart the app; then ddp connect"
        return
    finally:
        if platform == "android" and session.get("port"):
            subprocess.run([adb_path(), "-s", device, "forward", "--remove", f"tcp:{session['port']}"], capture_output=True)
    yield True, "live endpoint answers", ""
    yield bool(frame["elements"]), f"{len(frame['elements'])} elements on screen", \
        "iOS: the accessibility tree is empty, restart the app; Android: give what you need a testTag" \
        if platform == "ios" else "give the elements you need a testTag, text or contentDescription"


def doctor(args):
    session = json.loads(ctx.session_file.read_text()) if ctx.session_file.exists() else {}
    device, app = args.device or session.get("device") or os.environ.get("DDP_DEVICE"), args.app or session.get("app")
    if not (device and app):
        fail("doctor needs a device and an app", "missing_device", "ddp doctor DEVICE --app ID")
    platform = args.platform or ("ios" if len(device) == 36 and device.count("-") == 4 else "android")
    results = list(doctor_checks(platform, device, app))
    if ctx.last.exists():
        last = load(ctx.last)
        if last["frames"] and not last["frames"][-1]["elements"]:
            results.append((False, f"last recording {ctx.last} has no elements", "fix the steps above, then record again"))
    if args.json:
        print(json.dumps([{"ok": ok, "check": what, "fix": None if ok else fix} for ok, what, fix in results]))
    else:
        for ok, what, fix in results:
            print(f"{'ok  ' if ok else 'FAIL'} {what}" + ("" if ok else f"\n     fix: {fix}"))
    if not all(ok for ok, _, _ in results):
        sys.exit(1)


def batch(args):
    from .cli import run_command  # cli imports this module, so not at the top
    lines = args.lines or [line for line in sys.stdin.read().splitlines() if line.strip()]
    worst = 0
    for line in lines:
        print(f"$ ddp {line}", flush=True)
        code = run_command(shlex.split(line))
        worst = max(worst, code)
        if code and args.bail:
            break
    if worst:
        sys.exit(worst)
