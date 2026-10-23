import gzip
import json
import subprocess
import time
from pathlib import Path
from .state import KEPT_RECORDINGS, OWNER_TOLERANCE_PT, ctx, fail


def app_container(udid, app):
    """The app's data container. Cached per session: each `simctl` call costs about 0.2 s."""
    cache_file = ctx.dir / "cache.json"
    try:
        cache = json.loads(cache_file.read_text())
    except (OSError, ValueError):
        cache = {}
    key = f"{udid}/{app}"
    if key in cache and Path(cache[key]).exists():
        return cache[key]
    result = subprocess.run(["xcrun", "simctl", "get_app_container", udid, app, "data"], capture_output=True, text=True)
    if result.returncode != 0:
        fail(f"no app {app} on simulator {udid}: {result.stderr.strip()}", "app_not_installed",
             f"ddp doctor {udid} --app {app}")
    cache[key] = result.stdout.strip()
    ctx.dir.mkdir(parents=True, exist_ok=True)
    cache_file.write_text(json.dumps(cache))
    return cache[key]


def save_recording(text, name=None):
    """Writes the recording to last.jsonl and keeps a gzipped copy: the last KEPT_RECORDINGS in recordings/,
    or under saved/<name> when named (never pruned). Gzip makes them 25-55x smaller."""
    ctx.last.parent.mkdir(parents=True, exist_ok=True)
    ctx.last.write_text(text)
    if name:
        target = ctx.dir / "saved" / f"{name}.jsonl.gz"
    else:
        target = ctx.dir / "recordings" / f"{time.strftime('%Y%m%d-%H%M%S')}-{int(time.time() * 1000) % 1000:03d}.jsonl.gz"
    target.parent.mkdir(parents=True, exist_ok=True)
    with gzip.open(target, "wt", encoding="utf-8") as archive:
        archive.write(text)
    for old in history()[:-KEPT_RECORDINGS]:
        old.unlink()
    return target


def history():
    """Kept recordings, oldest first."""
    return sorted((ctx.dir / "recordings").glob("*.jsonl.gz"))


def recording_path(text):
    """A path, or the name of a saved recording."""
    path = Path(text)
    saved = ctx.dir / "saved" / f"{text}.jsonl.gz"
    return saved if not path.exists() and saved.exists() else path


def read_text(path):
    with path.open("rb") as raw:
        gzipped = raw.read(2) == b"\x1f\x8b"
    return gzip.open(path, "rt", encoding="utf-8").read() if gzipped else path.read_text()


ZERO_WIDTH = dict.fromkeys(map(ord, "﻿​‌‍⁠"))


def normalize_label(label):
    return label.translate(ZERO_WIDTH).replace(" ", " ") if label else label


def load(path):
    path = recording_path(str(path))
    if not path.exists():
        fail(f"{path} not found", "file_not_found", "ddp record, or pass --file")
    lines = [json.loads(line) for line in read_text(path).splitlines() if line.strip()]
    header = next((line for line in lines if line["type"] == "header"), None)
    if header is None:
        fail(f"{path} has no recording in it (no header line)", "bad_recording", "ddp record again")
    end = next((line for line in lines if line["type"] == "end"), {})
    frames = [line for line in lines if line["type"] == "frame"]
    tolerance = OWNER_TOLERANCE_PT * (header.get("scale", 1) if header.get("unit") == "px" else 1)
    for frame in frames:
        name_elements(frame)
        assign_owners(frame, tolerance)
    return {"header": header, "end": end, "frames": frames, "tolerance": tolerance, "path": path}


def name_elements(frame):
    """Normalized labels, and an index for elements that share a name in one frame: `Open`, `Open [2]`."""
    seen = {}
    for element in frame["elements"]:
        element["label"] = normalize_label(element.get("label"))
        name = element.get("id") or element.get("label") or "?"
        seen[name] = seen.get(name, 0) + 1
        element["key"] = name if seen[name] == 1 else f"{name} [{seen[name]}]"


def element_key(element):
    return element.get("key") or element.get("id") or element.get("label") or "?"


def contains(outer, inner, tolerance=0):
    ox, oy, ow, oh = outer
    ix, iy, iw, ih = inner
    return (ix >= ox - tolerance and iy >= oy - tolerance and
            ix + iw <= ox + ow + tolerance and iy + ih <= oy + oh + tolerance)


def area(rect):
    return rect[2] * rect[3]


def assign_owners(frame, tolerance):
    """A node belongs to the element whose subtree drew it, when the recorder knows (`own`, Android).
    Otherwise to the smallest element whose frame holds it. Others stay unowned."""
    elements = sorted(frame["elements"], key=lambda e: area(e["frame"]))
    for node in frame["nodes"]:
        if "own" in node:
            owner = frame["elements"][node["own"]] if 0 <= node["own"] < len(frame["elements"]) else None
        else:
            owner = next((e for e in elements if contains(e["frame"], node["frame"], tolerance)), None)
        node["owner"] = element_key(owner) if owner else None
    # Lookups by name, built once per frame: queries ask for one element in every frame.
    frame["by_key"] = {element_key(e): e for e in frame["elements"]}
    frame["by_owner"] = {}
    for node in frame["nodes"]:
        frame["by_owner"].setdefault(node["owner"], []).append(node)


def element_state(frame, key):
    element = frame["by_key"].get(key)
    if element is None:
        return None
    owned = frame["by_owner"].get(key, [])
    filled = [n for n in owned if n.get("fill")]
    bordered = [n for n in owned if n.get("border")]
    widest_border = max(bordered, key=lambda n: n["border"]["width"], default=None)
    drawn = union([n.get("visible", n["frame"]) for n in owned])
    return {
        "frame": element["frame"],
        "drawn": drawn,
        "label": element.get("label"),
        "fill": max(filled, key=lambda n: area(n["frame"]))["fill"] if filled else None,
        "border": widest_border["border"]["width"] if widest_border else 0,
        "borderColor": widest_border["border"]["color"] if widest_border else None,
        "opacity": min((n.get("opacity", 1) for n in owned), default=1),
        "kinds": sorted({n["kind"] for n in owned}),
        "draws": sum(1 for n in owned if n["kind"] == "draw"),
        # Canvas / drawingGroup on iOS: the main color its pixels show and their strongest alpha.
        "pixels": next(([n["pixels"]["colors"][0]["color"] if n["pixels"]["colors"] else None, n["pixels"]["peakAlpha"]]
                        for n in owned if n.get("pixels")), None),
    }


def union(rects):
    if not rects:
        return None
    left = min(r[0] for r in rects)
    top = min(r[1] for r in rects)
    right = max(r[0] + r[2] for r in rects)
    bottom = max(r[1] + r[3] for r in rects)
    return [round(left, 2), round(top, 2), round(right - left, 2), round(bottom - top, 2)]


def element_keys(frames):
    keys = []
    for frame in frames:
        for element in frame["elements"]:
            if element_key(element) not in keys:
                keys.append(element_key(element))
    return keys


PAINTS_INTERIOR = {"bitmap", "gradient", "opaque-drawing"}  # "draw" ops count only when filled


def paints_at(node, point):
    paints = node["kind"] in PAINTS_INTERIOR or node.get("fill")
    x, y, w, h = node.get("visible", node["frame"])
    return paints and x <= point[0] <= x + w and y <= point[1] <= y + h


def color_alpha(color):
    return int(color[7:9], 16) / 255 if color else 0


def node_alpha(node):
    """How strongly a node shows: its opacity times the strongest color it paints."""
    colors = [node.get("fill"), (node.get("border") or {}).get("color"), *node.get("gradient", [])]
    strongest = max(map(color_alpha, colors), default=0)
    if node.get("pixels"):
        strongest = node["pixels"]["peakAlpha"]
    elif node["kind"] in PAINTS_INTERIOR and not node.get("fill"):
        strongest = 1  # bitmaps and unread drawings: color unknown, count them as solid
    return node.get("opacity", 1) * strongest


def with_origin(recording):
    """The first change anywhere in a recording stands for the action, so two recordings line up on it."""
    frames = recording["frames"]
    keys = element_keys(frames)
    for i in range(1, len(frames)):
        if any(element_state(frames[i], k) != element_state(frames[i - 1], k) for k in keys):
            recording["origin"] = frames[i]["t"]
            break
    else:
        recording["origin"] = 0
    return recording


def element_alpha(frame, key):
    return max((node_alpha(n) for n in frame["by_owner"].get(key, [])), default=0)


QUERY_OPERATORS = ("!=", ">=", "<=", "=", ">", "<", "~")


def frame_at(frames, t):
    return min(frames, key=lambda f: abs(f["t"] - t))


def frames_between(frames, start, end):
    return [f for f in frames if (start is None or f["t"] >= start) and (end is None or f["t"] <= end)]


def parse_where(text):
    """'kind=draw and alpha>0' to [(field, operator, value)]."""
    clauses = []
    for clause in text.split(" and "):
        operator = next((op for op in QUERY_OPERATORS if op in clause), None)
        if operator is None:
            fail(f"can't read '{clause}'", "bad_where", f"use field{'|'.join(QUERY_OPERATORS)}value joined by ' and '")
        field, value = (part.strip() for part in clause.split(operator, 1))
        clauses.append((field, operator, value))
    return clauses


def matches(fields, clauses):
    for field, operator, expected in clauses:
        if field not in fields:
            fail(f"unknown field '{field}'. Fields: {', '.join(fields)}", "unknown_field")
        actual = fields[field]
        if operator == "~":
            ok = expected.lower() in str(actual).lower()
        else:
            try:
                actual, expected_value = float(actual if actual is not None else "nan"), float(expected)
            except ValueError:
                actual, expected_value = str(actual).lower() if actual is not None else "none", expected.lower()
            ok = {"=": actual == expected_value, "!=": actual != expected_value, ">": actual > expected_value,
                  "<": actual < expected_value, ">=": actual >= expected_value, "<=": actual <= expected_value}[operator]
        if not ok:
            return False
    return True
