import difflib
import json
from .state import ctx, fail, parse_ms
from .model import (
    area,
    contains,
    element_key,
    element_alpha,
    element_keys,
    element_state,
    frame_at,
    frames_between,
    history,
    load,
    matches,
    node_alpha,
    normalize_label,
    parse_where,
    read_text,
    recording_path,
    with_origin,
)
from .live import live_session, record_live, recording_for, snapshot_live
from .summary import print_curves, print_timeline
from .checks import comparable_state, state_differences


def describe_node(node):
    parts = [f"z={node['z']}", node["kind"] + (f"/{node['op']}" if node.get("op") else "")]
    if node.get("fill"):
        parts.append(f"fill {node['fill']}")
    if node.get("border"):
        parts.append(f"border {node['border']['width']} {node['border']['color']}")
    for field in ("corner", "opacity", "source"):
        if node.get(field):
            parts.append(f"{field} {node[field]}")
    if node.get("pixels"):
        pixels = node["pixels"]
        parts.append(f"pixels {[c['color'] for c in pixels['colors']]} peakAlpha {pixels['peakAlpha']}")
    parts.append(str(node.get("visible", node["frame"])))
    return " ".join(parts)


def save_refs(frames):
    """@e1, @e2 ... in order of first appearance. A recording never changes, so its refs stay valid;
    live, they go stale as soon as the screen changes."""
    refs = {f"@e{i + 1}": key for i, key in enumerate(element_keys(frames))}
    ctx.dir.mkdir(parents=True, exist_ok=True)
    (ctx.dir / "refs.json").write_text(json.dumps(refs))
    return {key: ref for ref, key in refs.items()}


def resolve_ref(text):
    if not text or not text.startswith("@e"):
        return text
    try:
        refs = json.loads((ctx.dir / "refs.json").read_text())
    except (OSError, ValueError):
        refs = {}
    if text not in refs:
        fail(f"unknown ref {text}", "unknown_ref", "ddp tree (refs come from the last tree)")
    return refs[text]


def parents_by_containment(frame, tolerance):
    """Each element's parent: the smallest element listed before it whose frame holds it. Recorders list
    elements in tree order, so a parent always comes first; a popup listed later never adopts the screen below."""
    elements = frame["elements"]
    parents = {}
    for index, element in enumerate(elements):
        holders = [other for other in elements[:index] if contains(other["frame"], element["frame"], tolerance)]
        holder = min(holders, key=lambda e: area(e["frame"]), default=None)
        parents[element_key(element)] = element_key(holder) if holder else None
    return parents


def changing_keys(frames):
    keys = element_keys(frames)
    return {key for key in keys
            if len({json.dumps(element_state(f, key), sort_keys=True) for f in frames}) > 1}


def tree(args):
    session = live_session(args)
    if session:
        recording = record_live(args) if args.changed else snapshot_live(session)
    else:
        recording = load(args.file or ctx.last)
    frame = frame_at(recording["frames"], parse_ms(args.at)) if args.at else recording["frames"][-1]
    refs = save_refs(recording["frames"])
    parents = parents_by_containment(frame, recording["tolerance"])
    children = {}
    for element in frame["elements"]:
        children.setdefault(parents[element_key(element)], []).append(element)
    shown = changing_keys(recording["frames"]) if args.changed else None
    if shown is not None:
        # Keep the ancestors of what changes, so its place in the screen stays readable.
        for key in list(shown):
            while parents.get(key):
                key = parents[key]
                shown.add(key)

    def subtree_has(element):
        return shown is None or element_key(element) in shown or \
            any(subtree_has(child) for child in children.get(element_key(element), []))

    rows = []  # (element, depth) in print order

    def collect(element, depth):
        if not subtree_has(element) or (args.depth is not None and depth > args.depth):
            return
        rows.append((element, depth))
        for child in children.get(element_key(element), []):
            collect(child, depth + 1)

    root_id = resolve_ref(args.within or args.id)
    if root_id:
        root = next((e for e in frame["elements"] if element_key(e) == root_id), None)
        if root is None:
            fail(f"no element '{root_id}' at t={frame['t']}ms", "unknown_element", "ddp tree (without --within)")
        if args.id and not args.within:
            children = {}  # --id: that element alone
        collect(root, 0)
    else:
        for element in children.get(None, []):
            collect(element, 0)
    unowned = [] if args.interactive or shown is not None or root_id else frame["by_owner"].get(None, [])

    if ctx.json:
        print(json.dumps({"t": frame["t"], "unit": recording["header"]["unit"], "elements": [
            {"ref": refs[element_key(e)], "key": element_key(e), "depth": depth, "parent": parents[element_key(e)],
             **{k: e[k] for k in ("id", "label", "frame", "tap", "touch") if e.get(k) is not None},
             **({} if args.interactive else {"nodes": [public_node(n) for n in frame["by_owner"].get(element_key(e), [])]})}
            for e, depth in rows], **({"unowned": [public_node(n) for n in unowned]} if unowned else {})}))
        return
    print(f"t={frame['t']}ms (frame {frame['i']} of {len(recording['frames'])}), unit {recording['header']['unit']}")
    for element, depth in rows:
        key = element_key(element)
        label = f' "{element["label"]}"' if element.get("label") and element.get("id") else ""
        indent = "  " * depth
        print(f"{indent}{refs[key]} {key}{label} {element['frame']}")
        if not args.interactive:
            for node in frame["by_owner"].get(key, []):
                print(f"{indent}  {describe_node(node)}")
    if unowned:
        print("(owned by no element)")
        for node in unowned:
            print(f"  {describe_node(node)}")


def public_node(node):
    """A node as the recorder wrote it, plus the owner the CLI worked out."""
    return {k: v for k, v in node.items() if k != "own"}


def get(args):
    recording = recording_for(args)
    frames = frames_between(recording["frames"], args.start and parse_ms(args.start), args.end and parse_ms(args.end))
    if args.id not in element_keys(recording["frames"]):
        fail(f"no element '{args.id}'. Known: {', '.join(element_keys(recording['frames']))}", "unknown_element", "ddp tree, or ddp find text <part of a label>")
    if args.curve:
        print_curves({**recording, "frames": frames}, args.id)
        return
    if ctx.json:
        for frame in frames:
            state = element_state(frame, args.id)
            row = {"t": frame["t"], "present": state is not None}
            if state is not None:
                row.update({k: v for k, v in state.items() if k not in ("kinds",)}, alpha=round(element_alpha(frame, args.id), 3))
            print(json.dumps(row))
        return
    print_timeline(args.id, frames, [element_state(f, args.id) for f in frames])


def node_fields(node, frame):
    x, y, w, h = node["frame"]
    border = node.get("border") or {}
    return {"t": frame["t"], "z": node["z"], "kind": node["kind"], "op": node.get("op"), "owner": node["owner"],
            "source": node.get("source"), "fill": node.get("fill"), "border": border.get("width", 0),
            "borderColor": border.get("color"), "corner": node.get("corner", 0), "opacity": node.get("opacity", 1),
            "alpha": round(node_alpha(node), 2), "x": x, "y": y, "w": w, "h": h}


def find_elements(recording, by, value):
    """Elements whose id is `value` (by id) or whose label holds it, any case (by text)."""
    found = {}
    for frame in recording["frames"]:
        for element in frame["elements"]:
            hit = element_key(element) == value or element.get("id") == value if by == "id" else \
                normalize_label(value).lower() in (element.get("label") or "").lower()
            if hit:
                found.setdefault(element_key(element), []).append((frame["t"], element))
    if not found:
        print("[]" if ctx.json else "no match")
        return
    refs = save_refs(recording["frames"])
    if ctx.json:
        print(json.dumps([{"ref": refs[key], "key": key, "label": seen[-1][1].get("label"), "frame": seen[-1][1]["frame"],
                           "first": seen[0][0], "last": seen[-1][0], "frames": len(seen)} for key, seen in found.items()]))
        return
    for key, seen in found.items():
        label = f' "{seen[-1][1]["label"]}"' if seen[-1][1].get("label") and seen[-1][1].get("id") else ""
        print(f"{refs[key]} {key}{label} {seen[-1][1]['frame']} present {seen[0][0]}..{seen[-1][0]}ms "
              f"({len(seen)} of {len(recording['frames'])} frames)")


def find(args):
    recording = recording_for(args)
    if args.by:
        if args.by not in ("text", "id") or args.value is None:
            fail("find takes `text <part of a label>`, `id <id>` or --where", "bad_find")
        find_elements(recording, args.by, args.value)
        return
    if not args.where:
        fail("find needs `text <label>`, `id <id>` or --where", "missing_option", "ddp find --where 'kind=draw and alpha>0'")
    start, end = (parse_ms(t) for t in args.between) if args.between else (None, None)
    clauses = parse_where(args.where)
    found = [(frame, node) for frame in frames_between(recording["frames"], start, end)
             for node in frame["nodes"] if matches(node_fields(node, frame), clauses)]
    if ctx.json:
        print(json.dumps({"matches": len(found), "rows": [node_fields(node, frame) for frame, node in found[:args.limit]]}))
        return
    if not found:
        print("no match")
        return
    frames_hit = sorted({frame["t"] for frame, _ in found})
    print(f"{len(found)} matches in {len(frames_hit)} frames, t={frames_hit[0]}..{frames_hit[-1]}ms")
    for frame, node in found[:args.limit]:
        print(f"  t={frame['t']} owner={node['owner']} {describe_node(node)}")
    if len(found) > args.limit:
        print(f"  ... {len(found) - args.limit} more (narrow --where or --between, or raise --limit)")


def tree_text(recording):
    frame = recording["frames"][-1]
    return [f"{element_key(e)} {e['frame']} " + " ".join(describe_node(n) for n in frame["nodes"] if n["owner"] == element_key(e))
            for e in frame["elements"]]


def diff(args):
    """Two recordings; one against the last one; none: the last two takes in the history."""
    if len(args.recordings) > 2:
        fail("diff takes at most two recordings", "bad_diff")
    if len(args.recordings) < 2:
        kept = history()
        paths = [*args.recordings, ctx.last] if args.recordings else kept[-2:]
        if len(paths) < 2:
            fail("no two recordings to compare", "file_not_found", "record twice, or pass two files")
        args.recordings = paths
    if read_text(recording_path(str(args.recordings[0]))) == read_text(recording_path(str(args.recordings[1]))):
        print(f"diff: {args.recordings[0]} and {args.recordings[1]} are the same")
        return
    left, right = with_origin(load(args.recordings[0])), with_origin(load(args.recordings[1]))
    if not args.id:
        # Like a snapshot diff: the screen at the end of each take, one line per element.
        lines = list(difflib.unified_diff(tree_text(left), tree_text(right), str(args.recordings[0]),
                                          str(args.recordings[1]), n=3, lineterm=""))
        print("\n".join(lines) if lines else "same screen at the end of both")
        return
    print(f"diff {args.id}: {args.recordings[0]} vs {args.recordings[1]}, t from each recording's first change, "
          f"sizes in pt (repeated rows skipped)")
    previous, rows = None, 0
    for frame in left["frames"]:
        t = frame["t"] - left["origin"]
        other = frame_at(right["frames"], t + right["origin"])
        row = "; ".join(state_differences(comparable_state(left, frame, args.id),
                                          comparable_state(right, other, args.id), args.ignore)) or "same"
        if row != previous:
            print(f"  t={t:.0f}ms {row}")
            rows += 1
        previous = row
    if rows == 1 and previous == "same":
        print("  no differences")
