import json
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path
from .state import MAX_FRAMES, OWNER_TOLERANCE_PT, adb_path, ctx, fail, parse_ms, run, self_command
from .model import (
    assign_owners,
    element_alpha,
    element_key,
    load,
    name_elements,
    normalize_label,
    save_recording,
)
from .live import (
    AndroidDevice,
    IOSDevice,
    LiveClient,
    live_session,
    record_live,
    require_session,
    run_then,
    snapshot_live,
    stream_frames,
    warn_if_empty,
)
from .checks import RECORDING_CHECKS, check_parity
from .queries import changing_keys, save_refs


def record(args):
    if args.on:
        return record_on(args)
    if live_session(args):
        recording = record_live(args)
        print(f"recorded {len(recording['frames'])} frames live -> {ctx.last}" +
              (f" (saved as {args.name})" if args.name else ""))
        warn_if_empty(recording)
        return
    if not (args.platform and args.app):
        fail("not connected", "not_connected", "ddp connect DEVICE --app ID, or pass --platform and --app")
    device_id = args.device or os.environ.get("DDP_DEVICE")
    if not device_id:
        fail("no device", "missing_device", "pass --device (simulator UDID or adb serial) or set DDP_DEVICE")
    args.frames = args.frames or 120
    if not 1 <= args.frames <= MAX_FRAMES:
        fail(f"--frames must be 1..{MAX_FRAMES} (about 10 s)", "bad_frames", "record longer flows in parts")
    if args.then and args.frames < 60:
        # The action lands about 0.5 s after the request on a simulator.
        print("ddp: warning: an action from --then lands about 0.5 s after recording starts; "
              "record 120+ frames to see it", file=sys.stderr)
    device = (IOSDevice if args.platform == "ios" else AndroidDevice)(device_id, args.app)
    device.request(args.frames)
    if args.then and not run_then(args.then):
        print("ddp: warning: --then command failed, the recording may show no change", file=sys.stderr)
    # A recording runs at the display rate; allow slow frames plus app startup jitter.
    deadline = time.time() + args.frames / 30 + 10
    while time.time() < deadline:
        content = device.fetch()
        if content:
            save_recording(content, args.name)
            if args.file:
                args.file.parent.mkdir(parents=True, exist_ok=True)
                args.file.write_text(content)
            recording = load(args.file or ctx.last)
            print(f"recorded {len(recording['frames'])} frames -> {args.file or ctx.last} "
                  f"(capture cost per frame: average {recording['end'].get('averageCaptureMs')} ms, "
                  f"slowest {recording['end'].get('slowestCaptureMs')} ms)")
            warn_if_empty(recording)
            return
        time.sleep(0.25)
    fail("no recording arrived", "no_recording", f"is the app in the foreground? ddp doctor {device_id} --app {args.app}")


def target(args):
    """(platform, device, app) from the flags or the session."""
    session = json.loads(ctx.session_file.read_text()) if ctx.session_file.exists() else {}
    device = getattr(args, "device", None) or session.get("device") or os.environ.get("DDP_DEVICE")
    app = getattr(args, "app", None) or session.get("app")
    if not device:
        fail("no device", "missing_device", "ddp connect DEVICE --app ID, or pass --device")
    platform = getattr(args, "platform", None) or session.get("platform") or \
        ("ios" if len(device) == 36 and device.count("-") == 4 else "android")
    return platform, device, app


def adb(device, *command):
    return subprocess.run([adb_path(), "-s", device, *command], capture_output=True, text=True)


def tap(args):
    session = require_session()
    recording = snapshot_live(session)
    frame = recording["frames"][-1]
    element = next((e for e in frame["elements"] if element_key(e) == args.id), None)
    if element is None:
        fail(f"no element '{args.id}' on screen", "unknown_element", "ddp tree -i")
    x, y, w, h = element["frame"]
    if session["platform"] == "android":
        result = adb(session["device"], "shell", "input", "tap", str(round(x + w / 2)), str(round(y + h / 2)))
        if result.returncode != 0:
            fail(f"adb input tap failed: {result.stderr.strip()}", "tap_failed")
    else:
        name = element.get("id") or element.get("label")
        activated = next(m for m in LiveClient(session, {"cmd": "tap", "id": name}))["value"]
        if not activated:
            fail(f"'{args.id}' has no accessibility action", "tap_failed", "use agent-device for a real touch")
    print(f"tapped {args.id} at ({x + w / 2:.0f}, {y + h / 2:.0f})")


def wait(args):
    """Live: until an element is drawn, a label shows, or nothing changes for a while."""
    session = require_session()
    if not (args.by or args.settled):
        fail("wait needs `id <id>`, `text <label>` or --settled 300ms", "missing_option")
    settled_ms = parse_ms(args.settled) if args.settled else None
    timeout = parse_ms(args.timeout)
    header, last_change, previous = None, 0, None
    for message in stream_frames(session, timeout):
        if message["type"] == "header":
            header = message
            continue
        if message["type"] != "frame":
            continue
        name_elements(message)
        assign_owners(message, OWNER_TOLERANCE_PT * (header.get("scale", 1) if header.get("unit") == "px" else 1))
        t = message["t"]
        if args.by:
            found = next((e for e in message["elements"]
                          if (element_key(e) == args.value or e.get("id") == args.value if args.by == "id"
                              else normalize_label(args.value).lower() in (e.get("label") or "").lower())
                          and element_alpha(message, element_key(e)) >= 0.05), None)
            if found:
                print(f"{element_key(found)} drawn at t={t:.0f}ms, frame {found['frame']}")
                return
        else:
            ignored = set(args.ignore)
            state = json.dumps([[e for e in message["elements"] if element_key(e) not in ignored],
                                [{k: v for k, v in n.items() if k != "z"} for n in message["nodes"]
                                 if n["owner"] not in ignored]], sort_keys=True)
            if state != previous:
                previous, last_change = state, t
            elif t - last_change >= settled_ms:
                print(f"settled: nothing changed from t={last_change:.0f}ms to t={t:.0f}ms")
                return
    what = f"{args.by} {args.value}" if args.by else f"{args.settled} without change"
    fail(f"timed out after {args.timeout} waiting for {what}", "timeout",
         "ddp tree -i to see what is there, or a longer --timeout")


def logs(args):
    """The app's log lines during the last take (or --file), at their time in the take."""
    recording = load(args.file or ctx.last)
    started = recording["header"].get("startedAt")
    if not started:
        fail("this recording has no start time (an older library)", "no_start_time", "record again")
    end = started + recording["frames"][-1]["t"] + 500
    platform, device, app = target(args)
    if not app:
        fail("no app", "missing_option", "pass --app, or ddp connect first")
    lines = []
    if platform == "ios":
        info = run(["xcrun", "simctl", "appinfo", device, app])
        executable = next((line.split("=", 1)[1].strip().strip('";') for line in info.splitlines()
                           if "CFBundleExecutable" in line), app.split(".")[-1])
        start_text = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(started / 1000 - 1))
        output = run(["xcrun", "simctl", "spawn", device, "log", "show", "--info", "--style", "compact", "--start", start_text,
                      "--predicate", f'process == "{executable}"'])
        for line in output.splitlines():
            try:
                stamp = time.mktime(time.strptime(line[:19], "%Y-%m-%d %H:%M:%S")) * 1000 + float(line[19:23] or 0) * 1000
            except ValueError:
                continue
            if started - 100 <= stamp <= end:
                lines.append((stamp - started, line[24:].strip()))
    else:
        pid = adb(device, "shell", "pidof", app).stdout.strip()
        if not pid:
            fail(f"{app} is not running", "not_running", f"adb -s {device} shell monkey -p {app} 1")
        output = adb(device, "logcat", "-d", "-v", "epoch", f"--pid={pid}").stdout
        for line in output.splitlines():
            parts = line.split(None, 1)
            try:
                stamp = float(parts[0]) * 1000
            except (ValueError, IndexError):
                continue
            if started - 100 <= stamp <= end:
                lines.append((stamp - started, parts[1].strip()))
    if args.grep:
        lines = [(t, text) for t, text in lines if args.grep.lower() in text.lower()]
    if not lines:
        print(f"no log lines from {app} during the take")
        return
    for t, text in lines[:args.limit]:
        print(f"t={t:.0f}ms {text}")
    if len(lines) > args.limit:
        print(f"... {len(lines) - args.limit} more (--grep or --limit)")


ANDROID_TEXT_SIZES = {"small": "0.85", "default": "1.0", "large": "1.15", "largest": "1.3"}


def settings(args):
    platform, device, app = target(args)
    what, value = args.what, args.value
    if platform == "ios":
        commands = {
            "appearance": [["xcrun", "simctl", "ui", device, "appearance", value]],
            "text-size": [["xcrun", "simctl", "ui", device, "content_size", value]],
            "reduce-motion": [["xcrun", "simctl", "spawn", device, "defaults", "write", "com.apple.Accessibility",
                               "ReduceMotionEnabled", "-bool", "YES" if value == "on" else "NO"]],
            "locale": [["xcrun", "simctl", "spawn", device, "defaults", "write", app or "-g", "AppleLanguages",
                        "-array", value]],
        }
    else:
        scale = "0" if value == "on" else "1"
        commands = {
            "appearance": [[adb_path(), "-s", device, "shell", "cmd", "uimode", "night", "yes" if value == "dark" else "no"]],
            "text-size": [[adb_path(), "-s", device, "shell", "settings", "put", "system", "font_scale",
                           ANDROID_TEXT_SIZES.get(value, value)]],
            "reduce-motion": [[adb_path(), "-s", device, "shell", "settings", "put", "global", name, scale]
                              for name in ("animator_duration_scale", "transition_animation_scale", "window_animation_scale")],
            "locale": [[adb_path(), "-s", device, "shell", "cmd", "locale", "set-app-locales", app or "",
                        "--locales", value]],
        }
    for command in commands[what]:
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode != 0:
            fail(f"{' '.join(command[-4:])} failed: {result.stderr.strip()}", "set_failed")
    relaunch = what == "locale" or (what == "reduce-motion" and platform == "ios")
    print(f"{what} {value} on {platform} {device}" + ("; relaunch the app to see it" if relaunch else ""))


def open_url(args):
    platform, device, app = target(args)
    if platform == "ios":
        command = ["xcrun", "simctl", "openurl", device, args.url]
    else:
        command = [adb_path(), "-s", device, "shell", "am", "start", "-W", "-a", "android.intent.action.VIEW",
                   "-d", args.url, *(["-p", app] if app else [])]
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0 or "Error" in result.stdout:
        fail(f"could not open {args.url}: {(result.stderr or result.stdout).strip()}", "open_failed")
    print(f"opened {args.url} on {platform} {device}")
    if platform == "ios" and not args.url.startswith(("http://", "https://")):
        # A system alert, outside the app: ddp can't see it.
        print("iOS asks 'Open in <app>?' for a custom scheme: confirm it (agent-device press Open)")


def record_on(args):
    """The same take on several sessions at once (one per platform), then parity between the first two."""
    names = args.on.split(",")
    processes = {}
    for name in names:
        session_file = Path(".ddp", "sessions", name, "session.json")
        if not session_file.exists():
            fail(f"no session '{name}'", "not_connected", f"ddp connect DEVICE --app ID --session {name}")
        session = json.loads(session_file.read_text())
        command = [*self_command(), "record", "--session", name, "--name", f"take-{name}",
                   "--for", args.duration or "2s"]
        if args.then:
            # Only these three placeholders: a command may hold other braces (JSON).
            then = args.then
            for field in ("platform", "device", "app"):
                then = then.replace("{" + field + "}", str(session[field]))
            command += ["--then", then]
        processes[name] = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    for name, process in processes.items():
        print(f"{name}: {process.communicate()[0].strip()}")
    if any(process.returncode for process in processes.values()):
        sys.exit(1)
    if len(names) >= 2:
        takes = [Path(".ddp", "sessions", name, "saved", f"take-{name}.jsonl.gz") for name in names[:2]]
        args.file, args.against = takes
        check_parity(args)


# 3x5 digits, one row per string, for labels that match @eN refs.
DIGITS = ["111101101101111", "010110010010111", "111001111100111", "111001111001111", "101101111001001",
          "111100111001111", "111100111101111", "111001001001001", "111101111101111", "111101111001111"]


BOX_COLORS = [(230, 25, 75), (60, 180, 75), (0, 130, 200), (245, 130, 48), (145, 30, 180), (240, 50, 230)]


def png_read(data):
    """8-bit RGB or RGBA, not interlaced (what simctl and screencap write) to (width, height, rows of RGBA)."""
    import struct
    import zlib
    position, idat, width = 8, b"", 0
    while position < len(data):
        length, kind = struct.unpack(">I4s", data[position:position + 8])
        body = data[position + 8:position + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color = struct.unpack(">IIBB", body[:10])
            if depth != 8 or color not in (2, 6):
                fail("unsupported screenshot format", "bad_png")
            channels = 4 if color == 6 else 3
        elif kind == b"IDAT":
            idat += body
        position += 12 + length
    raw, stride = zlib.decompress(idat), width * channels
    rows, previous = [], bytearray(stride)
    for y in range(height):
        kind, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up, up_left = previous[i], previous[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + left) & 255
            elif kind == 2:
                line[i] = (line[i] + up) & 255
            elif kind == 3:
                line[i] = (line[i] + (left + up) // 2) & 255
            elif kind == 4:
                guess = left + up - up_left
                nearest = min((abs(guess - left), left), (abs(guess - up), up), (abs(guess - up_left), up_left))[1]
                line[i] = (line[i] + nearest) & 255
        previous = line
        rows.append(line if channels == 4 else
                    bytearray(b for i in range(0, stride, 3) for b in (*line[i:i + 3], 255)))
    return width, height, rows


def png_write(width, height, rows):
    import struct
    import zlib

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) +
            chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def paint(rows, x0, y0, x1, y1, color):
    height, width = len(rows), len(rows[0]) // 4
    for y in range(max(int(y0), 0), min(int(y1), height)):
        row = rows[y]
        for x in range(max(int(x0), 0), min(int(x1), width)):
            row[x * 4:x * 4 + 4] = bytes((*color, 255))


def annotate(args):
    session = require_session()
    recording = snapshot_live(session)
    refs = save_refs(recording["frames"])
    frame = recording["frames"][-1]
    out = Path(args.out)
    if session["platform"] == "ios":
        run(["xcrun", "simctl", "io", session["device"], "screenshot", "--type=png", str(out)])
        scale = recording["header"].get("scale", 1)
    else:
        out.write_bytes(subprocess.run([adb_path(), "-s", session["device"], "exec-out", "screencap", "-p"],
                                       capture_output=True, check=True).stdout)
        scale = 1
    width, height, rows = png_read(out.read_bytes())
    density = recording["header"].get("scale", 1)  # labels the same size in pt on both platforms
    thick, dot = max(2, round(density)), max(3, round(density * 1.5))
    for index, element in enumerate(frame["elements"]):
        color = BOX_COLORS[index % len(BOX_COLORS)]
        x, y, w, h = (v * scale for v in element["frame"])
        for edge in ((x, y, x + w, y + thick), (x, y + h - thick, x + w, y + h), (x, y, x + thick, y + h),
                     (x + w - thick, y, x + w, y + h)):
            paint(rows, *edge, color)
        number = refs[element_key(element)][2:]
        label_w, label_h = (len(number) * 4 + 1) * dot, 7 * dot
        paint(rows, x, y, x + label_w, y + label_h, color)
        for position, digit in enumerate(number):
            for bit, on in enumerate(DIGITS[int(digit)]):
                if on == "1":
                    cx, cy = x + (1 + position * 4 + bit % 3) * dot, y + (1 + bit // 3) * dot
                    paint(rows, cx, cy, cx + dot, cy + dot, (255, 255, 255))
    out.write_bytes(png_write(width, height, rows))
    print(f"{out}: {len(frame['elements'])} boxes, label N is @eN")
    for element in frame["elements"]:
        print(f"  {refs[element_key(element)]} {element_key(element)}")


def explore(args):
    """Live: tap each tappable element on the screen once, record what happens, run check all on each take.
    Prints one line per control; each take is saved as explore-<control> for a closer look.
    Stops when a tap leaves the screen (most of its elements gone), so the agent decides where to go next."""
    import io
    session = require_session()
    start = snapshot_live(session)["frames"][-1]
    start_keys = {element_key(e) for e in start["elements"]}
    tappable = [element_key(e) for e in start["elements"] if e.get("tap") and element_key(e) not in args.skip]
    if not tappable:
        fail("nothing tappable on screen", "nothing_to_explore", "ddp tree -i (the recorder marks controls with tap)")
    print(f"exploring {len(tappable)} controls: {', '.join(tappable)}", flush=True)
    session_flag = ["--session", args.session] if args.session else []
    for key in tappable:
        name = "explore-" + "".join(c if c.isalnum() else "-" for c in key)
        args.duration, args.frames, args.name, args.all_frames = args.per, None, name, True
        args.then = shlex.join([*self_command(), "tap", key, "--max-output", "0", *session_flag])
        recording = record_live(args)
        args.shared_recording = recording
        details, saved = io.StringIO(), sys.stdout
        sys.stdout = details  # the details stay in the saved take; one line per control here
        try:
            problems = {check_name: count for check_name, check in RECORDING_CHECKS.items()
                        if (count := check(args, recording))}
        finally:
            sys.stdout = saved
        changed = [k for k in changing_keys(recording["frames"]) if k != key]
        print(f"{key}: changed {', '.join(changed[:6]) or 'nothing else'}; " +
              (", ".join(f"{check_name} {count}" for check_name, count in problems.items()) if problems else "no problems") +
              f" (ddp check all --file {name})", flush=True)
        now = {element_key(e) for e in recording["frames"][-1]["elements"]}
        if len(start_keys & now) < len(start_keys) / 2:
            print(f"  {key} left the screen (new: {', '.join(sorted(now - start_keys)[:5])}); stopped. "
                  "Go back and run explore again with --skip, or explore the new screen")
            return
