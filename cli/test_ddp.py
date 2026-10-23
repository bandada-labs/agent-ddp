"""Self-check for the ddp CLI logic on a synthetic recording: python3 cli/test_ddp.py"""
import json
import math
import subprocess
import sys
import tempfile
from pathlib import Path

CLI = Path(__file__).with_name("ddp")


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
    # Run inside the recording's folder: ddp keeps refs and history in ./.ddp.
    return subprocess.run([sys.executable, str(CLI), *args, "--file", str(recording)], cwd=recording.parent,
                          capture_output=True, text=True, check=True).stdout


def ddp_in(directory, *args, stdin=None):
    return subprocess.run([sys.executable, str(CLI), *args], cwd=directory, input=stdin, capture_output=True, text=True)


with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "r.jsonl")
    widths = [0, 2, 4, 2, 0]
    lines = [{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 5}]
    lines += [frame(i, w, banner=(i == 4)) for i, w in enumerate(widths)]
    lines.append({"type": "end", "frames": 5})
    recording.write_text("\n".join(json.dumps(line) for line in lines))

    summary = run("read", recording=recording)
    assert "border: 0 -> 0" in summary and "max 4" in summary and "TRANSIENT" in summary, summary
    assert "borderColor: None -> None, 3 values" in summary, summary
    assert "draws: 0 -> 0" in summary and "max 1" in summary, summary

    overlap = run("check", "overlap", recording=recording)
    assert "COVERED button" in overlap and "9/9" in overlap, overlap

    lines[-2] = frame(4, 0, banner=False)
    recording.write_text("\n".join(json.dumps(line) for line in lines))
    assert "no covered elements" in run("check", "overlap", recording=recording)

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


with tempfile.TemporaryDirectory() as directory:
    recording = Path(directory, "corners.jsonl")
    recording.write_text(corner_recording(clip_round=False))
    assert "OUTSIDE CORNERS card" in run("check", "corners", recording=recording)
    recording.write_text(corner_recording(clip_round=True))
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
    return subprocess.run([sys.executable, str(CLI), "check", "timeline", "--expect", str(cases_file),
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
    diffed = subprocess.run([sys.executable, str(CLI), "diff", str(recording), str(other), "--id", "glow",
                             "--ignore", "frame"], capture_output=True, text=True, check=True).stdout
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

    cut = run("tree", "--max-output", "40", "--content-boundaries", recording=recording)
    assert "[truncated: showing 40 of" in cut and "DDP_APP_CONTENT_START nonce=" in cut and "CONTENT_END" in cut, cut

    batch = ddp_in(directory, "batch", stdin="tree -i --file r.jsonl\nget nope --file r.jsonl\ncheck all --file r.jsonl\n")
    assert batch.returncode == 1 and batch.stdout.count("$ ddp ") == 3 and "overlap 2" in batch.stdout, batch.stdout
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
    lines = [{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": len(times)}]
    for i, (t, x) in enumerate(zip(times, xs)):
        lines.append({"type": "frame", "i": i, "t": t, "elements": [{"id": label, "frame": [x, 0, 40, 40]}],
                      "nodes": [{"z": 0, "kind": "layer", "fill": "#FF0000FF", "frame": [x, 0, 40, 40]}]})
    return "\n".join(json.dumps(line) for line in lines)


with tempfile.TemporaryDirectory() as directory:
    take = Path(directory, "move.jsonl")
    times = [i * 16 for i in range(80)]
    # Holds at 0, moves to 100 over 400 ms on the CSS ease-out curve, then holds.
    from importlib.machinery import SourceFileLoader
    curves = SourceFileLoader("ddp_curves", str(CLI)).load_module()
    xs = [0] * 10 + [round(100 * curves.bezier_progress(curves.EASINGS["easeOut"], u / 25), 2) for u in range(1, 26)] + [100] * 45
    take.write_text(motion_recording(times, xs))
    curve = run("get", "box", "--curve", recording=take)
    assert "x: 0 -> 100.0 over 400ms from 144ms, easeOut" in curve and "loose" not in curve, curve
    assert curves.sparkline([(0, 0), (50, 4), (100, 0)], width=3) == "▁█▁"
    assert curves.sparkline([(0, 1), (50, 0.2), (100, 1)], width=3) == "█▁█"  # a dip shows too
    assert "no jank" in run("check", "jank", recording=take)
    assert "everything settled" in run("check", "settle", recording=take)

    gaps = Path(directory, "gaps.jsonl")
    gaps.write_text(motion_recording([0, 16, 32, 48, 130, 146, 162, 178], [0] * 8))
    jank = ddp_in(directory, "check", "jank", "--file", "gaps.jsonl").stdout
    assert "JANK 1 hitches" in jank and "worst 82ms at t=48ms" in jank, jank

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
                {"id": "dark", "label": "Dark", "frame": [0, 140, 100, 20]}]
    nodes = [{"z": 0, "kind": "layer", "fill": "#FFFFFFFF", "frame": [0, 0, 400, 400], "own": -1},
             {"z": 1, "kind": "bitmap", "frame": [0, 100, 100, 20], "text": {"size": 14, "color": "#CCCCCCFF"}, "own": 3},
             {"z": 2, "kind": "bitmap", "frame": [0, 140, 100, 20], "text": {"size": 14, "color": "#222222FF"}, "own": 4},
             # Owned by nobody although it sits inside "dark": the recorder said so.
             {"z": 3, "kind": "layer", "fill": "#888888FF", "frame": [0, 140, 100, 20], "own": -1}]
    take.write_text("\n".join(json.dumps(line) for line in [
        {"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 1},
        {"type": "frame", "i": 0, "t": 0, "elements": elements, "nodes": nodes}]))
    a11y = ddp_in(directory, "check", "a11y", "--file", "a11y.jsonl").stdout
    assert "SMALL TARGET tiny: 20x20" in a11y and "chip" not in a11y, a11y
    assert "NO LABEL icon" in a11y and "LOW CONTRAST faint: 1.6:1" in a11y and "dark:" not in a11y, a11y
    assert "COVERED dark" in ddp_in(directory, "check", "overlap", "--file", "a11y.jsonl").stdout

    rows = [bytearray(b"\x10\x20\x30\xff" * 5) for _ in range(4)]
    png = curves.png_write(5, 4, rows)
    width, height, back = curves.png_read(png)
    assert (width, height) == (5, 4) and back == rows

    page = ddp_in(directory, "view", "page.html", "--file", "a11y.jsonl")
    assert page.returncode == 0 and '"tiny"' in Path(directory, "page.html").read_text(), page.stderr

    calls = "\n".join(json.dumps(m) for m in [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "ddp_tree", "arguments": {"args": "-i --file a11y.jsonl"}}},
    ]) + "\n"
    answers = [json.loads(line) for line in ddp_in(directory, "mcp", stdin=calls).stdout.splitlines()]
    assert any(tool["name"] == "ddp_tree" for tool in answers[1]["result"]["tools"])
    assert "@e1 tiny" in answers[2]["result"]["content"][0]["text"] and not answers[2]["result"]["isError"]

# --then runs without a shell, so "; touch" is an argument to echo, not a second command.
from importlib.machinery import SourceFileLoader
ddp = SourceFileLoader("ddp", str(CLI)).load_module()
with tempfile.TemporaryDirectory() as directory:
    marker = Path(directory, "injected")
    assert ddp.run_then(f"echo safe ; touch {marker}") and not marker.exists()
    assert not ddp.run_then("no-such-program-ddp")

print("ok")
