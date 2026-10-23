"""`ddp history`: what the app showed in the last minutes, from the flight recorder inside the Debug build.
It keeps every frame that changed plus taps and screen changes, so a still screen costs nothing."""
import gzip
import json
import subprocess
import time
from pathlib import Path
from .state import ANDROID_DIR, adb_path, ctx, fail, parse_ms
from .model import assign_owners, contains, element_key, element_state, name_elements, save_recording
from .live import LiveClient, live_session
from .device import target

SEGMENT_MS = 60_000  # the recorder starts a new file every minute
BURST_GAP_MS = 500   # frames closer than this belong to one change
NAMES_SHOWN = 4


def history(args):
    since = time.time() * 1000 - parse_ms(args.since)
    segments = read_segments(args, since)
    if args.action == "at":
        return take(args, segments)
    timeline(args, segments, since)


def read_segments(args, since):
    """[(header, [line, ...])] from the files that may hold anything after `since`, oldest first."""
    if args.dir:
        files = {path.name: path.read_bytes() for path in Path(args.dir).iterdir()}
    else:
        files = android_files(args, since)
    segments = []
    for name in sorted(files, key=lambda n: int(n.split(".")[0]) if n.split(".")[0].isdigit() else 0):
        start = name.split(".")[0]
        if not start.isdigit() or int(start) < since - SEGMENT_MS:
            continue
        data = files[name]
        text = (gzip.decompress(data) if data[:2] == b"\x1f\x8b" else data).decode("utf-8", "replace")
        lines = []
        for raw in text.splitlines():
            try:
                lines.append(json.loads(raw))
            except ValueError:
                pass  # the last line of a file the app was writing when it died
        header = next((line for line in lines if line.get("type") == "header"), None)
        if header:
            kept = [line for line in lines if line.get("type") in ("frame", "event")]
            for line in kept:
                line["at"] = header["startedAt"] + line.get("t", 0)
            segments.append((header, kept))
    if not segments:
        fail("no history in that window", "no_history",
             "the library keeps history since the app started; use the app, or a longer --since")
    return segments


def android_files(args, since):
    platform, device, app = target(args)
    if not app:
        fail("no app", "missing_option", "ddp connect DEVICE --app ID first, or pass --app")
    if platform != "android":
        fail("history is Android only for now", "unsupported_platform", "record a take with ddp read --for 3s")
    adb = [adb_path(), "-s", device, "exec-out", "run-as", app]
    listing = subprocess.run([*adb, "ls", f"{ANDROID_DIR}/history"], capture_output=True, text=True)
    if listing.returncode != 0:
        fail(f"no history on the device: {listing.stderr.strip() or listing.stdout.strip()}", "no_history",
             f"update the library in the app; ddp doctor {device} --app {app}")
    files = {}
    for name in listing.stdout.split():
        start = name.split(".")[0]
        if start.isdigit() and int(start) >= since - SEGMENT_MS:
            files[name] = subprocess.run([*adb, "cat", f"{ANDROID_DIR}/history/{name}"], capture_output=True).stdout
    return files


def clock(at):
    return time.strftime("%H:%M:%S", time.localtime(at / 1000)) + f".{int(at % 1000) // 100}"


def named(keys):
    keys = sorted(keys)
    shown = ", ".join(keys[:NAMES_SHOWN])
    return shown + (f" +{len(keys) - NAMES_SHOWN}" if len(keys) > NAMES_SHOWN else "")


def element_under(frame, point):
    """The smallest element holding the point: what a tap there hit."""
    holding = [e for e in frame["elements"] if contains(e["frame"], [point[0], point[1], 0, 0])]
    return element_key(min(holding, key=lambda e: e["frame"][2] * e["frame"][3])) if holding else None


def timeline(args, segments, since):
    """One line per change (elements that appeared, went or changed), per tap and screen, per request.
    Elements that change in most frames (a clock, a spinner) are left out, or every change would merge into one."""
    entries, frames, previous = [], [], None
    for header, lines in segments:
        tolerance = 4 * header.get("scale", 1)
        for line in lines:
            if line["type"] == "event":
                if line["at"] >= since:
                    entries.append((line["at"], event_text(line, previous)))
                continue
            name_elements(line)
            assign_owners(line, tolerance)
            if line["at"] >= since or not frames:
                frames.append(line)
            else:
                frames[0] = line  # the screen as it was when the window starts
            previous = line
    changes = [(frame, {key for key in frame["by_key"] if key in before["by_key"]
                        and element_state(frame, key) != element_state(before, key)})
               for before, frame in zip(frames, frames[1:])]
    counts = {}
    for _, changed in changes:
        for key in changed:
            counts[key] = counts.get(key, 0) + 1
    noisy = {key for key, count in counts.items() if count > len(changes) / 2 and len(changes) >= 20} | set(args.ignore)
    entries += bursts(frames, changes, noisy)
    entries += request_entries(args, since)
    entries = sorted(entry for entry in entries if entry[1])
    print(f"history {clock(max(since, frames[0]['at'] if frames else since))} to {clock(time.time() * 1000)}: "
          f"{len(frames)} frames kept, {sum(1 for _, text in entries if text.startswith('tap'))} taps" +
          (f"; always changing, left out: {named(noisy)}" if noisy else ""))
    hidden = max(len(entries) - args.limit, 0)
    if hidden:
        print(f"... {hidden} earlier lines (a shorter --since, or --limit)")
    for at, text in entries[hidden:]:
        print(f"{clock(at)} {text}")
    if entries:
        print("next: ddp history at <time> --for 2s, then read/check with --file .ddp/last.jsonl")


def bursts(frames, changes, noisy):
    """Changes closer than BURST_GAP_MS apart as one entry: what appeared, went and changed in between."""
    entries, burst = [], None
    for (frame, changed), before in zip(changes, frames):
        appeared_or_gone = set(frame["by_key"]) ^ set(before["by_key"])
        changed = (changed | appeared_or_gone) - noisy
        if not changed:
            continue
        if burst and frame["at"] - burst["last"] > BURST_GAP_MS:
            entries.append(close_burst(burst, noisy))
            burst = None
        if burst is None:
            burst = {"start": frame["at"], "before": before, "changed": set()}
        burst["last"], burst["after"] = frame["at"], frame
        burst["changed"] |= changed
    if burst:
        entries.append(close_burst(burst, noisy))
    return entries


def event_text(event, frame):
    if event["kind"] == "tap":
        hit = element_under(frame, (event["x"], event["y"])) if frame else None
        return f'tap {hit or "nothing"} ({event["x"]:.0f},{event["y"]:.0f})'
    if event["kind"] == "swipe":
        return f'swipe ({event["x"]:.0f},{event["y"]:.0f}) -> ({event["toX"]:.0f},{event["toY"]:.0f})'
    if event["kind"] == "screen":
        return f'screen {event["name"]}'
    return "app started" if event["kind"] == "launch" else event["kind"]


def close_burst(burst, noisy):
    before, after = set(burst["before"]["by_key"]) - noisy, set(burst["after"]["by_key"]) - noisy
    appeared, gone, parts = after - before, before - after, []
    # An element without an id is named by its label, so a new text reads as one element going and another
    # appearing at the same place: show it as the text change it is.
    for old in sorted(gone):
        place = burst["before"]["by_key"][old]["frame"][:2]
        new = next((key for key in sorted(appeared) if burst["after"]["by_key"][key]["frame"][:2] == place), None)
        if new:
            parts.append(f'"{old}" -> "{new}"')
            appeared.discard(new)
            gone.discard(old)
    if appeared:
        parts.append(f"+{named(appeared)}")
    if gone:
        parts.append(f"-{named(gone)}")
    changed = {key for key in burst["changed"] if key in before & after}
    if changed:
        parts.append(f"changed {named(changed)}")
    length = burst["last"] - burst["start"]
    return burst["start"], "  ".join(parts) + (f" ({length / 1000:.1f}s)" if length >= 100 else "") if parts else ""


def request_entries(args, since):
    """Requests the app reported, while it is connected live (the library keeps the last 200 in memory)."""
    session = None if args.dir else live_session(args)
    if not session:
        return []
    try:
        requests = list(LiveClient(session, {"cmd": "net"}))
    except Exception:
        return []  # the app is gone: frames and taps are still worth showing
    return [(r["startedAt"], f'{r["method"]} {r.get("status", r.get("error", "failed"))} {r["ms"]:.0f}ms {r["url"]}')
            for r in requests if r.get("startedAt", 0) >= since]


def parse_clock(text):
    """'14:31:02' or '14:31:02.5', today (yesterday when that is still ahead), to epoch ms."""
    try:
        whole, _, fraction = text.partition(".")
        moment = time.strptime(time.strftime("%Y-%m-%d ") + whole, "%Y-%m-%d %H:%M:%S")
    except ValueError:
        fail(f"can't read time '{text}'", "bad_time", "use the time from ddp history, e.g. 14:31:02.5")
    at = time.mktime(moment) * 1000 + float(f"0.{fraction or 0}") * 1000
    return at - 86_400_000 if at > time.time() * 1000 + 60_000 else at


def take(args, segments):
    """The history between `at` and `at + --for` as an ordinary take: the frame on screen at the start,
    every change after it, and the screen held until the end."""
    if not args.time:
        fail("history at needs a time", "missing_option", "ddp history, then ddp history at 14:31:02 --for 2s")
    start = parse_clock(args.time)
    end = start + parse_ms(args.duration)
    header = next((h for h, lines in reversed(segments) if h["startedAt"] <= start), segments[0][0])
    frames = [line for _, lines in segments for line in lines if line["type"] == "frame"]
    shown = [f for f in frames if f["at"] <= start][-1:] + [f for f in frames if start < f["at"] <= end]
    if not shown:
        fail(f"nothing recorded by {clock(start)}", "no_history", "ddp history to see the times it holds")
    shown.append(dict(shown[-1], at=end))
    out = [dict(header, startedAt=start, frames=len(shown))]
    for index, frame in enumerate(shown):
        out.append({key: value for key, value in frame.items() if key != "at"} | {"i": index, "t": max(frame["at"] - start, 0)})
    out.append({"type": "end", "frames": len(shown)})
    save_recording("\n".join(json.dumps(line) for line in out) + "\n", args.name)
    print(f"take {clock(start)} to {clock(end)}: {len(shown)} frames, only the ones that changed -> {ctx.last}")
    print(f"next: ddp read --file {ctx.last}, ddp check all --file {ctx.last}")
