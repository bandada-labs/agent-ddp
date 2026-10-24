---
name: reviewer
description: Review an animation step by step: attach, watch, pause and step, record and query, checks, compare platforms, network, report.
---

# Reviewing an animation with ddp

You are the reviewer. The app runs a Debug build on a simulator or emulator. agent-device is your hands, `ddp` your eyes. You never need a video: every question below has a text answer. Commands are in `ddp skills get core --full`.

## 1. Attach

```
ddp connect <sim UDID | adb serial> --app <bundle or application id>
ddp tree
```

`tree` lists what is on screen now: each element (accessibility id, test tag or label) and what it drew (fills, borders, corners, draws, pixels). If the element you need is missing, ask for a `testTag` / `accessibilityIdentifier` on it. That is the only app change most reviews need.

## 2. Watch the animation while it runs

```
ddp watch card --then "agent-device press id=start" --for 3s
```

Every line is one frame where something changed: position, size, alpha, fill, border, number of draws. Use `--until` to stop at the moment you care about, e.g. `--until 'alpha>0.9'` or `--until 'present=false'`.

## 3. Stop time and look closely

```
ddp time pause
ddp tree --id card
ddp time step 5            # 5 frames forward, then tree again
ddp time seek -300ms       # rewind the app's own timeline 300 ms before the pause
ddp eval card layer.shadowRadius     # iOS; Android: border.width, semantics.Selected
ddp time resume
```

Pause, step and slow motion (`time rate 0.1`) work on Compose animations, UIKit / Core Animation, and any timeline the app drives from its clock port through ddp's clock. Only that last kind rewinds. SwiftUI's own `withAnimation` keeps running; for those, record and use `get` (step 4).

## 4. Record a window and ask questions

```
ddp read --for 3s --then "agent-device press id=start"      # what changed, per element
ddp get glow --from 1.2s --to 1.6s --file .ddp/last.jsonl    # per-frame rows of one element
ddp find --where 'kind=draw and alpha>0.5' --between 1s 2s --file .ddp/last.jsonl
ddp tree --at 1.4s --file .ddp/last.jsonl
```

Each live command records first and leaves `.ddp/last.jsonl`. Add `--file` to ask more questions about the same take without recording again. Look for `TRANSIENT` in `read`: a value that is the same before and after but different in between, such as a border that only flashes mid-animation.

## 5. Run the checks

```
ddp check overlap --for 4s --then "agent-device press id=start"   # text something is drawn over, at the end
ddp check corners --file .ddp/last.jsonl                 # a fill or hatch past rounded corners
ddp check timeline --expect cases.json --for 4s --then "agent-device press id=start"
```

`check timeline` compares the take with what should happen (`at`, `until`, `minAlpha`, `frame` per element) and says `MISSING`, `LATE` or `EARLY` with the ms. A case with `"absent": true` says what must not be drawn, such as an old screen flashing after a transition, and reports `SHOWN` with the frames. It exits 1, so it works as a gate.

## 6. Compare platforms

Record the same flow on both, then:

```
ddp check parity --file ios.jsonl --against android.jsonl
ddp diff ios.jsonl android.jsonl --id glow --ignore frame
```

`parity` gives one verdict per element: timing, peak alpha, corner, shadow. `diff` shows the frames where one element differs.

## 7. Check the network behind it

```
ddp net watch --until 'url~/api'     # then trigger the action with agent-device
ddp net list
ddp net get n3
```

Secrets are redacted inside the app before ddp sees them. If you still see one, report it as a bug in the app's redaction list.

## 8. Report

Quote the lines: element, time, value, expected value. For example: "`glow` MISSING (expected 2000..2400 ms): never in the recording; the take ends at 1700 ms because the screen closed early". Then `ddp disconnect`.

Limits worth knowing: Compose nodes without semantics are not seen, Android popups and dialogs are not read, iOS works on the simulator only.
