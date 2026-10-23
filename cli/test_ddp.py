"""Self-check for the ddp CLI logic on a synthetic recording: python3 cli/test_ddp.py"""
import json
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
    return subprocess.run([sys.executable, str(CLI), *args, "--file", str(recording)],
                          capture_output=True, text=True, check=True).stdout


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

# --then runs without a shell, so "; touch" is an argument to echo, not a second command.
from importlib.machinery import SourceFileLoader
ddp = SourceFileLoader("ddp", str(CLI)).load_module()
with tempfile.TemporaryDirectory() as directory:
    marker = Path(directory, "injected")
    assert ddp.run_then(f"echo safe ; touch {marker}") and not marker.exists()
    assert not ddp.run_then("no-such-program-ddp")

print("ok")
