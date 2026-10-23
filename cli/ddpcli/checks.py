import gzip
import json
import math
import sys
from pathlib import Path
from .state import OWNER_TOLERANCE_PT, ctx, fail
from .model import (
    area,
    color_alpha,
    contains,
    element_alpha,
    element_key,
    element_keys,
    element_state,
    frame_at,
    load,
    node_alpha,
    paints_at,
    read_text,
    recording_path,
    with_origin,
)
from .live import recording_for
from .summary import numeric_tracks


def coverage(frame, element, tolerance=0):
    """Which of 9 sample points inside the element are painted over by something it does not own.
    Nodes of elements inside this one (a button's own label) count as its own."""
    key = element_key(element)
    inner_keys = {element_key(e) for e in frame["elements"]
                  if contains(element["frame"], e["frame"], tolerance)}
    own_top = max((n["z"] for n in frame["nodes"] if n["owner"] == key), default=None)
    if own_top is None:
        return None
    x, y, w, h = element["frame"]
    points = [(x + w * fx, y + h * fy) for fy in (0.2, 0.5, 0.8) for fx in (0.2, 0.5, 0.8)]
    covered, coverers = 0, {}
    for point in points:
        above = [n for n in frame["nodes"] if n["z"] > own_top and n["owner"] not in inner_keys and paints_at(n, point)]
        if above:
            covered += 1
            top = max(above, key=lambda n: n["z"])
            coverers[top["z"]] = top
    return covered, list(coverers.values())


def check_overlap(args, recording=None):
    recording = recording or recording_for(args)
    frames = recording["frames"] if args.all_frames else recording["frames"][-1:]
    reported = set()
    for frame in frames:
        for element in frame["elements"]:
            key = element_key(element)
            result = coverage(frame, element, recording["tolerance"])
            if not result or result[0] == 0 or key in reported:
                continue
            reported.add(key)
            covered, coverers = result
            print(f"COVERED {key} at t={frame['t']}ms: {covered}/9 sample points, frame {element['frame']}")
            for node in coverers:
                details = {k: node[k] for k in ("kind", "source", "fill", "frame", "owner") if node.get(k)}
                print(f"  by z={node['z']} {json.dumps(details)}")
    if not reported and recording is not getattr(args, "shared_recording", None):
        print("no covered elements")
    return len(reported)


def inside_rounded(point, rect, radius, tolerance):
    """Whether a point is inside a rounded rectangle, give or take the tolerance."""
    x, y, w, h = rect
    radius = min(radius, w / 2, h / 2)
    # Distance from the point to the inner rectangle the corner arcs are centered on.
    dx = max(x + radius - point[0], 0, point[0] - (x + w - radius))
    dy = max(y + radius - point[1], 0, point[1] - (y + h - radius))
    return math.hypot(dx, dy) <= radius + tolerance


def outline_points(node):
    """Points on the edge of what a node paints: line ends, 8 points round an oval, or a rect's corners."""
    if node.get("points"):
        p = node["points"]
        return [(p[i], p[i + 1]) for i in range(0, len(p), 2)]
    x, y, w, h = node["frame"]
    if node.get("op") in ("circle", "oval", "arc"):
        return [(x + w / 2 + w / 2 * math.cos(a), y + h / 2 + h / 2 * math.sin(a))
                for a in (i * math.pi / 4 for i in range(8))]
    # A rounded node's own corner is cut too: take the point on its arc at 45 degrees.
    inset = min(node.get("corner", 0), w / 2, h / 2) * (1 - math.sqrt(0.5))
    return [(x + inset, y + inset), (x + w - inset, y + inset), (x + inset, y + h - inset), (x + w - inset, y + h - inset)]


def clamp(point, rect):
    x, y, w, h = rect
    return (min(max(point[0], x), x + w), min(max(point[1], y), y + h))


def corner_breaks(frame, element, tolerance):
    """Nodes drawn inside an element's rounded shape that reach past its corners and are not clipped to them."""
    key = element_key(element)
    shapes = [n for n in frame["nodes"] if n["owner"] == key and n.get("corner") and (n.get("fill") or n.get("border"))]
    if not shapes:
        return []
    shape = max(shapes, key=lambda n: area(n["frame"]))
    sx, sy, sw, sh = shape["frame"]
    breaks = []
    for node in frame["nodes"]:
        x, y, w, h = node.get("visible", node["frame"])
        center_inside = sx <= x + w / 2 <= sx + sw and sy <= y + h / 2 <= sy + sh
        # The shape's own stroke, or anything as big as the shape (a page background), is not a draw inside it.
        if (node is shape or node["kind"] == "bitmap" or not center_inside
                or contains(node["frame"], shape["frame"], tolerance) or area(node["frame"]) > area(shape["frame"]) * 1.5):
            continue
        clip_round = node.get("clipRound")
        outside = [p for p in (clamp(p, (x, y, w, h)) for p in outline_points(node))
                   if not inside_rounded(p, shape["frame"], shape["corner"], tolerance)
                   and not (clip_round and not inside_rounded(p, clip_round[:4], clip_round[4], tolerance))]
        if outside:
            breaks.append((node, outside[0]))
    return breaks


def check_corners(args, recording=None):
    recording = recording or recording_for(args)
    frames = recording["frames"] if args.all_frames else recording["frames"][-1:]
    reported = set()
    for frame in frames:
        for element in frame["elements"]:
            key = element_key(element)
            if key in reported:
                continue
            breaks = corner_breaks(frame, element, recording["tolerance"] / OWNER_TOLERANCE_PT)
            if not breaks:
                continue
            reported.add(key)
            shape = max((n for n in frame["nodes"] if n["owner"] == key and n.get("corner")), key=lambda n: area(n["frame"]))
            print(f"OUTSIDE CORNERS {key} at t={frame['t']}ms: {len(breaks)} nodes reach past corner radius "
                  f"{shape['corner']} of {shape['frame']}")
            for node, point in breaks[:5]:
                details = {k: node[k] for k in ("kind", "op", "fill", "border", "frame", "points") if node.get(k)}
                print(f"  z={node['z']} at ({point[0]:.1f}, {point[1]:.1f}) {json.dumps(details)}")
    if not reported and recording is not getattr(args, "shared_recording", None):
        print("nothing outside rounded corners")
    return len(reported)


PARITY_TIMING_MS = 50   # three frames at 60 Hz


PARITY_ALPHA = 0.1


PARITY_PT = 1


def parity_profile(recording, key):
    """Timing (from the recording's first change), peak alpha, corner and shadow of one element, in pt."""
    header = recording["header"]
    to_pt = 1 / header.get("scale", 1) if header.get("unit") == "px" else 1
    frames = recording["frames"]
    states = [json.dumps(element_state(f, key), sort_keys=True) for f in frames]
    changes = [frames[i]["t"] for i in range(1, len(frames)) if states[i] != states[i - 1]]
    owned = [n for f in frames for n in f["nodes"] if n["owner"] == key]
    shadows = [n["shadow"] for n in owned if n.get("shadow")]
    return {
        "changes": (changes[0] - recording["origin"], changes[-1] - recording["origin"]) if changes else None,
        "alpha": max(map(node_alpha, owned), default=0),
        "corner": max((n.get("corner", 0) for n in owned), default=0) * to_pt,
        "shadow": max((s["radius"] for s in shadows), default=0) * to_pt,
        "shadowAlpha": max((s["opacity"] * color_alpha(s["color"]) for s in shadows), default=0),
    }


def parity_differences(a, b, names):
    found = []
    if (a["changes"] is None) != (b["changes"] is None):
        found.append(f"timing: changes in {names[0] if a['changes'] else names[1]} only")
    elif a["changes"] and any(abs(x - y) > PARITY_TIMING_MS for x, y in zip(a["changes"], b["changes"])):
        found.append(f"timing: {names[0]} {a['changes'][0]:.0f}..{a['changes'][1]:.0f}ms, "
                     f"{names[1]} {b['changes'][0]:.0f}..{b['changes'][1]:.0f}ms")
    for field, tolerance, unit in (("alpha", PARITY_ALPHA, ""), ("corner", PARITY_PT, "pt"),
                                   ("shadow", PARITY_PT, "pt radius"), ("shadowAlpha", PARITY_ALPHA, "")):
        if abs(a[field] - b[field]) > tolerance:
            label = "peak alpha" if field == "alpha" else "shadow alpha" if field == "shadowAlpha" else field
            found.append(f"{label}: {names[0]} {a[field]:.2f}{unit}, {names[1]} {b[field]:.2f}{unit}")
    return found


def check_parity(args):
    if not args.against:
        fail("check parity needs --against <other recording>", "missing_option")
    left, right = with_origin(load(args.file or ctx.last)), with_origin(load(args.against))
    names = (left["header"]["platform"], right["header"]["platform"])
    if names[0] == names[1]:
        names = (str(args.file), str(args.against))
    left_keys, right_keys = element_keys(left["frames"]), element_keys(right["frames"])
    print(f"parity {names[0]} vs {names[1]}: times from each recording's first change, sizes in pt")
    for key in left_keys:
        if key not in right_keys:
            continue
        found = parity_differences(parity_profile(left, key), parity_profile(right, key), names)
        print(f"DIFF {key}" if found else f"same {key}")
        for line in found:
            print(f"  {line}")
    for key, side in [(k, names[0]) for k in left_keys if k not in right_keys] + \
                     [(k, names[1]) for k in right_keys if k not in left_keys]:
        print(f"only in {side}: {key}")


def case_name(case):
    return case.get("id") or f'text "{case["text"]}"'


def case_matches(case, element):
    """By id (element id or label, exact) or by text (part of the label, any case)."""
    if "id" in case:
        return element_key(element) == case["id"]
    return case["text"].lower() in (element.get("label") or "").lower()


def shown_times(recording, case, to_pt):
    """Times (from the origin) of frames where the element is there, strong enough and in place,
    plus why it never was when the list is empty."""
    times, present, strong = [], False, False
    for frame in recording["frames"]:
        element = next((e for e in frame["elements"] if case_matches(case, e)
                        and element_alpha(frame, element_key(e)) >= case.get("minAlpha", 0.05)), None)
        if element is None:
            present = present or any(case_matches(case, e) for e in frame["elements"])
            continue
        present = strong = True
        if "frame" in case:
            actual = [v * to_pt for v in element["frame"]]
            if any(abs(a - b) > case.get("tolerancePt", 4) for a, b in zip(actual, case["frame"])):
                continue
        times.append(frame["t"] - recording["origin"])
    if times:
        return times, None
    why = "never in the recording" if not present else \
        f"there but never at alpha {case.get('minAlpha', 0.05)} or more" if not strong else \
        f"there but never at {case['frame']} (within {case.get('tolerancePt', 4)} pt)"
    return times, why


def absent_verdict(case, times, tolerance, recording_end):
    """A case that must not be drawn between `at` and `until` (default: the end of the recording).
    The window is narrowed by the tolerance at both ends, so a fade that ends on time is not counted."""
    start, end = case["at"] + tolerance, case.get("until", recording_end) - tolerance
    inside = [t for t in times if start <= t <= end]
    if not inside:
        return "ok", "not drawn"
    return "SHOWN", f"{len(inside)} frames, {inside[0]:.0f}..{inside[-1]:.0f}ms"


def timeline_verdict(case, times, why, tolerance, recording_start):
    if not times:
        return "MISSING", why
    problems = []
    # Shown from the first recorded frame: it may have been there all along, which is on time for a case
    # expected from the action on, and early only for one expected later.
    already_there = times[0] <= recording_start and case["at"] <= tolerance
    if times[0] > case["at"] + tolerance:
        problems.append(f"appears {times[0] - case['at']:.0f}ms late (at {times[0]:.0f}ms)")
    elif times[0] < case["at"] - tolerance and not already_there:
        problems.append(f"appears {case['at'] - times[0]:.0f}ms early (at {times[0]:.0f}ms)")
    if "until" in case:
        if times[-1] < case["until"] - tolerance:
            problems.append(f"gone {case['until'] - times[-1]:.0f}ms early (last at {times[-1]:.0f}ms)")
        elif times[-1] > case["until"] + tolerance:
            problems.append(f"stays {times[-1] - case['until']:.0f}ms too long (last at {times[-1]:.0f}ms)")
    if not problems:
        return "ok", f"{times[0]:.0f}..{times[-1]:.0f}ms"
    late = any("late" in problem or "too long" in problem for problem in problems)
    return ("LATE" if late else "EARLY"), "; ".join(problems)


def check_timeline(args):
    if not args.expect:
        fail("check timeline needs --expect <cases.json>", "missing_option", "ddp skills get core --filter timeline")
    expected = json.loads(args.expect.read_text())
    recording = recording_for(args)
    if expected.get("origin", "first-change") == "start":
        recording["origin"] = recording["frames"][0]["t"]
    else:
        with_origin(recording)
    header = recording["header"]
    to_pt = 1 / header.get("scale", 1) if header.get("unit") == "px" else 1
    tolerance = expected.get("toleranceMs", PARITY_TIMING_MS)
    print(f"timeline: times from the recording's {expected.get('origin', 'first-change')} "
          f"(t={recording['origin']:.0f}ms), tolerance {tolerance}ms")
    failed = 0
    recording_start = recording["frames"][0]["t"] - recording["origin"]
    recording_end = recording["frames"][-1]["t"] - recording["origin"]
    for case in expected["expect"]:
        times, why = shown_times(recording, case, to_pt)
        if case.get("absent"):
            verdict, detail = absent_verdict(case, times, tolerance, recording_end)
        else:
            verdict, detail = timeline_verdict(case, times, why, tolerance, recording_start)
            if verdict == "MISSING" and case["at"] > recording_end:
                detail += f"; the recording ends at {recording_end:.0f}ms, before this case (next: record longer with --for)"
        failed += verdict != "ok"
        window = f"{case['at']}..{case['until']}ms" if "until" in case else f"from {case['at']}ms"
        expectation = "absent" if case.get("absent") else "expected"
        print(f"{verdict} {case_name(case)} ({expectation} {window}): {detail}")
    if failed:
        sys.exit(1)


MIN_TARGET = {"ios": 44, "android": 48}  # pt / dp: Apple HIG and Material


def luminance(color):
    channels = [int(color[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    linear = [c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def over(color, background):
    """A translucent color composited over an opaque background."""
    alpha = int(color[7:9], 16) / 255
    mixed = [round(int(color[i:i + 2], 16) * alpha + int(background[i:i + 2], 16) * (1 - alpha)) for i in (1, 3, 5)]
    return "#" + "".join(f"{c:02X}" for c in mixed) + "FF"


def contrast(text_color, background):
    lighter, darker = sorted((luminance(text_color), luminance(background)), reverse=True)
    return (lighter + 0.05) / (darker + 0.05)


def background_under(frame, node):
    """The opaque color behind a node: the fills below it that hold its center, blended upwards.
    None when what is right behind it is not a plain fill."""
    x, y, w, h = node["frame"]
    center = (x + w / 2, y + h / 2)
    under = [n for n in frame["nodes"] if n["z"] < node["z"] and paints_at(n, center)]
    if under and not max(under, key=lambda n: n["z"]).get("fill"):
        return None  # text over a gradient, an image or a drawing: no single color to compare with
    below = [n for n in under if n.get("fill")]
    color = None
    for fill in sorted(below, key=lambda n: n["z"]):
        if color is None:
            if fill["fill"].endswith("FF"):
                color = fill["fill"]
        else:
            color = over(fill["fill"], color)
    return color


def check_a11y(args, recording=None):
    """Small touch targets, controls nobody can name, and text without enough contrast (WCAG AA)."""
    recording = recording or recording_for(args)
    header = recording["header"]
    to_pt = 1 / header.get("scale", 1) if header.get("unit") == "px" else 1
    minimum = MIN_TARGET.get(header["platform"], 44)
    problems = {}
    last = recording["frames"][-1]
    for element in last["elements"]:
        if not element.get("tap"):
            continue
        key = element_key(element)
        w, h = (v * to_pt for v in element.get("touch", element["frame"])[2:])
        if w < minimum - 0.5 or h < minimum - 0.5:
            problems[key] = f"SMALL TARGET {key}: {w:.0f}x{h:.0f}, under {minimum}x{minimum}"
        elif not element.get("label"):
            problems[key] = f"NO LABEL {key}: a control a screen reader can't name"
    worst = {}  # per element, the frame where its text is hardest to read
    unjudged = set()
    for frame in recording["frames"]:
        for node in frame["nodes"]:
            text = node.get("text") or {}
            key = node.get("owner") or f"text at {node['frame']}"
            if not text.get("color") or (key in problems and key not in worst):
                continue
            background = background_under(frame, node)
            if not background:
                unjudged.add(key)
                continue
            ratio = contrast(over(text["color"], background), background)
            size_pt = (text.get("size") or 0) * to_pt
            large = size_pt >= 18 or (size_pt >= 14 and text.get("weight", 400) >= 700)
            needed = 3 if large else 4.5
            if ratio < needed and ratio < worst.get(key, needed):
                worst[key] = ratio
                estimated = " (color read from pixels)" if text.get("estimated") else ""
                problems[key] = (f"LOW CONTRAST {key}: {ratio:.1f}:1 at t={frame['t']:.0f}ms, {text['color']} on "
                                 f"{background}, needs {needed}:1{estimated}")
    for line in problems.values():
        print(line)
    unjudged -= problems.keys()
    if recording is not getattr(args, "shared_recording", None):
        if not problems:
            print("no accessibility problems found")
        if unjudged:
            print(f"contrast not judged (text over a gradient, image or drawing): {', '.join(sorted(unjudged))}")
    return len(problems)


JANK_FACTOR = 3        # a frame interval 3x the usual one means at least two missed refreshes


SETTLE_WINDOW_MS = 500


JITTER_PT = 1


def check_jank(args, recording=None):
    """Missed refreshes from the frame timestamps the recorder already writes."""
    recording = recording or recording_for(args)
    if recording["header"].get("history"):
        if recording is not getattr(args, "shared_recording", None):
            print("jank not judged: a history take keeps only the frames that changed; record live for jank")
        return 0
    times = [f["t"] for f in recording["frames"]]
    intervals = sorted(b - a for a, b in zip(times, times[1:]))
    if not intervals:
        return 0
    usual = intervals[len(intervals) // 2]
    hitches = [(a, b - a) for a, b in zip(times, times[1:]) if b - a > usual * JANK_FACTOR]
    end = recording["end"]
    if hitches:
        worst = max(hitches, key=lambda h: h[1])
        print(f"JANK {len(hitches)} hitches over {times[-1] - times[0]:.0f}ms (usual frame {usual:.1f}ms), "
              f"worst {worst[1]:.0f}ms at t={worst[0]:.0f}ms: " +
              ", ".join(f"{t:.0f}ms+{gap:.0f}" for t, gap in hitches[:8]) + (" ..." if len(hitches) > 8 else ""))
        if end.get("averageCaptureMs") is not None:  # live takes end without the recorder's numbers
            print(f"  capture cost per frame: average {end['averageCaptureMs']} ms, slowest {end.get('slowestCaptureMs')} ms"
                  " (the recorder's own share)")
    elif recording is not getattr(args, "shared_recording", None):
        print(f"no jank: {len(times)} frames, usual frame {usual:.1f}ms")
    return len(hitches)


def check_settle(args, recording=None):
    """Elements still moving at the end of the take (an animation that never stops),
    or shaking back and forth by about a point (jitter)."""
    recording = recording or recording_for(args)
    frames = recording["frames"]
    to_pt = 1 / recording["header"].get("scale", 1) if recording["header"].get("unit") == "px" else 1
    if frames[-1]["t"] - frames[0]["t"] < 2 * SETTLE_WINDOW_MS:
        if recording is not getattr(args, "shared_recording", None):
            print(f"take too short to tell: settle needs {2 * SETTLE_WINDOW_MS}ms or more")
        return 0
    window_start = frames[-1]["t"] - SETTLE_WINDOW_MS
    problems = 0
    for key in element_keys(frames):
        for name, points in numeric_tracks(recording, key).items():
            tail = [v for t, v in points if t >= window_start]
            steps = [b - a for a, b in zip(tail, tail[1:]) if b != a]
            if len(steps) < 3:
                continue
            turns = sum(1 for a, b in zip(steps, steps[1:]) if (a > 0) != (b > 0))
            amplitude = (max(tail) - min(tail)) * (to_pt if name in ("x", "y", "w", "h", "border") else 1)
            if turns >= 2 and amplitude <= JITTER_PT and name in ("x", "y", "w", "h"):
                print(f"JITTER {key}: {name} shakes by {amplitude:.2f}pt, {turns} turns in the last {SETTLE_WINDOW_MS}ms")
            else:
                print(f"NOT SETTLED {key}: {name} still changes in the last {SETTLE_WINDOW_MS}ms "
                      f"({len(steps)} changes, {tail[0]} -> {tail[-1]}). A loop, or record longer to tell")
            problems += 1
            break  # one line per element
    if not problems and recording is not getattr(args, "shared_recording", None):
        print(f"everything settled in the last {SETTLE_WINDOW_MS}ms")
    return problems


BASELINE_DIR = Path("ddp-baselines")


def baseline(args):
    """Keep the last take as a baseline next to the code, so it can be committed and checked in CI."""
    source = recording_path(str(args.file)) if args.file else ctx.last
    if not source.exists():
        fail(f"{source} not found", "file_not_found", "ddp record first")
    BASELINE_DIR.mkdir(exist_ok=True)
    target = BASELINE_DIR / f"{args.name}.jsonl.gz"
    with gzip.open(target, "wt", encoding="utf-8") as archive:
        archive.write(read_text(source))
    print(f"baseline {args.name} -> {target} ({target.stat().st_size} bytes)")


def check_baseline(args):
    if not args.against:
        fail("check baseline needs --against <baseline name>", "missing_option", "ddp baseline save NAME, then --against NAME")
    path = BASELINE_DIR / f"{args.against}.jsonl.gz"
    path = path if path.exists() else recording_path(str(args.against))
    expected, actual = with_origin(load(path)), with_origin(recording_for(args))
    keys = element_keys(expected["frames"]) + [k for k in element_keys(actual["frames"]) if k not in element_keys(expected["frames"])]
    results = []
    for key in keys:
        first = None
        for frame in actual["frames"]:
            t = frame["t"] - actual["origin"]
            other = frame_at(expected["frames"], t + expected["origin"])
            found = state_differences(comparable_state(expected, other, key), comparable_state(actual, frame, key))
            if found:
                first = (t, found)
                break
        results.append((key, first))
        if first:
            print(f"CHANGED {key} from t={first[0]:.0f}ms: {'; '.join(first[1][:3])} (baseline vs now)")
    changed = sum(1 for _, first in results if first)
    print(f"baseline {args.against}: {changed} of {len(results)} elements changed")
    if args.junit:
        write_junit(args.junit, f"ddp baseline {args.against}",
                    [(key, f"from t={first[0]:.0f}ms: {'; '.join(first[1])}" if first else None) for key, first in results])
    if changed:
        sys.exit(1)


def write_junit(path, suite, cases):
    """cases: (name, failure message or None)."""
    from xml.sax.saxutils import quoteattr
    failures = sum(1 for _, message in cases if message)
    lines = [f'<testsuite name={quoteattr(suite)} tests="{len(cases)}" failures="{failures}">']
    for name, message in cases:
        if message:
            lines.append(f"  <testcase name={quoteattr(name)}><failure message={quoteattr(message)}/></testcase>")
        else:
            lines.append(f"  <testcase name={quoteattr(name)}/>")
    lines.append("</testsuite>")
    Path(path).write_text("\n".join(lines) + "\n")


def check_all(args):
    """Every check that needs only this recording, in one call: only problems are printed, exit 1 on any."""
    recording = recording_for(args)
    args.shared_recording = recording
    problems = {name: check(args, recording) for name, check in RECORDING_CHECKS.items()}
    found = {name: count for name, count in problems.items() if count}
    print(f"check all over {len(recording['frames'])} frames: " +
          (", ".join(f"{name} {count}" for name, count in found.items()) if found else "no problems") +
          f" (ran {', '.join(problems)})")
    if found:
        sys.exit(1)


def comparable_state(recording, frame, key):
    """An element's state in pt, so two platforms compare."""
    state = element_state(frame, key)
    if state is None:
        return {"present": False}
    to_pt = 1 / recording["header"].get("scale", 1) if recording["header"].get("unit") == "px" else 1
    return {"present": True, "frame": [round(v * to_pt, 1) for v in state["frame"]],
            "alpha": round(element_alpha(frame, key), 2), "fill": state["fill"],
            "border": round(state["border"] * to_pt, 1), "borderColor": state["borderColor"],
            "opacity": state["opacity"], "draws": state["draws"]}


def state_differences(a, b, ignored=()):
    found = []
    for field in (a.keys() | b.keys()) - set(ignored):
        left, right = a.get(field), b.get(field)
        tolerance = PARITY_ALPHA if field in ("alpha", "opacity") else PARITY_PT
        if isinstance(left, list) and isinstance(right, list):
            differs = any(abs(x - y) > tolerance for x, y in zip(left, right))
        elif isinstance(left, (int, float)) and isinstance(right, (int, float)) and not isinstance(left, bool):
            differs = abs(left - right) > (0 if field == "draws" else tolerance)
        else:
            differs = left != right
        if differs:
            found.append(f"{field} {left} vs {right}")
    return sorted(found)


RECORDING_CHECKS = {"overlap": check_overlap, "corners": check_corners, "jank": check_jank, "settle": check_settle,
                    "a11y": check_a11y}


CHECKS = {**RECORDING_CHECKS, "parity": check_parity, "timeline": check_timeline, "baseline": check_baseline,
          "all": check_all}
