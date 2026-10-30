"""Conformance check for the ddp binary, from the outside: python3 cli/test_ddp.py
Runs cli/target/release/ddp (cargo build --release), or the program in env DDP_BIN."""
import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

DDP = [os.environ.get("DDP_BIN") or str(Path(__file__).resolve().parent / "target" / "release" / "ddp")]


def frame(i, border_width, banner):
    nodes = [
        {"z": 0, "kind": "layer", "fill": "#FFFFFFFF", "frame": [0, 0, 400, 800]},
        {"z": 1, "kind": "layer", "fill": "#0000FFFF", "frame": [10, 10, 100, 40]},
        {"z": 2, "kind": "bitmap", "frame": [20, 20, 80, 20]},
    ]
    if border_width:
        nodes.append({"z": 3, "kind": "layer", "border": {"width": border_width, "color": "#FF0000FF"},
                      "frame": [10, 10, 100, 40]})
    if border_width == 4:
        # A Canvas ring that only shows mid-animation.
        nodes.append({"z": 5, "kind": "draw", "op": "circle", "border": {"width": 3, "color": "#FF00FFFF"},
                      "frame": [20, 15, 30, 30]})
    if banner:
        nodes.append({"z": 4, "kind": "layer", "fill": "#888888FF", "frame": [0, 0, 400, 60]})
    return {"type": "frame", "i": i, "t": i * 16, "nodes": nodes,
            "elements": [{"id": "button", "frame": [10, 10, 100, 40]},
                         # Compose reports a button's text as its own element; it must not cover the button.
                         {"label": "Tap", "frame": [20, 20, 80, 20]}]}


def run(*args, recording):
    # Run inside the recording's folder: ddp keeps refs and history in ./.ddp. Checks that find problems
    # exit 1, so the exit code is not checked here; a crash shows as missing output in the asserts.
    result = subprocess.run([*DDP, *args, "--file", str(recording)], cwd=recording.parent,
                            capture_output=True, text=True)
    assert "Traceback" not in result.stderr, result.stderr
    return result.stdout


def ddp_in(directory, *args, stdin=None):
    return subprocess.run([*DDP, *args], cwd=directory, input=stdin, capture_output=True, text=True)


with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "r.jsonl")
    widths = [0, 2, 4, 2, 0]
    lines = [{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 5}]
    lines += [frame(i, w, banner=(i == 4)) for i, w in enumerate(widths)]
    lines.append({"type": "end", "frames": 5})
    recording.write_text("\n".join(json.dumps(line) for line in lines))

    # A library newer than the CLI: stop with the fix instead of misreading frames. No field means protocol 0.
    newer = Path(directory, "newer.jsonl")
    newer.write_text("\n".join(json.dumps(line) for line in [{**lines[0], "protocol": 99}, *lines[1:]]))
    refused = ddp_in(directory, "read", "--file", str(newer), "--json")
    assert refused.returncode == 1 and json.loads(refused.stdout)["code"] == "protocol_too_new", refused.stdout
    assert "agent-ddp@latest" in json.loads(refused.stdout)["next"], refused.stdout

    # Launch arguments are iOS only: Android refuses before it touches a device.
    no_args = ddp_in(directory, "launch", "emulator-5554", "--app", "dev.agentddp.sample", "--arg", "-Flag", "--json")
    assert no_args.returncode == 1 and json.loads(no_args.stdout)["code"] == "not_supported", no_args.stdout

    summary = run("read", recording=recording)
    assert "border: 0 -> 0" in summary and "max 4" in summary and "TRANSIENT" in summary, summary
    assert "borderColor: None -> None, 3 values" in summary, summary
    assert "draws: 0 -> 0" in summary and "max 1" in summary, summary

    # Pixel checks judge only what the platform answered at the end of a live take (an inspect line).
    assert "overlap not judged: the take has no screen check" in run("check", "overlap", recording=recording)

    def inspected(texts=(), corners=(), hit=()):
        return {"type": "inspect", "minimum": 44, "hit": list(hit), "texts": list(texts), "corners": list(corners)}

    # Android: a text whose color shows nowhere in its frame; iOS: the same color at its ink and clear points.
    hidden = {"key": "Tap", "rect": [20, 20, 80, 20], "colors": ["#000000FF"], "seen": [120], "screen": "#888888FF"}
    shape = {"key": "Tap", "rect": [20, 20, 80, 20], "ink": "#888888FF", "inkShare": 1, "clear": "#888888FF",
             "clearShare": 1, "inkLikeClear": 1}
    recording.write_text("\n".join(json.dumps(line) for line in lines + [inspected([hidden, shape])]))
    overlap = run("check", "overlap", recording=recording)
    assert "COVERED Tap: its text (#000000FF) shows nowhere in its frame" in overlap and \
        "COVERED Tap: the screen shows #888888FF where its text has ink" in overlap, overlap
    shown = [{**hidden, "seen": [3]}, {**shape, "ink": "#000000FF", "inkLikeClear": 0}]
    recording.write_text("\n".join(json.dumps(line) for line in lines + [inspected(shown)]))
    assert "no covered text" in run("check", "overlap", recording=recording)

# Parity: same button on both platforms; Android in px at scale 2, starts later, and is fainter.
def parity_recording(platform, unit, scale, start, fill, corner):
    lines = [{"type": "header", "platform": platform, "unit": unit, "scale": scale, "frames": 6}]
    for i in range(6):
        width = 2 * scale if 2 <= i <= 4 else 0
        node = {"z": 0, "kind": "layer", "fill": fill, "corner": corner, "frame": [0, 0, 100 * scale, 40 * scale]}
        if width:
            node["border"] = {"width": width, "color": fill}
        lines.append({"type": "frame", "i": i, "t": start + i * 16, "nodes": [node],
                      "elements": [{"id": "button", "frame": [0, 0, 100 * scale, 40 * scale]}]})
    return "\n".join(json.dumps(line) for line in lines)


with tempfile.TemporaryDirectory() as directory:
    ios, android = Path(directory, "ios.jsonl"), Path(directory, "android.jsonl")
    ios.write_text(parity_recording("ios", "pt", 3, 0, "#0000FFFF", 12))
    android.write_text(parity_recording("android", "px", 2, 500, "#0000FF80", 24))
    parity = run("check", "parity", "--against", str(android), recording=ios)
    assert "DIFF button" in parity and "peak alpha: ios 1.00, android 0.50" in parity, parity
    assert "timing" not in parity and "corner" not in parity, parity

# Corners: a hatch line at the card's left edge reaches past its rounded corners, unless a rounded clip cuts it.
def corner_recording(clip_round):
    line = {"z": 1, "kind": "draw", "op": "line", "border": {"width": 2, "color": "#888888FF"},
            "frame": [10, 10, 0, 40], "points": [10, 10, 10, 50]}
    if clip_round:
        line["clipRound"] = [10, 10, 100, 40, 12]
    nodes = [{"z": 0, "kind": "layer", "fill": "#EEEEEEFF", "corner": 12, "frame": [10, 10, 100, 40]}, line]
    lines = [{"type": "header", "platform": "android", "unit": "px", "scale": 1, "frames": 1},
             {"type": "frame", "i": 0, "t": 0, "nodes": nodes, "elements": [{"id": "card", "frame": [10, 10, 100, 40]}]}]
    return "\n".join(json.dumps(line) for line in lines)


def corner_place(near, behind):
    return {"type": "inspect", "hit": [], "texts": [], "corners": [
        {"key": "card", "z": 1, "point": [10, 10], "shape": [10, 10, 100, 40], "radius": 12, "color": "#888888FF",
         "near": near, "screen": "#888888FF", "behind": behind}]}


with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "corners.jsonl")
    # The geometry points at the place; the pixels decide.
    recording.write_text(corner_recording(clip_round=False) + "\n" + json.dumps(corner_place(0, "#FFFFFFFF")))
    corners = run("check", "corners", recording=recording)
    assert "OUTSIDE CORNERS card: #888888FF shows at (10, 10)" in corners and "the screen is #FFFFFFFF" in corners, corners
    # Clipped after all (the screen shows the background there), or the same color behind: nothing to report.
    for near, behind in ((90, "#FFFFFFFF"), (0, "#8A8A8AFF")):
        recording.write_text(corner_recording(clip_round=False) + "\n" + json.dumps(corner_place(near, behind)))
        assert "nothing outside rounded corners" in run("check", "corners", recording=recording)

# Timeline: a glow shows at 200..300ms after the tap (the first change, at t=100), at half alpha.
def timeline_recording():
    lines = [{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 30}]
    for i in range(30):
        t = i * 20
        nodes = [{"z": 0, "kind": "layer", "fill": "#0000FFFF" if t < 100 else "#00FF00FF", "frame": [0, 0, 50, 50]}]
        elements = [{"id": "button", "frame": [0, 0, 50, 50]}]
        if 300 <= t <= 400:
            nodes.append({"z": 1, "kind": "layer", "fill": "#FFFFFF80", "frame": [100, 0, 50, 50]})
            elements.append({"id": "glow", "frame": [100, 0, 50, 50]})
        lines.append({"type": "frame", "i": i, "t": t, "nodes": nodes, "elements": elements})
    return "\n".join(json.dumps(line) for line in lines)


def timeline(cases, recording):
    cases_file = Path(recording.parent, "cases.json")
    cases_file.write_text(json.dumps({"expect": cases}))
    return subprocess.run([*DDP, "check", "timeline", "--expect", str(cases_file),
                           "--file", str(recording)], capture_output=True, text=True)


with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "timeline.jsonl")
    recording.write_text(timeline_recording())
    on_time = timeline([{"id": "glow", "at": 200, "until": 300, "minAlpha": 0.4, "frame": [100, 0, 50, 50]}], recording)
    assert on_time.returncode == 0 and "ok glow" in on_time.stdout, on_time.stdout
    late = timeline([{"id": "glow", "at": 100}], recording)
    assert late.returncode == 1 and "LATE glow" in late.stdout and "100ms late" in late.stdout, late.stdout
    # The button is there before the first change (t=100); expected from the action on, it is on time.
    present = timeline([{"id": "button", "at": 0}], recording)
    assert present.returncode == 0 and "ok button" in present.stdout, present.stdout
    # Absent: the glow (label-less, by id) and a text match must not be drawn in a window.
    shown = timeline([{"id": "glow", "absent": True, "at": 0, "until": 400}], recording)
    assert shown.returncode == 1 and "SHOWN glow (absent 0..400ms): 6 frames, 200..300ms" in shown.stdout, shown.stdout
    clear = timeline([{"id": "glow", "absent": True, "at": 350}, {"text": "nothing like this", "absent": True, "at": 0}], recording)
    assert clear.returncode == 0 and "ok glow (absent from 350ms): not drawn" in clear.stdout, clear.stdout
    faint = timeline([{"id": "glow", "at": 200, "minAlpha": 0.9}, {"id": "halo", "at": 0}], recording)
    assert "MISSING glow" in faint.stdout and "MISSING halo" in faint.stdout, faint.stdout

# Queries over the timeline recording above: tree, get, find, diff.
with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "timeline.jsonl")
    recording.write_text(timeline_recording())
    tree = run("tree", "--at", "0.35s", recording=recording)
    assert "t=340ms" in tree and "glow [100, 0, 50, 50]" in tree and "fill #FFFFFF80" in tree, tree
    got = run("get", "glow", "--from", "280ms", "--to", "420", recording=recording)
    assert got.count("\n") == 4 and "absent" in got and "0.50" in got, got
    found = run("find", "--where", "fill~ffffff and alpha<0.6", "--between", "0", "1s", recording=recording)
    assert found.startswith("6 matches in 6 frames, t=300..400ms"), found
    other = Path(directory, "other.jsonl")
    other.write_text(timeline_recording().replace("#FFFFFF80", "#FFFFFFFF"))
    diffed = subprocess.run([*DDP, "diff", str(recording), str(other), "--id", "glow",
                             "--ignore", "frame"], cwd=directory, capture_output=True, text=True, check=True).stdout
    assert "alpha 0.5 vs 1.0; fill #FFFFFF80 vs #FFFFFFFF" in diffed, diffed

# Agent ergonomics: refs, filters, find, duplicate names, errors, output budget, batch, check all, gzip, diff, skills.
with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "r.jsonl")
    widths = [0, 2, 4, 2, 0]
    lines = [{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 5}]
    lines += [frame(i, w, banner=(i == 4)) for i, w in enumerate(widths)]
    for line in lines[1:]:
        # Two rows with the same label, and a zero-width space in one of them.
        line["elements"] += [{"label": "Open", "frame": [0, 100, 50, 20]}, {"label": "Op\u200ben", "frame": [0, 130, 50, 20]}]
    lines.append({"type": "end", "frames": 5})
    lines.append({"type": "inspect", "hit": [], "corners": [], "texts": [
        {"key": "Tap", "rect": [20, 20, 80, 20], "colors": ["#000000FF"], "seen": [120], "screen": "#888888FF"}]})
    recording.write_text("\n".join(json.dumps(line) for line in lines))

    interactive = run("tree", "-i", recording=recording)
    assert "@e1 button [10, 10, 100, 40]" in interactive and "z=" not in interactive, interactive
    assert "  @e2 Tap" in interactive, interactive  # nested under the button that holds it
    assert "Open [2]" in interactive, interactive
    assert ddp_in(directory, "get", "@e1", "--file", "r.jsonl").stdout.startswith("button:")
    assert "@e3 Open" in run("find", "text", "open", recording=recording)
    changed = run("tree", "-c", "-i", recording=recording)
    assert "button" in changed and "Open" not in changed, changed

    missing = ddp_in(directory, "get", "nope", "--file", "r.jsonl")
    assert missing.returncode == 1 and "next: ddp tree" in missing.stderr, missing.stderr
    as_json = json.loads(ddp_in(directory, "get", "nope", "--json", "--file", "r.jsonl").stdout)
    assert as_json["code"] == "unknown_element" and as_json["next"], as_json

    tree_json = json.loads(run("tree", "-i", "--json", recording=recording))
    assert tree_json["elements"][0]["ref"] == "@e1" and tree_json["elements"][1]["parent"] == "button", tree_json
    assert json.loads(run("find", "text", "open", "--json", recording=recording))[0]["key"] == "Open"
    rows = [json.loads(line) for line in run("get", "button", "--json", recording=recording).splitlines()]
    assert len(rows) == 5 and rows[2]["border"] == 4, rows
    checked = ddp_in(directory, "check", "overlap", "--json", "--file", "r.jsonl")
    assert checked.returncode == 1 and json.loads(checked.stdout)["ok"] is False, checked.stdout

    cut = run("tree", "--max-output", "40", "--content-boundaries", recording=recording)
    assert "[truncated: showing 40 of" in cut and "DDP_APP_CONTENT_START nonce=" in cut and "CONTENT_END" in cut, cut

    batch = ddp_in(directory, "batch", stdin="tree -i --file r.jsonl\nget nope --file r.jsonl\ncheck all --file r.jsonl\n")
    assert batch.returncode == 1 and batch.stdout.count("$ ddp ") == 3 and "overlap 1" in batch.stdout, batch.stdout
    bail = ddp_in(directory, "batch", "--bail", "get nope --file r.jsonl", "tree --file r.jsonl")
    assert bail.stdout.count("$ ddp ") == 1, bail.stdout

    import gzip
    with gzip.open(Path(directory, "r.jsonl.gz"), "wt") as archive:
        archive.write(recording.read_text())
    assert "TRANSIENT" in ddp_in(directory, "read", "--file", "r.jsonl.gz").stdout

    assert "are the same" in ddp_in(directory, "diff", "r.jsonl", "r.jsonl.gz").stdout
    other = Path(directory, "other.jsonl")
    other.write_text(recording.read_text().replace("#0000FFFF", "#00FF00FF"))
    screen = ddp_in(directory, "diff", "r.jsonl", "other.jsonl").stdout
    assert "-button" in screen and "+button" in screen and "#00FF00FF" in screen, screen

    skills = ddp_in(directory, "skills", "list").stdout
    assert "core" in skills and "reviewer" in skills, skills
    timeline_only = ddp_in(directory, "skills", "get", "core", "--full", "--filter", "timeline").stdout
    assert "check timeline" in timeline_only and "## The loop" not in timeline_only, timeline_only

    # Output budget: a verbosity regression fails here. Tokens are about chars / 4.
    for command, budget in ((["tree", "-i"], 200), (["tree"], 450), (["read"], 1000), (["check", "all"], 450)):
        size = len(ddp_in(directory, *command, "--file", "r.jsonl").stdout)
        assert size <= budget, f"{command} printed {size} chars, budget {budget}"
        if "-v" in sys.argv:
            print(command, size)

# Curves, jank, settle, baselines.
def motion_recording(times, xs, label="box"):
    lines = [{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": len(times), "missed": True}]
    for i, (t, x) in enumerate(zip(times, xs)):
        lines.append({"type": "frame", "i": i, "t": t, "elements": [{"id": label, "frame": [x, 0, 40, 40]}],
                      "nodes": [{"z": 0, "kind": "layer", "fill": "#FF0000FF", "frame": [x, 0, 40, 40]}]})
    return "\n".join(json.dumps(line) for line in lines)


with tempfile.TemporaryDirectory() as directory:
    take = Path(directory, "move.jsonl")
    times = [i * 16 for i in range(80)]
    # Holds at 0, moves to 100 over 400 ms on the CSS ease-out curve, then holds.
    def ease_out(x):
        """y of the CSS ease-out curve (0, 0, 0.58, 1) at time x, by bisection."""
        def at(a, b, u):
            return 3 * a * u * (1 - u) ** 2 + 3 * b * u * u * (1 - u) + u ** 3
        low, high = 0.0, 1.0
        for _ in range(30):
            middle = (low + high) / 2
            low, high = (middle, high) if at(0, 0.58, middle) < x else (low, middle)
        return at(0, 1, (low + high) / 2)
    xs = [0] * 10 + [round(100 * ease_out(u / 25), 2) for u in range(1, 26)] + [100] * 45
    take.write_text(motion_recording(times, xs))
    curve = run("get", "box", "--curve", recording=take)
    assert "x: 0 -> 100.0 over 400ms from 144ms, matches easeOut (off by" in curve and "no named" not in curve, curve
    assert "no jank" in run("check", "jank", recording=take)
    assert "everything settled" in run("check", "settle", recording=take)

    # The library counts the refreshes the app missed (its own capture left out); the CLI only reads them.
    gaps = Path(directory, "gaps.jsonl")
    late = [json.loads(line) for line in motion_recording([0, 16, 32, 48, 130, 146], [0] * 6).splitlines()]
    late[5]["missed"] = 4
    gaps.write_text("\n".join(json.dumps(line) for line in late))
    jank = ddp_in(directory, "check", "jank", "--file", "gaps.jsonl").stdout
    assert "JANK the app missed 4 refreshes in one place, worst 4 before t=130ms" in jank, jank
    old = Path(directory, "old.jsonl")
    old.write_text(motion_recording([0, 16, 32, 48, 130, 146], [0] * 6).replace(', "missed": true', ""))
    assert "jank not judged" in ddp_in(directory, "check", "jank", "--file", "old.jsonl").stdout

    loop = Path(directory, "loop.jsonl")
    loop.write_text(motion_recording(times, [20 * math.sin(i / 3) for i in range(80)]))
    assert "NOT SETTLED box: x" in ddp_in(directory, "check", "settle", "--file", "loop.jsonl").stdout
    shaky = Path(directory, "shaky.jsonl")
    shaky.write_text(motion_recording(times, [0] * 40 + [0.4 * (i % 2) for i in range(40)]))
    assert "JITTER box: x shakes by 0.40pt" in ddp_in(directory, "check", "settle", "--file", "shaky.jsonl").stdout

    assert "baseline before" in ddp_in(directory, "baseline", "save", "before", "--file", "move.jsonl").stdout
    same = ddp_in(directory, "check", "baseline", "--against", "before", "--file", "move.jsonl")
    assert same.returncode == 0 and "0 of 1 elements changed" in same.stdout, same.stdout
    slower = Path(directory, "slower.jsonl")
    slower.write_text(motion_recording(times, [0] * 10 + [4 * u for u in range(1, 26)] + [100] * 45))
    changed = ddp_in(directory, "check", "baseline", "--against", "before", "--file", "slower.jsonl", "--junit", "j.xml")
    assert changed.returncode == 1 and "CHANGED box" in changed.stdout, changed.stdout
    assert '<testsuite name="ddp baseline before" tests="1" failures="1">' in Path(directory, "j.xml").read_text()

# Accessibility, owners from the recorder, the PNG codec behind annotate, view, mcp.
with tempfile.TemporaryDirectory() as directory:
    take = Path(directory, "a11y.jsonl")
    elements = [{"id": "tiny", "label": "Tiny", "tap": True, "frame": [0, 0, 20, 20]},
                {"id": "chip", "label": "Chip", "tap": True, "frame": [0, 40, 60, 32], "touch": [0, 32, 60, 48]},
                {"id": "icon", "tap": True, "frame": [100, 0, 48, 48]},
                {"id": "faint", "label": "Faint", "frame": [0, 100, 100, 20]},
                {"id": "dark", "label": "Dark", "frame": [0, 140, 100, 20]},
                {"id": "onGradient", "label": "Over a gradient", "frame": [200, 0, 100, 20]}]
    nodes = [{"z": 0, "kind": "layer", "fill": "#FFFFFFFF", "frame": [0, 0, 400, 400], "own": -1},
             {"z": 1, "kind": "bitmap", "frame": [0, 100, 100, 20], "text": {"size": 14, "color": "#CCCCCCFF"}, "own": 3},
             {"z": 2, "kind": "bitmap", "frame": [0, 140, 100, 20], "text": {"size": 14, "color": "#222222FF"}, "own": 4},
             {"z": 4, "kind": "gradient", "gradient": ["#FFFFFFFF", "#000000FF"], "frame": [200, 0, 100, 20], "own": -1},
             {"z": 5, "kind": "bitmap", "frame": [200, 0, 100, 20], "text": {"size": 14, "color": "#777777FF"}, "own": 5},
             # Owned by nobody although it sits inside "dark": the recorder said so.
             {"z": 3, "kind": "layer", "fill": "#888888FF", "frame": [0, 140, 100, 20], "own": -1}]
    frame_line = {"type": "frame", "i": 0, "t": 0, "elements": elements, "nodes": nodes}
    header = {"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 1}
    take.write_text("\n".join(json.dumps(line) for line in [header, frame_line]))
    a11y = ddp_in(directory, "check", "a11y", "--file", "a11y.jsonl").stdout
    # Labels come from the accessibility tree; touch size and contrast only from what the platform answered.
    assert "NO LABEL icon" in a11y and "touch targets and contrast not judged" in a11y and "SMALL" not in a11y, a11y
    inspect = {"type": "inspect", "minimum": 44, "corners": [],
               # How far a touch reaches, from the app's hit test; tappable reports a control no touch reaches.
               "hit": [{**elements[0], "points": 9, "reach": [20, 20]}, {**elements[1], "points": 9},
                       {"id": "hidden", "label": "Hidden", "tap": True, "points": 0, "reason": "covered", "reach": [0, 0]},
                       {"id": "edge", "label": "Edge", "tap": True, "points": 9, "reach": [43.5, 44]}],
               "texts": [
                   {"key": "faint", "rect": [0, 100, 100, 20], "size": 14, "colors": ["#CCCCCCFF"], "seen": [0],
                    "screen": "#FFFFFFFF", "background": "#FFFFFFFF", "uniform": 1},
                   {"key": "dark", "rect": [0, 140, 100, 20], "size": 14, "colors": ["#222222FF"], "seen": [0],
                    "screen": "#FFFFFFFF", "background": "#FFFFFFFF", "uniform": 1},
                   {"key": "onGradient", "rect": [200, 0, 100, 20], "size": 14, "colors": ["#777777FF"], "seen": [0],
                    "screen": "#FFFFFFFF", "background": "#000000FF", "uniform": 0.6},
                   # iOS: the screen's colors at the text's own ink and clear points; its size is an estimate.
                   {"key": "Tiny", "rect": [0, 0, 20, 20], "size": 17, "estimated": True, "ink": "#AAAAAAFF",
                    "inkShare": 1, "clear": "#FFFFFFFF", "clearShare": 1, "inkLikeClear": 0}]}
    take.write_text("\n".join(json.dumps(line) for line in [header, frame_line, inspect]))
    a11y = ddp_in(directory, "check", "a11y", "--file", "a11y.jsonl").stdout
    assert "SMALL TARGET tiny: a touch reaches it over 20x20" in a11y and "chip" not in a11y, a11y
    assert "hidden" not in a11y and "Edge" not in a11y, a11y
    assert "contrast not judged (covered, more than one color, or over an image or a gradient): onGradient" in a11y, a11y
    assert "LOW CONTRAST faint: 1.6:1, #CCCCCCFF on #FFFFFFFF, needs 4.5:1" in a11y and "dark:" not in a11y, a11y
    assert "LOW CONTRAST Tiny: 2.3:1, #AAAAAAFF on #FFFFFFFF, needs 3:1 at least (its size is estimated)" in a11y, a11y
    # A hook that failed in the app run is said with every check, and is not a problem: the exit code stays.
    inspect["hooks"] = [{"name": "swiftui.responders", "kind": "internal", "status": "failing", "error": "no responders",
                         "feeds": "whose gesture takes a touch"}]
    take.write_text("\n".join(json.dumps(line) for line in [header, frame_line, inspect]))
    corners = ddp_in(directory, "check", "corners", "--file", "a11y.jsonl")
    assert corners.returncode == 0, corners.stdout
    assert ("hook swiftui.responders (internal) failing: no responders; not judged: whose gesture takes a touch "
            "(ddp doctor)") in corners.stdout, corners.stdout

    page = ddp_in(directory, "view", "page.html", "--file", "a11y.jsonl")
    assert page.returncode == 0 and '"tiny"' in Path(directory, "page.html").read_text(), page.stderr

    calls = "\n".join(json.dumps(m) for m in [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "ddp_tree", "arguments": {"args": "-i --file a11y.jsonl"}}},
    ]) + "\n"
    answers = [json.loads(line) for line in ddp_in(directory, "mcp", stdin=calls).stdout.splitlines()]
    assert any(tool["name"] == "ddp_tree" for tool in answers[1]["result"]["tools"])
    every = [json.loads(line) for line in ddp_in(directory, "mcp", "--tools", "all", stdin=calls).stdout.splitlines()]
    names = {tool["name"] for tool in every[1]["result"]["tools"]}
    assert {"ddp_launch", "ddp_state", "ddp_events", "ddp_storage", "ddp_perf", "ddp_crash", "ddp_swipe", "ddp_compare"} <= names, names
    assert "@e1 tiny" in answers[2]["result"]["content"][0]["text"] and not answers[2]["result"]["isError"]

# Live commands against a fake app: a WebSocket server that speaks the control endpoint's JSON-RPC.
import base64
import hashlib
import socket
import struct
import threading
import time as clock


class FakeApp:
    """Serves frames like the in-app library: a button; after a tap a toast fades in over 5 frames;
    a spinner that never stops. Frames are 16 ms apart in app time and sent in real time."""

    def __init__(self):
        self.tapped_at = None
        self.typed, self.prefs, self.interceptor, self.answers = "", {}, None, {}
        # App values: total grows by 10 on every read; mode is writable.
        self.total, self.mode = 0, "idle"
        self.touches = []
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen()
        self.port = self.server.getsockname()[1]
        threading.Thread(target=self.serve, daemon=True).start()

    # A tap sets both: the frame index in a stream, and the time, for `screen.frame` (what `ddp tap` polls).
    @property
    def tapped_at(self):
        return self._tapped_at

    @tapped_at.setter
    def tapped_at(self, index):
        self._tapped_at = index
        self.tapped_clock = None if index is None else clock.monotonic()

    def frame(self, i):
        elements = [{"id": "button", "label": "Go", "tap": True, "frame": [0, 0, 100, 50]},
                    {"id": "spinner", "frame": [0, 100, 20, 20]},
                    # Shows but a clear view on top takes its touches.
                    {"id": "locked", "label": "Locked", "tap": True, "frame": [0, 60, 100, 30]},
                    # Reached, but a parent takes the touch on its way.
                    {"id": "row", "label": "Row", "tap": True, "frame": [0, 130, 100, 30]}]
        nodes = [{"z": 0, "kind": "layer", "fill": "#0000FFFF", "frame": [0, 0, 100, 50]},
                 {"z": 1, "kind": "layer", "fill": "#000000FF", "frame": [0, 100, 20 + i % 3, 20]}]
        if self.tapped_at is not None:
            alpha = min(1, (i - self.tapped_at) / 5)
            if alpha > 0:
                elements.append({"id": "toast", "label": "Saved", "frame": [0, 200, 100, 30]})
                nodes.append({"z": 2, "kind": "layer", "fill": f"#00FF00{round(alpha * 255):02X}", "frame": [0, 200, 100, 30]})
        return {"type": "frame", "i": i, "t": i * 16, "elements": elements, "nodes": nodes}

    def serve(self):
        while True:
            connection, _ = self.server.accept()
            threading.Thread(target=self.session, args=(connection,), daemon=True).start()

    def session(self, connection):
        reader = connection.makefile("rb")
        headers = {}
        for line in iter(reader.readline, b"\r\n"):
            name, _, value = line.decode().partition(":")
            headers[name.strip().lower()] = value.strip()
        accept = base64.b64encode(hashlib.sha1(
            (headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        connection.sendall(f"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                           f"Sec-WebSocket-Accept: {accept}\r\n\r\n".encode())
        lock = threading.Lock()

        def send(message):
            payload = json.dumps(message).encode()
            size = bytes([len(payload)]) if len(payload) < 126 else b"\x7e" + struct.pack(">H", len(payload))
            with lock:
                connection.sendall(b"\x81" + size + payload)

        def watch():
            send({"method": "screen.stream", "params": {"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 0}})
            self.stream_index = 0
            try:
                while True:
                    send({"method": "screen.stream", "params": self.frame(self.stream_index)})
                    self.stream_index += 1
                    clock.sleep(0.016)
            except OSError:
                pass

        try:
            while True:
                head = reader.read(2)
                if len(head) < 2 or head[0] & 0x0f == 8:
                    return
                size = head[1] & 0x7f
                if size == 126:
                    size = struct.unpack(">H", reader.read(2))[0]
                mask = reader.read(4)
                message = json.loads(bytes(b ^ mask[i % 4] for i, b in enumerate(reader.read(size))))
                method, params = message["method"], message.get("params", {})
                result = {}
                if method == "app.info":
                    result = {"protocol": 2, "platform": "ios", "app": "fake", "name": "Fake", "unit": "pt", "scale": 3, "screen": [390, 844]}
                elif method == "screen.frame":
                    result = self.frame(0)
                    if self.tapped_clock is not None and clock.monotonic() - self.tapped_clock > 0.05:
                        result["elements"].append({"id": "toast", "label": "Saved", "frame": [0, 200, 100, 30]})
                elif method == "screen.hit":
                    misses = {"locked": ("covered", "scrim"), "row": ("intercepted", "BlockingLayout (onInterceptTouchEvent)")}
                    hits = [{**e, "points": 0, "reason": misses[e["id"]][0], "by": misses[e["id"]][1]} if e["id"] in misses else
                            {**e, "points": 9, "point": [50, 25]}
                            for e in self.frame(0)["elements"] if e.get("tap")]
                    result = {"elements": [h for h in hits if params.get("id") in (None, h["id"], h["label"])]}
                elif method == "screen.pixels":
                    # 4x2 pixels per rectangle: blue on top, white below. Answered like screen.pixels in both libraries.
                    pixels = base64.b64encode(bytes([0, 0, 255, 255] * 4 + [255, 255, 255, 255] * 4)).decode()
                    result = {"scale": 3, "regions": [{"x": 0, "y": 0, "width": 4, "height": 2, "rgba": pixels}
                                                      for _ in params["rects"]]}
                elif method == "input.touch":
                    self.touches.append((params["phase"], round(params["x"]), round(params["y"]), clock.monotonic()))
                    if params["phase"] == "down" and params["x"] < 100 and params["y"] < 50:
                        self.tapped_at = self.stream_index
                elif method == "screen.watch":
                    threading.Thread(target=watch, daemon=True).start()
                elif method == "input.type":
                    self.typed += params["text"]
                elif method == "input.key":
                    self.typed += f"<{params['key']}>"
                elif method == "prefs.read":
                    result = {"values": dict(self.prefs)}
                elif method == "prefs.write":
                    self.prefs[params["key"]] = params["value"]
                elif method == "prefs.remove":
                    self.prefs.pop(params["key"], None)
                elif method == "state.list":
                    result = {"values": [{"name": "mode", "writable": True}, {"name": "total", "writable": False}]}
                elif method == "state.get":
                    self.total += 10
                    values = {"mode": self.mode, "total": self.total}
                    result = {"values": {name: values[name] for name in params.get("names", values)}}
                elif method == "state.set":
                    if params["name"] != "mode":
                        send({"id": message["id"], "error": {"code": -32000, "message": f"'{params['name']}' is read-only: the app gave no setter"}})
                        continue
                    self.mode = params["value"]
                    result = {"value": self.mode}
                elif method == "storage.files":
                    result = {"path": params.get("path", ""), "entries": [{"name": "Documents", "folder": True},
                                                                         {"name": "note.txt", "folder": False, "bytes": 22}]}
                elif method == "storage.read":
                    result = {"bytes": 22, "truncated": False, "text": "hello from the sample"}
                elif method == "storage.query":
                    result = {"columns": ["name", "token"], "rows": [["first", "[redacted]"]], "more": True}
                elif method == "storage.keys":
                    result = {"keys": [], "unread": "the Keychain refused: the app has no Keychain entitlement (an unsigned build)"}
                elif method == "screen.highlight":
                    self.highlighted = params
                elif method == "screen.animations":
                    result = {"animations": [{"key": "spin", "type": "CABasicAnimation", "keyPath": "transform.rotation.z",
                                              "duration": 1, "repeat": "forever", "frame": [10, 10, 40, 40], "id": "spinner"}]}
                elif method == "screen.setValue":
                    self.set_value = params
                    result = {"path": params["path"], "value": params["value"]}
                elif method == "net.cookies":
                    result = {"cookies": [{"name": "session", "domain": "api.example.com", "path": "/", "secure": True, "expires": None}]}
                elif method == "media.list":
                    result = {"players": [{"source": "AVPlayerLayer", "state": "playing", "rate": 1, "position": 12.5,
                                           "duration": 60, "frame": [0, 0, 390, 220]}],
                              "nowPlaying": {"title": "Song", "position": 12.5}}
                elif method == "perf.metrics":
                    result = {"ms": params["ms"], "refreshMs": 16.67, "refreshes": 59, "missed": 3, "worstGapMs": 66.7,
                              "drawn": 20, "drawMeanMs": 6.1, "drawP95Ms": 12.4, "drawnOverRefresh": 1}
                elif method in ("events.list", "events.watch"):
                    events = [{"t": 1729700000000, "name": "rule", "fields": {"event": "save:expenses", "spent": 120}},
                              {"t": 1729700000500, "name": "sync", "fields": {"ok": True}}]
                    if method == "events.list":
                        result = {"events": events}
                    else:
                        for entry in events:
                            send({"method": "events.entry", "params": entry})
                elif method == "app.commands":
                    result = {"commands": ["load"]}
                elif method == "app.run":
                    # Waits, off the reading loop, for whoever intercepts: like the library's waiting methods.
                    threading.Thread(target=self.load, args=(message["id"], send), daemon=True).start()
                    continue
                elif method == "net.intercept":
                    self.interceptor = send
                elif method in ("net.respond", "net.fail"):
                    self.answers[params["id"]].set_result(params)
                elif method == "net.release":
                    self.interceptor = None
                send({"id": message["id"], "result": result})
        except OSError:
            pass  # the client hung up, as it does when it has enough
        finally:
            connection.close()

    stream_index = 0

    def load(self, id, send):
        status = 200
        if self.interceptor:
            import concurrent.futures
            answer = self.answers["f1"] = concurrent.futures.Future()
            self.interceptor({"method": "net.paused", "params": {"id": "f1", "request": {"method": "GET", "url": "http://x/items"}}})
            params = answer.result(timeout=5)
            status = params.get("status", params.get("error"))
        send({"id": id, "result": {"value": f"loaded {status}"}})


with tempfile.TemporaryDirectory() as directory:
    app = FakeApp()
    Path(directory, ".ddp").mkdir()
    Path(directory, ".ddp", "session.json").write_text(json.dumps(
        {"platform": "ios", "app": "fake", "device": "fake", "port": app.port}))
    live = ddp_in(directory, "tree", "-i")
    assert "@e1 button" in live.stdout and "@e2 spinner" in live.stdout, live.stdout + live.stderr
    tapped = ddp_in(directory, "tap", "@e1").stdout
    assert "tapped button at (50, 25)" in tapped and "new: toast" in tapped, tapped
    app.tapped_at = None
    # The app's hit test says a touch misses it: tap refuses, check tappable reports it, --force touches anyway.
    refused = ddp_in(directory, "tap", "locked")
    assert refused.returncode == 1 and "locked is covered by scrim" in refused.stderr, refused.stderr
    assert "tapped locked at (50, 75)" in ddp_in(directory, "tap", "locked", "--force").stdout
    intercepted = ddp_in(directory, "tap", "row")
    assert intercepted.returncode == 1 and \
        "row is taken first by a parent's BlockingLayout (onInterceptTouchEvent)" in intercepted.stderr, intercepted.stderr
    tappable = ddp_in(directory, "check", "tappable")
    assert tappable.returncode == 1 and "COVERED locked: covered by scrim" in tappable.stdout and \
        "INTERCEPTED row: taken first by a parent's BlockingLayout (onInterceptTouchEvent)" in tappable.stdout and \
        "1 of 3 controls take a touch" in tappable.stdout, tappable.stdout + tappable.stderr
    then_tap = f"{DDP[0]} tap button"
    waited = ddp_in(directory, "wait", "text", "saved", "--timeout", "2s")
    assert waited.returncode == 1 and "next:" in waited.stderr, waited.stderr  # nothing tapped yet: times out
    assert ddp_in(directory, "wait", "--settled", "100ms", "--timeout", "1s").returncode == 1  # the spinner never stops
    settled = ddp_in(directory, "wait", "--settled", "100ms", "--ignore", "spinner", "--timeout", "1s")
    assert settled.returncode == 0 and "settled" in settled.stdout, settled.stdout + settled.stderr
    watched = ddp_in(directory, "watch", "toast", "--until", "alpha>0.9", "--then", then_tap, "--for", "2s")
    assert "until met" in watched.stdout and "present False -> True" in watched.stdout, watched.stdout + watched.stderr
    app.tapped_at = None
    recorded = ddp_in(directory, "read", "--for", "1s", "--then", then_tap)
    assert "toast" in recorded.stdout and recorded.returncode == 0, recorded.stdout + recorded.stderr
    # A live `record` keeps the platform's answers, so a check on the file later judges instead of saying not judged.
    assert ddp_in(directory, "record", "--for", "300ms", "--name", "kept").returncode == 0
    from_file = ddp_in(directory, "check", "overlap", "--file", ".ddp/last.jsonl").stdout
    assert "not judged" not in from_file and ("no covered text" in from_file or "COVERED" in from_file), from_file
    app.tapped_at = None
    explored = ddp_in(directory, "explore", "--per", "1s")
    assert "locked: not tapped, covered by scrim" in explored.stdout and \
        "exploring 1 controls: button" in explored.stdout and "button: changed" in explored.stdout and \
        "toast" in explored.stdout, explored.stdout + explored.stderr
    # Two sessions at once, a placeholder and a JSON-looking argument in --then.
    for name in ("ios", "android"):
        Path(directory, ".ddp", "sessions", name).mkdir(parents=True)
        Path(directory, ".ddp", "sessions", name, "session.json").write_text(json.dumps(
            {"platform": name, "app": "fake", "device": "fake", "port": app.port}))
    both = ddp_in(directory, "record", "--on", "ios,android", "--for", "300ms",
                  "--then", f'{sys.executable} -c "import sys; print(sys.argv[1:])" {{platform}} {{"k": 1}}')
    assert both.returncode == 0 and "['ios', '{k:', '1}']" in both.stdout and "parity" in both.stdout, both.stdout + both.stderr
    # compare: the same element on two sessions differs by nothing; a reference all blue differs in the bottom half.
    across = ddp_in(directory, "compare", "button", "--on", "ios,android", "--out", str(Path(directory, "across.png")))
    assert across.returncode == 0 and "button on ios 4x2 px against android 4x2 px" in across.stdout and \
        "difference 0.0% (pixels that differ: 0.0%)" in across.stdout, across.stdout + across.stderr
    assert Path(directory, "across.png").read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"
    import zlib
    def png(width, height, rgb):
        raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
        chunk = lambda kind, body: struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
        return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + \
            chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    Path(directory, "blue.png").write_bytes(png(8, 4, [0, 0, 255]))
    against = ddp_in(directory, "compare", "button", "--reference", str(Path(directory, "blue.png")),
                     "--out", str(Path(directory, "against.png")))
    assert against.returncode == 0 and "difference 50.0% (pixels that differ: 50.0%)" in against.stdout and \
        "0-1       0.0%  differ 0.0%" in against.stdout and "1-2     100.0%  differ 100.0%" in against.stdout, against.stdout + against.stderr
    # What is left out counts in no score: the whole element here.
    ignored = ddp_in(directory, "compare", "button", "--reference", str(Path(directory, "blue.png")), "--ignore", "button",
                     "--out", str(Path(directory, "ignored.png")))
    assert "difference 0.0% (pixels that differ: 0.0%)" in ignored.stdout and \
        "left out: 100.0% of the pixels" in ignored.stdout, ignored.stdout + ignored.stderr
    same = ddp_in(directory, "compare", "--on", "ios,android", "--structure")
    assert same.returncode == 0 and "structure: ios 5 labeled elements, android 5; 5 on both" in same.stdout and \
        "the same labels on both" in same.stdout, same.stdout + same.stderr
    masked = ddp_in(directory, "compare", "button", "--on", "ios,android", "--mask", "0,0,10,10")
    assert masked.returncode == 1 and "--mask is for the whole screen" in masked.stderr, masked.stderr
    missing = ddp_in(directory, "compare", "button")
    assert missing.returncode == 1 and "compare needs --reference" in missing.stderr, missing.stderr
    # A bar only when the caller sets one: 50% differ passes --max 60 and fails --max 10 and --max-band 90.
    reference = str(Path(directory, "blue.png"))
    within = ddp_in(directory, "compare", "button", "--reference", reference, "--max", "60%")
    assert within.returncode == 0 and "within the bar" in within.stdout, within.stdout + within.stderr
    over = ddp_in(directory, "compare", "button", "--reference", reference, "--max", "10", "--max-band", "90")
    assert over.returncode == 1 and "over --max: 50.0% of the pixels differ, the bar is 10.0%" in over.stdout and \
        "over --max-band: band 1-2 differs 100.0%, the bar is 90.0%" in over.stdout, over.stdout + over.stderr
    wrong = ddp_in(directory, "compare", "button", "--reference", reference, "--max", "lots")
    assert wrong.returncode == 1 and "--max takes a percentage" in wrong.stderr, wrong.stderr
    # Two saved screenshots, devices off, from a folder that has no .ddp yet.
    with tempfile.TemporaryDirectory() as fresh:
        Path(fresh, "a.png").write_bytes(png(8, 4, [0, 0, 255]))
        Path(fresh, "b.png").write_bytes(png(16, 8, [0, 0, 255]))
        Path(fresh, "red.png").write_bytes(png(8, 4, [255, 0, 0]))
        saved = ddp_in(fresh, "compare", "--reference", "a.png", "--screen", "b.png", "--max", "1")
        assert saved.returncode == 0 and "b.png 16x8 against a.png 8x4 (scaled 2.0)" in saved.stdout and \
            "difference 0.0%" in saved.stdout and "within the bar" in saved.stdout, saved.stdout + saved.stderr
        assert Path(fresh, "ddp-compare.png").exists()
        red = ddp_in(fresh, "compare", "--reference", "a.png", "--screen", "red.png", "--max", "1")
        assert red.returncode == 1 and "over --max: 100.0%" in red.stdout, red.stdout + red.stderr
        live_only = ddp_in(fresh, "compare", "--reference", "a.png", "--screen", "b.png", "--ignore", "clock")
        assert live_only.returncode == 1 and "--ignore needs the live screen" in live_only.stderr, live_only.stderr
    # --then runs without a shell, so "; touch" is an argument to echo, not a second command.
    marker = Path(directory, "injected")
    safe = ddp_in(directory, "read", "--for", "200ms", "--then", f"echo safe ; touch {marker}")
    assert safe.returncode == 0 and not marker.exists(), safe.stdout + safe.stderr
    # Typing, preferences, app commands, and a mock that answers the request its own --then is waiting on.
    assert "typed 3 characters into button" in ddp_in(directory, "type", "Ana", "--into", "@e1").stdout
    assert "pressed Enter" in ddp_in(directory, "press", "Enter").stdout and app.typed == "Ana<Enter>", app.typed
    ddp_in(directory, "prefs", "set", "flag", "on")
    assert ddp_in(directory, "prefs").stdout == "flag = on\n"
    ddp_in(directory, "prefs", "rm", "flag")
    assert ddp_in(directory, "prefs").stdout == "no values\n"
    # Gestures: one finger on input.touch, with the timing each needs.
    app.touches, app.tapped_at = [], None
    point = ddp_in(directory, "tap", "50,25")
    assert "tapped (50, 25); changed: toast" in point.stdout and [t[0] for t in app.touches] == ["down", "up"], point.stdout + point.stderr
    app.touches = []
    swiped = ddp_in(directory, "swipe", "10,10", "90,40", "--for", "120ms")
    phases = [t[0] for t in app.touches]
    assert swiped.returncode == 0 and phases[0] == "down" and phases[-1] == "up" and phases.count("move") == 12, swiped.stdout + str(phases)
    assert app.touches[-2][1:3] == (90, 40) and "swipe 10,10 (10, 10) -> (90, 40)" in swiped.stdout, swiped.stdout
    app.touches = []
    ddp_in(directory, "tree", "-i")
    refs = ddp_in(directory, "swipe", "@e1", "@e2", "--for", "60ms")
    assert refs.returncode == 0 and "-> (10, 110)" in refs.stdout, refs.stdout + refs.stderr
    app.touches = []
    ddp_in(directory, "double-tap", "button")
    assert [t[0] for t in app.touches] == ["down", "up", "down", "up"] and app.touches[0][1:3] == (50, 25), app.touches
    app.touches = []
    ddp_in(directory, "long-press", "button", "--for", "300ms")
    assert [t[0] for t in app.touches] == ["down", "up"] and app.touches[1][3] - app.touches[0][3] >= 0.3, app.touches
    app.touches = []
    ddp_in(directory, "drag", "button", "0,200", "--for", "120ms")
    assert app.touches[1][3] - app.touches[0][3] >= 0.5 and app.touches[-1][0:3] == ("up", 0, 200), app.touches
    # Without the library: Android only, points only, and never a double tap.
    ios_host = ddp_in(directory, "tap", "10,10", "--device", "5A9A96DD-128D-475C-B007-46F50ED266A7")
    assert ios_host.returncode == 1 and "no public way to touch a simulator" in ios_host.stderr, ios_host.stderr
    double = ddp_in(directory, "double-tap", "10,10", "--device", "emulator-5554")
    assert double.returncode == 1 and "a double tap needs the library" in double.stderr, double.stderr
    element = ddp_in(directory, "swipe", "button", "0,0", "--device", "emulator-5554")
    assert element.returncode == 1 and "knows no elements" in element.stderr, element.stderr
    # Host commands refuse what a platform has no public way to do, before running anything.
    sim, emu = "5A9A96DD-128D-475C-B007-46F50ED266A7", "emulator-5554"
    for args, said in [(("set", "orientation", "landscape", "--device", sim), "no public way to rotate"),
                       (("set", "location", "200,10", "--device", sim), "location is LAT,LON"),
                       (("push", "p.json", "--device", emu, "--app", "x"), "no local way to deliver a push"),
                       (("job", "7", "--device", sim, "--app", "x"), "only from the debugger"),
                       (("biometric", "enroll", "--device", emu), "only through Settings")]:
        refused = ddp_in(directory, *args)
        assert refused.returncode == 1 and said in refused.stderr, (args, refused.stderr)
    # Back on iOS is UIKit's edge swipe: from the very edge, across the middle of the screen.
    app.touches = []
    back = ddp_in(directory, "back")
    assert back.returncode == 0 and app.touches[0][0:3] == ("down", 1, 422) and app.touches[-1][0:3] == ("up", 273, 422), back.stdout + str(app.touches[:2])
    missing = ddp_in(directory, "swipe", "button")
    assert missing.returncode != 0, missing.stdout
    app.tapped_at = None
    # App values: list, set the writable one, refuse the read-only one, watch until a bar.
    listed = ddp_in(directory, "state").stdout
    assert "mode = idle (writable)" in listed and "total = " in listed and "total = 10 (writable)" not in listed, listed
    assert ddp_in(directory, "state", "set", "mode", "demo").stdout == "mode = demo\n"
    assert ddp_in(directory, "state", "get", "mode").stdout == "mode = demo\n"
    refused = ddp_in(directory, "state", "set", "total", "5")
    assert refused.returncode == 1 and "read-only" in refused.stderr, refused.stderr
    met = ddp_in(directory, "state", "watch", "total", "--until", ">100")
    assert met.returncode == 0 and "until met at" in met.stdout and "->" in met.stdout, met.stdout + met.stderr
    many = ddp_in(directory, "state", "watch", "total", "--until", ">600")
    printed = [line for line in many.stdout.splitlines() if "->" in line]
    assert len(printed) == 20 and "more changes, now" in many.stdout and "until met" in many.stdout, many.stdout
    unmet = ddp_in(directory, "state", "watch", "mode", "--until", "=never", "--for", "300ms")
    assert unmet.returncode == 1 and "until not met in 300ms: mode=never" in unmet.stdout, unmet.stdout + unmet.stderr
    assert ddp_in(directory, "storage").stdout == "Documents/\nnote.txt  22 bytes\n"
    assert ddp_in(directory, "storage", "read", "note.txt").stdout == "hello from the sample\n"
    assert "... 1 lines in all, 22 bytes" in ddp_in(directory, "storage", "read", "note.txt", "--limit", "0").stdout
    queried = ddp_in(directory, "storage", "query", "a.sqlite", "SELECT * FROM items").stdout
    assert queried == "name | token\nfirst | [redacted]\n... more rows: add LIMIT/OFFSET to see them\n", queried
    keys = ddp_in(directory, "storage", "keys").stdout
    assert keys.startswith("not all read: the Keychain refused") and "no keys" not in keys, keys
    assert ddp_in(directory, "storage", "read").returncode == 1
    shown = ddp_in(directory, "highlight", "button", "--for", "500ms")
    assert shown.stdout == "highlighted button [0, 0, 100, 50] for 500ms\n" and app.highlighted == {"rect": [0, 0, 100, 50], "ms": 500}, shown.stdout
    assert ddp_in(directory, "animations").stdout == "spinner transform.rotation.z CABasicAnimation 1s repeat forever [10, 10, 40, 40]\n"
    # --set sends a number as JSON and a color as text, and prints what the app reads back.
    assert ddp_in(directory, "eval", "button", "layer.cornerRadius", "--set", "12").stdout == "12\n"
    assert app.set_value == {"id": "button", "path": "layer.cornerRadius", "value": 12}, app.set_value
    ddp_in(directory, "eval", "button", "layer.backgroundColor", "--set", "#FF0000")
    assert app.set_value["value"] == "#FF0000", app.set_value
    assert ddp_in(directory, "net", "cookies").stdout == "api.example.com session/ (expires session secure)\n"
    media = ddp_in(directory, "media").stdout
    assert media == "AVPlayerLayer playing at 12.5s of 60s rate 1 [0, 0, 390, 220]\nnow playing: title=Song position=12.5\n", media
    # The same answers as JSON, for a script or an MCP client.
    assert json.loads(ddp_in(directory, "perf", "--for", "200ms", "--json").stdout)["missed"] == 3
    assert json.loads(ddp_in(directory, "state", "--json").stdout)[0] == {"name": "mode", "writable": True, "value": "demo"}
    assert json.loads(ddp_in(directory, "events", "--json").stdout)[1]["name"] == "sync"
    assert json.loads(ddp_in(directory, "storage", "--json").stdout)["entries"][1]["bytes"] == 22
    assert json.loads(ddp_in(directory, "media", "--json").stdout)["players"][0]["state"] == "playing"
    perf = ddp_in(directory, "perf", "--for", "1s")
    assert perf.returncode == 0 and perf.stdout == "1.0s: the main thread missed 3 of 59 refreshes (16.67 ms each), worst gap 66.7 ms; " \
        "20 frames drawn, the app's work per frame mean 6.1 ms, p95 12.4 ms, 1 over a refresh\n", perf.stdout + perf.stderr
    # App events: the kept ones, a filter, and a follow that ends with --for.
    events = ddp_in(directory, "events").stdout
    assert "rule event=save:expenses spent=120" in events and "sync ok=true" in events, events
    assert "sync" not in ddp_in(directory, "events", "--grep", "expenses").stdout
    followed = ddp_in(directory, "events", "--follow", "--for", "300ms")
    assert followed.returncode == 0 and "rule event=save:expenses" in followed.stdout, followed.stdout + followed.stderr
    assert ddp_in(directory, "run").stdout == "load\n" and ddp_in(directory, "run", "load").stdout == '"loaded 200"\n'
    mocked = ddp_in(directory, "net", "mock", "*items*", "--status", "503", "--for", "5s", "--limit", "1",
                    "--then", f"{DDP[0]} run load")
    assert mocked.returncode == 0 and '"loaded 503"' in mocked.stdout and "GET http://x/items -> 503" in mocked.stdout, \
        mocked.stdout + mocked.stderr
    failed = ddp_in(directory, "net", "fail", "*items*", "--error", "timeout", "--for", "5s", "--limit", "1",
                    "--then", f"{DDP[0]} run load")
    assert '"loaded timeout"' in failed.stdout and "1 request answered" in failed.stdout, failed.stdout + failed.stderr

# History: a closed (gzipped) minute and the open one, cut mid-line as when the app dies.
import gzip
import time as clock
with tempfile.TemporaryDirectory() as directory:
    history_dir = Path(directory, "history")
    history_dir.mkdir()
    start = int(clock.time() * 1000) - 70_000

    def screen(t, label, spinner_w):
        return {"type": "frame", "t": t, "nodes": [], "elements": [
            {"id": "pay", "label": "Pay", "frame": [0, 0, 100, 50]},
            {"label": label, "frame": [0, 100, 200, 30]},
            {"id": "spinner", "frame": [0, 200, spinner_w, 10]}]}

    first = [{"type": "header", "platform": "android", "unit": "px", "scale": 1, "frames": 0, "startedAt": start,
              "history": True}]
    first += [screen(t, "Total 10", 10 + t % 7) for t in range(0, 60_000, 2_000)]
    first.append({"type": "end", "frames": 30})
    Path(history_dir, f"{start}.jsonl.gz").write_bytes(gzip.compress("\n".join(map(json.dumps, first)).encode()))
    second = [{"type": "header", "platform": "android", "unit": "px", "scale": 1, "frames": 0,
               "startedAt": start + 60_000, "history": True},
              screen(1_100, "Total 12", 3),
              # Written when the finger lifts, stamped when it went down: before the frame it caused.
              {"type": "event", "kind": "tap", "t": 1_000, "x": 50, "y": 25}]
    Path(history_dir, f"{start + 60_000}.jsonl").write_text("\n".join(map(json.dumps, second)) + '\n{"type": "fr')
    listed = ddp_in(directory, "history", "--dir", str(history_dir))
    assert listed.returncode == 0, listed.stdout + listed.stderr
    assert "tap pay (50,25)" in listed.stdout and '"Total 10" -> "Total 12"' in listed.stdout, listed.stdout
    assert "left out: spinner" in listed.stdout, listed.stdout
    tap_time = next(line.split()[0] for line in listed.stdout.splitlines() if "tap pay" in line)
    taken = ddp_in(directory, "history", "at", tap_time, "--for", "1s", "--dir", str(history_dir))
    assert taken.returncode == 0 and "3 frames" in taken.stdout, taken.stdout + taken.stderr
    summary = ddp_in(directory, "read", "--file", ".ddp/last.jsonl").stdout
    assert "Total 12" in summary, summary
    assert "jank not judged" in ddp_in(directory, "check", "jank", "--file", ".ddp/last.jsonl").stdout
    # Inspect mode: the next tap a person makes, and the element under it in the frame before.
    waiting = subprocess.Popen([*DDP, "inspect", "--for", "5s", "--dir", str(history_dir)], cwd=directory,
                               stdout=subprocess.PIPE, text=True)
    clock.sleep(1)
    now = int(clock.time() * 1000)
    third = [{"type": "header", "platform": "android", "unit": "px", "scale": 1, "frames": 0, "startedAt": now, "history": True},
             screen(0, "Total 12", 3), {"type": "event", "kind": "tap", "t": 10, "x": 40, "y": 110}]
    Path(history_dir, f"{now}.jsonl").write_text("\n".join(map(json.dumps, third)) + "\n")
    inspected = waiting.communicate(timeout=10)[0]
    assert waiting.returncode == 0 and 'tapped Total 12 [0, 100, 200, 30] at (40, 110)' in inspected, inspected

print("ok")
