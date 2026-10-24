# Status

Last update: 2026-10-24. Read this after a break, then [DECISIONS.md](DECISIONS.md).

## In one paragraph

A small library inside a Debug build of an iOS or Android app serves what the app draws, frame by frame (ADR 0001). The `ddp` CLI, one Rust binary on npm (ADR 0002, 0003), reads it live or from recordings and prints short text for coding agents (ADR 0006). The protocol between them is versioned (ADR 0004). A flight recorder keeps the last 15 minutes on both platforms (ADR 0005).

## Released

0.1.5 on 2026-10-23: npm `agent-ddp`, GitHub release with 5 binaries, JitPack `com.github.bandada-labs:agent-ddp:v0.1.5`, SwiftPM from the repo URL. What is in it: [CHANGELOG.md](../CHANGELOG.md).

## Next release

0.2.0 is ready and waits for the maintainer's yes (a `v0.2.0` tag runs the release). It changes the protocol, so the CLI and the library must be updated together. What is in it: the 0.2.0 section of [CHANGELOG.md](../CHANGELOG.md), which also becomes the GitHub release notes. Checked on 2026-10-24: CI's three jobs locally, every command live on the iOS simulator and the Android emulator, release builds without the library, the npm package installed from a local pack, the JitPack artifact, and the 0.1.5 library and CLI against the new ones.

## Next

1. The control endpoint in the library (ADR 0008, which replaced the CDP plan of ADR 0007). Seven phases, in order:
   1. Transport. Done.
   2. Reading: `app.info`, `screen.*`, `net.list`/`net.get`/`net.watch`, `log.watch`. Done.
   3. Acting: `input.*`, `net.*` pausing, `prefs.*`. Done on both platforms. Device settings (appearance, text size, reduce motion, locale) stay in `ddp set`, which changes the real setting through `simctl` and `adb`, so they need nothing in the library.
   4. Scripts in the app. Done: a script is an app command, Swift or Kotlin the app adds in Debug (`app.commands`, `app.run`), plus `net.send` with the app's own client. No JavaScript.
   5. Frames and history over the endpoint. Done: `screen.frame`, `screen.watch`, `screen.value`, `time.*`, `history.files`, `history.read`. The v1 `tap` (iOS accessibility action) is not carried over: `input.touch` is a real finger.
   6. The CLI on top. Done: every live command (`connect`, `tree`, `read`, `watch`, `wait`, `tap`, `eval`, `time`, `net`, `history`, `doctor`) talks to the control endpoint, with the same output. `tap` is a real touch on both platforms. New: `type`, `press`, `net send`, `net mock`, `net fail`, `prefs`, `run`, `logs --follow`.
   7. v1 goes. Done: the JSON lines endpoint (`live.json`, the socket `agentddp.<package>`) is gone from both libraries, and the protocol is version 2 in `app.info` and every header. All seven phases are done.
2. Tappability (`screen.hit`, `ddp tap` checks, `ddp check tappable`). Done on both platforms; checked on trap buttons in each sample app, each confirmed with a `--force` tap that changes nothing. iOS: disabled, covered by a clear view, a parent that takes no touches, outside its parent's bounds. Android: disabled, covered, outside, a parent `onInterceptTouchEvent`, a Compose parent consuming the Initial pass. On iOS, which gesture wins during a touch (a parent tap recognizer, a `highPriorityGesture`) is decided by UIKit only during a real touch, so it is left to `tap`'s `nothing changed`; the sample keeps those traps.
3. The places that still model the engine instead of asking it (the second hard rule, 2026-10-24). Each needs the maintainer's call: replace with an engine answer, or remove.
   - Android Views, `screen.hit`: which window and which child take a touch is rebuilt (bounds, visibility, Z and drawing order, clickable or a touch listener). View dispatch has no dry run. Transforms, `TouchDelegate` and a custom `onTouchEvent` are not seen.
   - iOS SwiftUI, `screen.hit`: whether the gesture SwiftUI returns belongs to the control is guessed (the same gesture does not win 2 pt outside the control's frame), and a List row by the cell under its center.
   - `check overlap` and `check a11y` contrast: what is drawn over what is our walk of the layer and draw tree (sublayer order and `zPosition`, 3D transforms ignored) and our blend of fills. iOS gives a node to the smallest element whose frame holds it. The engine's answer is the pixels it composited.
   - `check corners`: rounded-rect geometry over the nodes, not the clip the renderer applied.
   - `check a11y` small targets: iOS uses the accessibility frame, not the area `point(inside:)` accepts. SwiftUI text size is the bitmap height / 1.2, which picks the 3:1 or 4.5:1 limit.
   - `check jank`: gaps between our capture frames, three times the usual one. The engine counts hitches itself (iOS `CADisplayLink` timestamps, Android `FrameMetrics`).
   - `get --curve`: the easing name is fitted from samples, while the engine holds the real timing function or interpolator.
4. A code of conduct, once the maintainer gives a contact address.
5. iOS on physical devices (today the CLI uses `simctl`).
6. History on iOS for `UIView.animate` animations, if users need more than their start and end (ADR 0005).

## Known limits

Listed in [agents.md, "What it does not see yet"](agents.md#what-it-does-not-see-yet).
