# Status

Last update: 2026-11-06. Read this after a break, then [DECISIONS.md](DECISIONS.md).

## In one paragraph

A small library inside a Debug build of an iOS or Android app serves what the app draws, frame by frame (ADR 0001). The `ddp` CLI, one Rust binary on npm (ADR 0002, 0003), reads it live or from recordings and prints short text for coding agents (ADR 0006). The protocol between them is versioned (ADR 0004). A flight recorder keeps the last 15 minutes on both platforms (ADR 0005).

## Released

0.4.0 on 2026-11-03 (CLI and both libraries; npm, GitHub release with 5 binaries, JitPack `v0.4.0`, SwiftPM from the repo URL): `ddp source`, which names the line of the app's code that made an element (Android: the composable calls and modifiers, from the compiler's source information; iOS: the SwiftUI view that wrote it, with its modifiers, or where a UIKit view was created), `ddp recompositions` on Android, Compose `Icon` and `Image` as bitmap nodes, `connect --wait`, and fixes from both device passes. Checked before the tag: CI's three jobs, and the new commands live on the Android emulator and on the iOS 27.0 simulator with the sample apps. What is in it: [CHANGELOG.md](../CHANGELOG.md).

0.3.0 on 2026-10-30 (CLI and both libraries; npm, GitHub release with 5 binaries, JitPack `v0.3.0`, SwiftPM from the repo URL): app state and events, gestures with a real finger, `perf`, `storage`, `media`, `crash`, `debug`, `profile`, `memory`, `audits`, `inspect`, `animations`, `eval --set`, device actions, network timing and cookies, a structure compare across phones, Compose `drawWithCache`, and the shapes inside a SwiftUI `Canvas`. Checked before the tag: CI's three jobs, the new commands live on both sample apps, and the hook matrix on iOS 26.5 and API 35. iOS 27.0 was checked for the device commands the same day, but its simulator runtime stopped booting after a CoreSimulator update before the hook matrix ran. What is in it: [CHANGELOG.md](../CHANGELOG.md).

0.2.1 on 2026-10-27 (CLI only; npm, GitHub release with 5 binaries, JitPack `v0.2.1`): `ddp compare`, which compares the screen against a reference screenshot, or one element against a reference image or against the same element on the other phone. `--ignore` and `--mask` leave moving parts out. The libraries are the same as in 0.2.0. Also a doc note: a headless simulator draws frames when the app has the library (121 frames in 2 s on iOS 27.0).

0.2.0 on 2026-10-24: the control endpoint (protocol 2), so the CLI and the library must be updated together. npm `agent-ddp`, GitHub release with 5 binaries, JitPack `com.github.bandada-labs:agent-ddp:v0.2.0`, SwiftPM from the repo URL. Checked before the tag: CI's three jobs locally, every command live on the sample apps, release builds without the library, and the hook matrix on iOS 26.5 and 27.0 and API 29, 33 and 35. What is in it: [CHANGELOG.md](../CHANGELOG.md).

0.1.5 on 2026-10-23.

## In progress: 0.5.0 (not released)

An app team uses an unofficial beta (0.5.0-beta.N, not on npm) as its only device tool and reports gaps; beta.3 (2026-11-06) fixed its first 11 findings, checked live on both platforms (19 iOS and 16 Android no-SDK taps against the library: 0 false positives, 0 misses).

Done and checked live on the iOS 27.0 simulator and the API 36 emulator with the sample apps: `ddp up` / `ddp down` (own device from a warm base, boot under 10 s); apps without the library on both platforms (`--device`, or a session `up` keeps): Android through ddp's runner instrumentation (a tap in about 35 ms), iOS from the Mac through the simulator's HID and accessibility services (about 200 ms per command, no XCTest); every action prints what changed with refs; `ddp walk` start, expect, end, replay with no model, and parity between iOS and Android (a walk made on iOS replayed on Android, parity 0 differences). Before the tag: the version bump, the release workflow building the runner APK first (changed, untested until a tag), and the hook matrix for the two new Mac-side hooks.

## Next

0. After 0.4.0: the hook matrix on older versions (API 26 to 28, iOS 17 and 18) once the images can be downloaded, and a physical iPhone when there is a test device.
1. The control endpoint in the library (ADR 0008, which replaced the CDP plan of ADR 0007). Seven phases, in order:
   1. Transport. Done.
   2. Reading: `app.info`, `screen.*`, `net.list`/`net.get`/`net.watch`, `log.watch`. Done.
   3. Acting: `input.*`, `net.*` pausing, `prefs.*`. Done on both platforms. Device settings (appearance, text size, reduce motion, locale) stay in `ddp set`, which changes the real setting through `simctl` and `adb`, so they need nothing in the library.
   4. Scripts in the app. Done: a script is an app command, Swift or Kotlin the app adds in Debug (`app.commands`, `app.run`), plus `net.send` with the app's own client. No JavaScript.
   5. Frames and history over the endpoint. Done: `screen.frame`, `screen.watch`, `screen.value`, `time.*`, `history.files`, `history.read`. The v1 `tap` (iOS accessibility action) is not carried over: `input.touch` is a real finger.
   6. The CLI on top. Done: every live command (`connect`, `tree`, `read`, `watch`, `wait`, `tap`, `eval`, `time`, `net`, `history`, `doctor`) talks to the control endpoint, with the same output. `tap` is a real touch on both platforms. New: `type`, `press`, `net send`, `net mock`, `net fail`, `prefs`, `run`, `logs --follow`.
   7. v1 goes. Done: the JSON lines endpoint (`live.json`, the socket `agentddp.<package>`) is gone from both libraries, and the protocol is version 2 in `app.info` and every header. All seven phases are done.
2. Tappability (`screen.hit`, `ddp tap` checks, `ddp check tappable`). Done on both platforms; checked on trap buttons in each sample app, each confirmed with a `--force` tap that changes nothing. iOS: disabled, covered by a clear view, a parent that takes no touches, outside its parent's bounds. Android: disabled, covered, outside, a parent `onInterceptTouchEvent`, a Compose parent consuming the Initial pass. On iOS, which gesture wins during a touch (a parent tap recognizer, a `highPriorityGesture`) is decided by UIKit only during a real touch, so it is left to `tap`'s `nothing changed`; the sample keeps those traps.
3. The places that modeled the engine (the second hard rule, 2026-10-24). All seven fixed: the Android View hit test, whose SwiftUI gesture it is, covered text, corners, touch size, contrast, jank, and `get --curve`. Each check has a case in the sample apps where it must fire and one where it must stay quiet, confirmed live on both apps. Engine hooks are measured (the third hard rule): `app.hooks`, `ddp doctor`, [hooks.md](hooks.md); a failed hook is printed with every check and kept in the take. The one hook that read memory without a check (`swiftui.responders`, word 2 of a metadata record) now checks the layout first and fails instead of crashing. Open: a known-answer probe for hooks that answer wrong without failing (proposed, maintainer unsure about the cost); the hook matrix (`samples/hook_matrix.py`) passes on API 29, 33 and 35 and on iOS 26.5 and 27.0. On iOS 26.5 it found the gesture container keeps `responder` itself, not in `base`; both are read now. API 26 to 28 and iOS 15 to 25 are not checked.
4. A code of conduct, once the maintainer gives a contact address.
5. iOS on physical devices (today the CLI uses `simctl`).
6. History on iOS for `UIView.animate` animations, if users need more than their start and end (ADR 0005).

## Known limits

Listed in [agents.md, "What it does not see yet"](agents.md#what-it-does-not-see-yet).
