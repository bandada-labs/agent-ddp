import json
import math
from .state import fail
from .model import element_alpha, element_keys, element_state
from .live import recording_for


def describe_track(name, states, times):
    # Frames where the element is absent are skipped; a value of None (no fill) is a real value.
    present = [(t, s[name]) for t, s in zip(times, states) if s is not None]
    if len({json.dumps(v) for _, v in present}) <= 1:
        return None
    first, last = present[0][1], present[-1][1]
    note = ""
    if json.dumps(first) == json.dumps(last):
        note = "  TRANSIENT (same at start and end, different in between)"
    change_times = [t for i, (t, v) in enumerate(present)
                    if i > 0 and json.dumps(v) != json.dumps(present[i - 1][1])]
    window = f"changing {change_times[0]}..{change_times[-1]}ms"
    if all(isinstance(v, (int, float)) for _, v in present):
        peak_t, peak = max(present, key=lambda p: p[1])
        low_t, low = min(present, key=lambda p: p[1])
        return (f"  {name}: {first} -> {last} {sparkline(present)}, {window}, "
                f"min {low} @{low_t}ms, max {peak} @{peak_t}ms{note}")
    return f"  {name}: {first} -> {last}, {len(change_times) + 1} values, {window}{note}"


SPARK = "▁▂▃▄▅▆▇█"


def sparkline(points, width=12):
    """The shape of a numeric track in a few characters: per time slice, the value farthest from where the
    track started, so a dip shows as well as a peak."""
    start, end = points[0][0], points[-1][0]
    low, high = min(v for _, v in points), max(v for _, v in points)
    span = (end - start) or 1
    slices = [[] for _ in range(width)]
    for t, value in points:
        slices[min(int((t - start) / span * width), width - 1)].append(value)
    marks, last = [], points[0][1]
    for values in slices:
        last = max(values, key=lambda v: abs(v - points[0][1])) if values else last  # an empty slice repeats the last
        marks.append(SPARK[round((last - low) / ((high - low) or 1) * (len(SPARK) - 1))])
    return "".join(marks)


def read(args):
    recording = recording_for(args)
    frames = recording["frames"]
    times = [f["t"] for f in frames]
    keys = [args.id] if args.id else element_keys(frames)
    if args.id and args.id not in element_keys(frames):
        fail(f"no element '{args.id}'. Known: {', '.join(element_keys(frames))}", "unknown_element", "ddp tree, or ddp find text <part of a label>")
    if args.json:
        for frame in frames:
            states = {k: element_state(frame, k) for k in keys}
            print(json.dumps({"i": frame["i"], "t": frame["t"], "elements": states}))
        return
    header = recording["header"]
    print(f"{header['platform']} {len(frames)} frames over {times[-1]:.0f} ms, unit {header['unit']}")
    for key in keys:
        states = [element_state(f, key) for f in frames]
        if args.id:
            print_timeline(key, frames, states)
            continue
        last = next((s for s in reversed(states) if s), None)
        draws = f" draws {last['draws']}" if any(s and s["draws"] for s in states) else ""
        print(f"{key}: frame {last['frame']} fill {last['fill']} border {last['border']}{draws} "
              f"kinds {','.join(last['kinds']) or 'none'}")
        for name in ("frame", "drawn", "fill", "border", "borderColor", "opacity", "draws", "pixels", "label", "kinds"):
            line = describe_track(name, states, times)
            if line:
                print(line)


def print_timeline(key, frames, states):
    print(f"{key}: t(ms) drawn alpha fill border borderColor opacity draws pixels (repeated rows skipped)")
    previous = None
    for frame, state in zip(frames, states):
        row = "absent" if state is None else (
            f"{state['drawn']} {element_alpha(frame, key):.2f} {state['fill']} {state['border']} {state['borderColor']} "
            f"{state['opacity']} {state['draws']} {state['pixels'] or ''}".rstrip())
        if row != previous:
            print(f"  {frame['t']:>8} {row}")
        previous = row


# CSS / SwiftUI / Compose named timing curves as cubic Bezier control points.
EASINGS = {"linear": (0, 0, 1, 1), "easeIn": (0.42, 0, 1, 1), "easeOut": (0, 0, 0.58, 1),
           "easeInOut": (0.42, 0, 0.58, 1), "fastOutSlowIn": (0.4, 0, 0.2, 1)}


def bezier_progress(control, x):
    """y of a timing curve at time x (0..1), found by bisection on the curve's x."""
    x1, y1, x2, y2 = control

    def at(a, b, u):
        return 3 * a * u * (1 - u) ** 2 + 3 * b * u * u * (1 - u) + u ** 3

    low, high = 0.0, 1.0
    for _ in range(30):
        middle = (low + high) / 2
        low, high = (middle, high) if at(x1, x2, middle) < x else (low, middle)
    return at(y1, y2, (low + high) / 2)


def segments(points):
    """Moves of a numeric track: runs of change between holds, split again at a turn (rise then fall)."""
    runs, current = [], []
    for index in range(1, len(points)):
        moving = points[index][1] != points[index - 1][1]
        if moving:
            current = current or [points[index - 1]]
            current.append(points[index])
        elif current and all(points[j][1] == points[index][1] for j in range(index, min(index + 3, len(points)))):
            runs.append(current)
            current = []
    if current:
        runs.append(current)
    split = []
    for run in runs:
        start, end = run[0][1], run[-1][1]
        extreme = max(range(len(run)), key=lambda i: abs(run[i][1] - start))
        # Comes back to where it started (a flash): two moves, out and back.
        if 0 < extreme < len(run) - 1 and abs(end - start) < abs(run[extreme][1] - start) / 2:
            split += [run[:extreme + 1], run[extreme:]]
        else:
            split.append(run)
    return split


def describe_move(run):
    """`0 -> 4 over 400ms from 550ms, easeOut` or a spring with its overshoot."""
    (t0, v0), (t1, v1) = run[0], run[-1]
    duration = t1 - t0
    distance = v1 - v0
    text = f"{v0} -> {v1} over {duration:.0f}ms from {t0:.0f}ms"
    if not distance or duration <= 0 or len(run) < 4:
        return text
    progress = [((t - t0) / duration, (v - v0) / distance) for t, v in run]
    overshoot = max(p for _, p in progress) - 1
    if overshoot > 0.02:
        return f"{text}, spring (overshoot {overshoot:.0%})"
    fits = {name: math.sqrt(sum((bezier_progress(c, x) - y) ** 2 for x, y in progress) / len(progress))
            for name, c in EASINGS.items()}
    best = min(fits, key=fits.get)
    return f"{text}, {best}" + ("" if fits[best] < 0.05 else f" (loose fit {fits[best]:.2f})")


def numeric_tracks(recording, key):
    """x, y, w, h, alpha, border, opacity of one element per frame, in the recording's unit."""
    tracks = {name: [] for name in ("x", "y", "w", "h", "alpha", "border", "opacity")}
    for frame in recording["frames"]:
        state = element_state(frame, key)
        if state is None:
            continue
        values = dict(zip("xywh", state["frame"]), alpha=round(element_alpha(frame, key), 3),
                      border=state["border"], opacity=state["opacity"])
        for name, value in values.items():
            tracks[name].append((frame["t"], value))
    return tracks


def print_curves(recording, key):
    print(f"{key} moves (unit {recording['header']['unit']}):")
    found = False
    for name, points in numeric_tracks(recording, key).items():
        for run in segments(points):
            if abs(run[-1][1] - run[0][1]) < 0.05 and len(run) < 4:
                continue  # rounding noise, not a move
            found = True
            print(f"  {name}: {describe_move(run)}")
    if not found:
        print("  no numeric change")
