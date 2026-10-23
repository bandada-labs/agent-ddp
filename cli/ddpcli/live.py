import json
import shlex
import socket
import subprocess
import sys
from pathlib import Path
from .state import ANDROID_DIR, IOS_DIR, OWNER_TOLERANCE_PT, adb_path, ctx, fail, parse_ms, run
from .model import (
    app_container,
    assign_owners,
    element_alpha,
    element_state,
    load,
    matches,
    name_elements,
    parse_where,
    save_recording,
)


class IOSDevice:
    def __init__(self, udid, app):
        self.udid, self.app = udid, app
        self.directory = Path(app_container(udid, app), IOS_DIR)

    def request(self, frames):
        self.directory.mkdir(parents=True, exist_ok=True)
        (self.directory / "recording.jsonl").unlink(missing_ok=True)
        (self.directory / "request.json").write_text(json.dumps({"frames": frames}))
        run(["xcrun", "simctl", "spawn", self.udid, "notifyutil", "-p", "dev.agentddp.record"])

    def fetch(self):
        recording = self.directory / "recording.jsonl"
        return recording.read_text() if recording.exists() else None


class AndroidDevice:
    def __init__(self, serial, app):
        self.adb, self.serial, self.app = adb_path(), serial, app

    def shell(self, *args):
        return subprocess.run([self.adb, "-s", self.serial, "exec-out", *args],
                              capture_output=True, text=True)

    def request(self, frames):
        self.shell("run-as", self.app, "rm", "-f", f"{ANDROID_DIR}/recording.jsonl")
        sent = self.shell("am", "broadcast", "-a", "dev.agentddp.RECORD", "-p", self.app,
                          "--ei", "frames", str(frames))
        if sent.returncode != 0:
            fail(f"broadcast failed: {sent.stderr.strip()}", "broadcast_failed", f"ddp doctor {self.serial} --app {self.app}")

    def fetch(self):
        result = self.shell("run-as", self.app, "cat", f"{ANDROID_DIR}/recording.jsonl")
        return result.stdout if result.returncode == 0 and result.stdout.startswith("{") else None


def run_then(command):
    """Runs the action without a shell: no pipes, globs or variables, only a program and its arguments."""
    try:
        return subprocess.run(shlex.split(command)).returncode == 0
    except (OSError, ValueError) as error:
        print(f"ddp: --then: {error}", file=sys.stderr)
        return False


def warn_if_empty(recording):
    if recording["frames"] and not any(f["elements"] for f in recording["frames"]):
        print("ddp: warning: no elements in this recording\nnext: ddp doctor", file=sys.stderr)


def live_session(args):
    """The saved `ddp connect` session, unless this command reads a file (offline mode)."""
    if getattr(args, "file", None) is not None or not ctx.session_file.exists():
        return None
    return json.loads(ctx.session_file.read_text())


def require_session():
    if not ctx.session_file.exists():
        fail("not connected", "not_connected", "ddp connect DEVICE --app ID")
    return json.loads(ctx.session_file.read_text())


class LiveClient:
    """One command on the app's live endpoint: a token line, a command line, then JSON lines back."""

    def __init__(self, session, command):
        try:
            self.socket = socket.create_connection(("127.0.0.1", session["port"]), timeout=10)
        except OSError as error:
            fail(f"can't reach the app ({error})", "unreachable",
                 f"is it running? ddp connect {session['device']} --app {session['app']}, or ddp doctor")
        self.session = session
        self.socket.sendall((json.dumps({"token": session.get("token", "")}) + "\n" + json.dumps(command) + "\n").encode())
        self.lines = self.socket.makefile("r", encoding="utf-8")

    def __iter__(self):
        answered = False
        for line in self.lines:
            if not line.strip():
                continue  # keep-alive
            answered = True
            message = json.loads(line)
            if message.get("type") == "error":
                fail(message["error"], "app_error")
            if message.get("type") == "end" and "frames" not in message:
                return
            yield message
            if message.get("type") == "end":
                return
        if not answered:
            # adb forward accepts the connection even when nothing listens in the app, then closes it.
            fail("the app closed the connection without answering", "unreachable",
                 f"is the app running? ddp doctor {self.session['device']} --app {self.session['app']}")

    def close(self):
        self.socket.close()


def connect(args):
    platform = args.platform or ("ios" if len(args.device) == 36 and args.device.count("-") == 4 else "android")
    if platform == "ios":
        endpoint_file = Path(app_container(args.device, args.app), IOS_DIR, "live.json")
        if not endpoint_file.exists():
            fail("the app has no live endpoint", "no_endpoint",
                 f"run a Debug build that calls AgentDDP.start(); ddp doctor {args.device} --app {args.app}")
        endpoint = json.loads(endpoint_file.read_text())
        session = {"platform": "ios", "app": args.app, "device": args.device,
                   "port": endpoint["port"], "token": endpoint["token"]}
    else:
        forwarded = subprocess.run([adb_path(), "-s", args.device, "forward", "tcp:0", f"localabstract:agentddp.{args.app}"],
                                   capture_output=True, text=True)
        if forwarded.returncode != 0:
            fail(f"adb forward failed: {forwarded.stderr.strip()}", "forward_failed", f"ddp doctor {args.device} --app {args.app}")
        session = {"platform": "android", "app": args.app, "device": args.device, "port": int(forwarded.stdout.strip())}
    snapshot = list(LiveClient(session, {"cmd": "snapshot"}))
    ctx.session_file.parent.mkdir(parents=True, exist_ok=True)
    ctx.session_file.write_text(json.dumps(session))
    frame = next(m for m in snapshot if m["type"] == "frame")
    print(f"connected to {args.app} on {platform} {args.device} (port {session['port']}): "
          f"{len(frame['elements'])} elements, {len(frame['nodes'])} nodes on screen")
    if not frame["elements"]:
        print("ddp: warning: no elements on screen\nnext: ddp doctor", file=sys.stderr)


def disconnect(args):
    if ctx.session_file.exists():
        session = json.loads(ctx.session_file.read_text())
        if session["platform"] == "android":
            subprocess.run([adb_path(), "-s", session["device"], "forward", "--remove", f"tcp:{session['port']}"],
                           capture_output=True)
        ctx.session_file.unlink()
    print(f"disconnected; commands read {ctx.last} (or --file) again")


def stream_frames(session, duration_ms, then=None):
    """Live frames for `duration_ms` (None: until the caller stops), running `then` once the stream starts."""
    client = LiveClient(session, {"cmd": "stream", "frames": 0})
    started_then = False
    try:
        for message in client:
            yield message
            if message["type"] != "frame":
                continue
            if then and not started_then:
                started_then = True
                if not run_then(then):
                    print("ddp: warning: --then command failed", file=sys.stderr)
            if duration_ms is not None and message["t"] >= duration_ms:
                return
    finally:
        client.close()


def record_live(args):
    """Records the live window the command asks for into the recording file and loads it."""
    session = live_session(args)
    duration = parse_ms(args.duration) if args.duration else (args.frames or 120) * 1000 / 60
    lines = list(stream_frames(session, duration, args.then))
    save_recording("\n".join(json.dumps(line) for line in lines) + "\n", getattr(args, "name", None))
    return load(ctx.last)


def snapshot_live(session):
    """One frame of what is on screen now. Not kept in the history: it is not a take."""
    ctx.last.parent.mkdir(parents=True, exist_ok=True)
    ctx.last.write_text("\n".join(json.dumps(m) for m in LiveClient(session, {"cmd": "snapshot"})) + "\n")
    return load(ctx.last)


def recording_for(args):
    """Live: record the window the command asks for. Offline: load --file (default .ddp/last.jsonl)."""
    if live_session(args):
        return record_live(args)
    return load(args.file or ctx.last)


def watch(args):
    session = require_session()
    clauses = parse_where(args.until) if args.until else None
    header, previous, tolerance = None, None, OWNER_TOLERANCE_PT
    duration = parse_ms(args.duration) if args.duration else None
    for message in stream_frames(session, duration, args.then):
        if message["type"] == "header":
            header = message
            tolerance = OWNER_TOLERANCE_PT * (header.get("scale", 1) if header.get("unit") == "px" else 1)
            print(f"watching {args.id} live, unit {header['unit']} (only changes are printed)")
            continue
        if message["type"] != "frame":
            continue
        name_elements(message)
        assign_owners(message, tolerance)
        state = watch_state(message, args.id)
        if previous is None:
            print(f"t={message['t']:.0f}ms " + "; ".join(f"{k} {v}" for k, v in state.items()
                                                          if v is not None or state["present"]), flush=True)
        elif changes := [f"{k} {previous.get(k)} -> {v}" for k, v in state.items() if previous.get(k) != v]:
            print(f"t={message['t']:.0f}ms " + "; ".join(changes), flush=True)
        previous = state
        if clauses and matches({**state, "t": message["t"]}, clauses):
            print(f"until met at t={message['t']:.0f}ms")
            return


WATCH_FIELDS = ("x", "y", "w", "h", "alpha", "fill", "border", "borderColor", "opacity", "draws", "pixels")


def watch_state(frame, key):
    state = element_state(frame, key)
    if state is None:
        # Every field, empty: `--until 'alpha>0.9'` is simply false until the element shows up.
        return {"present": False, **dict.fromkeys(WATCH_FIELDS)}
    x, y, w, h = state["frame"]
    return {"present": True, "x": x, "y": y, "w": w, "h": h, "alpha": round(element_alpha(frame, key), 2),
            "fill": state["fill"], "border": state["border"], "borderColor": state["borderColor"],
            "opacity": state["opacity"], "draws": state["draws"], "pixels": state["pixels"]}


def evaluate(args):
    session = require_session()
    for message in LiveClient(session, {"cmd": "eval", "id": args.id, "path": args.path}):
        print(json.dumps(message.get("value")))


def net(args):
    session = require_session()
    if args.action == "get":
        if not args.request_id:
            fail("net get needs a request id", "missing_option", "ddp net list")
        for message in LiveClient(session, {"cmd": "net-get", "id": args.request_id}):
            print(json.dumps(message, indent=2, sort_keys=True))
        return
    if args.action == "list":
        requests = list(LiveClient(session, {"cmd": "net"}))
        if not requests:
            print("no requests yet (the app reports them from its Debug network decorator)")
        for request in requests:
            print(net_line(request))
        return
    clauses = parse_where(args.until) if args.until else None
    client = LiveClient(session, {"cmd": "net-watch"})
    print("watching requests live", flush=True)
    try:
        for request in client:
            print(net_line(request), flush=True)
            if clauses and matches(request, clauses):
                print(f"until met: {request['id']}")
                return
    finally:
        client.close()


def time_control(args):
    session = require_session()
    command = {"cmd": "time", "action": args.action}
    if args.value is not None:
        command["value"] = parse_ms(args.value) if args.action == "seek" else float(args.value)
    for state in LiveClient(session, command):
        paused = "paused" if state["paused"] else f"running at rate {state['rate']}"
        since = f", {state['sincePauseMs']:.0f}ms from the pause point" if "sincePauseMs" in state else ""
        print(f"clock {paused}{since}")


def net_line(request):
    status = request.get("status", request.get("error", "failed"))
    return f"{request['id']} {request['method']} {status} {request['ms']:.0f}ms {request['responseBytes']}B {request['url']}"
