# Changelog

Changes that users or agents will notice. Before 1.0, a minor version may change the protocol or the output; those changes are marked.

## Unreleased

- `ddp animations` (iOS): the Core Animation animations on screen, by layer, with key path, class, duration and repeat. `ddp eval <id> layer.speed --set 0` holds one. Android refuses: Compose animations are not listed by the platform.
- `ddp eval <id> <path> --set VALUE` (iOS): try a style live, a number, `true`/`false` or a color set on the element or its layer, then read back from UIKit. Android refuses: Compose takes its values from the app's state.
- `ddp launch --arg` (iOS, repeatable): a launch argument as an Xcode scheme passes it, such as `-_UIConstraintBasedLayoutLogUnsatisfiable YES` so iOS 27 logs Auto Layout conflicts for `ddp audits`. Android refuses it and points to `--env`.
- Android: a layer dimmed in a `graphicsLayer { alpha = ... }` block now shows its opacity; only `graphicsLayer(alpha = ...)` and `Modifier.alpha` were read before. The block runs on Compose's own `GraphicsLayerScope()`, so it reads the same animated state.
- `ddp net get` shows a request's phases (`timing`: dns, connect, tls, wait, and download on iOS), its `protocol` and whether it `reused` a connection, when the app passes `AgentDDP.timing` to URLSession (iOS) or `AgentDdpNetwork.timing` as OkHttp's event listener factory (Android).
- `ddp inspect`: a person taps an element on the phone and ddp says which, from the taps History keeps. `ddp highlight <element>`: a box over it in the live app for a while (new protocol method `screen.highlight`).
- `ddp logs --follow --source`: on iOS each line also says its subsystem and the library that wrote it (`log.entry` adds `subsystem` and `sender`).
- `ddp net cookies` and `ddp net clear-cookies`: the app's cookies by name, domain and expiry (never the values) from `HTTPCookieStorage.shared` or the default `java.net.CookieManager`, and clearing them. Offline and slow networks stay `ddp net fail` and `ddp net mock`.
- `ddp profile [--for 3s]`: the functions the app spends its time in, from `sample` on the simulator or `simpleperf` on Android, top entries only, kernel waits counted apart.
- `ddp memory [summary|leaks|heap]`: the app's memory with the platform's tools: `footprint`, `leaks` and `heap` on the simulator process; `dumpsys meminfo` and `am dumpheap` (an `.hprof` in `.ddp/`) on Android.
- `ddp audits [--since 5m]`: the runtime warnings the platform wrote in the app's log (SwiftUI and AttributeGraph issues, Auto Layout conflicts (iOS 27 logs them only with `-_UIConstraintBasedLayoutLogUnsatisfiable YES`), Main Thread Checker; StrictMode, skipped frames, leaked windows), exit 1 when there is one. The samples get a `warn` command.
- `ddp back` (the Back key, or iOS's edge swipe) and `ddp background --for 2s` (leave the app and come back). History records `background` and `foreground` from the platform's notifications. `app.info` adds `screen`.
- Device actions with each platform's own tool: `ddp set location` and `set orientation` (Android), `ddp biometric enroll|match|fail`, `ddp push` (iOS, `simctl push`; `not_authorized` when the app has no notification permission) and `ddp job` (Android, JobScheduler). What a platform cannot do from the host is refused with the reason.
- The new commands work like the old ones for agents: `--json` on `state`, `events`, `storage`, `perf` and `media`; `@eN` refs for both places of `swipe` and `drag`; and the MCP server offers them (`launch` and `swipe` in `core`, the rest in `live`, `compare` in `checks`).
- Android: what Compose `drawWithContent` and `drawWithCache` lambdas draw is read like a `Canvas` or `drawBehind` one, and text drawn in a Canvas is a `text` draw op with its string. `drawWithCache` goes through a new engine hook, `compose.drawCache`.
- `ddp tap` and `check tappable` see the keyboard: a control all under it, as UIKit's keyboard notifications or Android's IME insets (API 30+) place it, is `COVERED by the keyboard` instead of reaching.
- `ddp media`: the players the platform shows (an `AVPlayerLayer` or `VideoView` on screen) with state, position, duration and rate, and the now-playing info on iOS. A player with no public way in (audio-only, Media3) is shown as an app value, with a recipe in `ddp skills get app`. New protocol method `media.list`.
- `ios/build-swiftc.sh`: builds the iOS library as a static library and module with plain `swiftc`, for apps built without an Xcode project or SwiftPM. CI builds it, and the iOS sample app, on Xcode 15.4, the oldest Xcode supported.
- `ddp compare --on ios,android --structure`: what the two phones draw by label, not pixels: labels only one phone has, and the same label drawn as another kind. Platform idiom no longer hides a missing tab or chip.
- `ddp storage`: the app's files (list, read, remove), its SQLite databases with one read-only query, and its Keychain items or Keystore aliases by name, never their data. Text is redacted like network bodies. New protocol methods `storage.files`, `storage.read`, `storage.remove`, `storage.query`, `storage.keys`. `ddp launch --reset` clears the app's data first.
- `ddp perf [--for 5s]`: refreshes the main thread missed and the worst gap, from the platform's vsync callbacks, plus the app's work per frame on Android (FrameMetrics, without the GPU wait), without a recording. New protocol method `perf.metrics`. History pauses while it measures.
- Gestures with a real finger: `ddp swipe`, `ddp drag`, `ddp double-tap`, `ddp long-press`, and `ddp tap x,y`, each followed by what changed. Without the library, `--device` uses Android's `adb input` with screen pixels.
- Fixed: a command given `--device` for another device than the session's took the session's platform and app.
- `ddp crash`: the app's last crash with its stack, from the platform's own record, after the app is gone. iOS reads the simulator crash report macOS writes, plus the `Fatal error` message; Android reads the crash log buffer (Java and native crashes). Nothing changes in the libraries.
- `ddp events`: events the app sends in Debug (`AgentDDP.event` on iOS, `AgentDdpEvents.send` on Android), the last 200 or each as it comes with `--follow`. Each one is also in `ddp history` next to frames and taps. New protocol methods `events.list`, `events.watch`, `events.unwatch`.
- `ddp state`: values the app adds in Debug (`AgentDDP.state` on iOS, `AgentDdpState.add` on Android), for any app. List, `get`, `set` the writable ones, and `watch --until`. New protocol methods `state.list`, `state.get`, `state.set`. `ddp skills get app` shows an agent how to add them.
- `ddp launch <device> --app <id> [--env KEY=VALUE ...]`: starts the app fresh with values for it and connects. iOS gets environment variables, Android string extras of the launch intent; ddp then asks the app which names arrived (new protocol method `app.launch`, names only) and prints `app received ...`. `doctor` and `logs` point to it when the app is not running, instead of a `monkey` line that did not start every app.
- `ddp compare --reference a.png --screen b.png`: two saved screenshots, so both phones can be captured first and compared later with the devices off.
- `ddp compare --max <pct>` and `--max-band <pct>`: a pass bar the caller sets on the share of pixels that differ, overall and in any one band. Over it, `compare` exits 1. Without a bar it still exits 0.
- `ddp record` keeps the platform's answers at the end of a live take (hit test and pixels), so `check overlap`, `corners`, `a11y` and `jank` on the file judge it as they would live. Before, they said `not judged`.
- Fixed: `ddp compare --device` on iOS failed in a folder without `.ddp` ("The folder screen.png doesn't exist"). Reading a reference that is not a PNG had the same problem.
- Fixed: History used a lot of CPU on an idle screen with a view that redraws every frame without changing what ddp reads, such as a looping Lottie or a spinner. Each redraw ran a full capture on the main thread. After 2 s of captures that see nothing new, History now captures at most every 250 ms, as it already did for busy elements. Measured on the sample's spinner screen: iOS 27.0 simulator 75% CPU before, 4% after, 1% without ddp; Android API 35 emulator, main thread 34-55% of a core before, 10-14% after, 5-11% without ddp.

## 0.2.1 (2026-10-27)

The CLI can compare pixels: the screen against a reference screenshot, and one element against a reference image or against the same element on the other phone. Only the CLI changed. The libraries are the same as in 0.2.0, so there is nothing to update in the app.

### Added

- `ddp compare --reference <image>`: the device's screenshot against a reference screenshot of another size. The reference is scaled to the screen's width with the top edges together. ddp prints the difference overall and in 8 bands from top to bottom, and writes `ddp-compare.png` with three panels: the reference, the screen, and the difference in red. It needs only `--device`, so the app does not need the library.
- `ddp compare <element> --reference <image>` and `ddp compare <element> --on ios,android`: one element's pixels from the app's own windows (`screen.pixels`). The second form compares two sessions, for example the same card on iPhone and Android. Both images are scaled to the same size.
- `--ignore <element>...` and `--mask x,y,w,h` leave a clock, a spinner or the status bar out of the scores.
- A reference can be any image macOS opens with `sips` (JPEG, WebP, HEIC), not only PNG.
- `compare` is a measurement, not a check: it reports no problem and always exits 0.

### Docs

- A simulator booted with `xcrun simctl boot` and no Simulator.app window draws frames when the app has the library. Measured on iOS 27.0: 121 frames in 2 s and `check jank` with no missed refresh (docs/agents.md).

## 0.2.0 (2026-10-24)

The library now serves a control endpoint with names taken from Android and iOS (ADR 0008), and the CLI uses only that. Agents can act on the app, not only read it: touch and type, answer or fail its requests, edit its preferences and run commands the app adds in Debug. `ddp tap` checks first that a touch reaches the element. Every check now reports only what the platform itself answers (its hit test, the pixels of the app's own windows, its frame times), and says `not judged` where it cannot. The engine pieces it reaches outside public API were checked on the iOS 26.5 and 27.0 simulators and on Android API 29, 33 and 35 emulators (docs/hooks.md).

### Upgrading

- Update the CLI and the library together. CLI 0.2.0 speaks only the control endpoint, which libraries before 0.2.0 do not have: `ddp connect` and `ddp doctor` say so with the code `library_too_old`. CLI 0.1.x cannot reach a 0.2.0 library and says `can't reach the app`. Recordings and history files from 0.1.x still open in 0.2.0.
- Scripts that read `ddp tap` output, or expect it to always succeed, see the new output and errors below. `ddp tap --force` is the old behaviour.
- Scripts that read the output of `check overlap`, `corners`, `a11y` or `jank` see the new lines below. These checks judge only takes recorded live by a check (or `explore`); on history or an old file they say `not judged`.

### Breaking

- Protocol version 2. The v1 JSON lines endpoint is gone from both libraries: iOS no longer writes `live.json`, Android no longer opens the socket `agentddp.<package>`. `app.info` and every header say `"protocol": 2`.
- `ddp tap` asks the app's hit test before it touches. It waits up to 2 s for the element to hold still, be enabled and have a point a touch reaches, and fails with `disabled`, `covered`, `untouchable`, `outside` or, on Android, `intercepted` (a parent takes the touch on its way down) otherwise. After the touch it prints what changed, or `nothing changed within 1000ms; look again before a second tap`.
- `ddp tap` on iOS is a real touch, as on Android, instead of the element's accessibility action. Elements with no accessibility action can now be tapped; the button's pressed look shows.
- `check overlap` reports text something is drawn over so it cannot be seen, from the app's pixels, instead of nodes painted over a control by the recorded paint order. A control covered for touch is `check tappable`'s.
- `check corners` reports a draw past a rounded corner only when the app's pixels show it there.
- `check a11y`: `SMALL TARGET` is how far a touch reaches the control by the app's hit test, so SwiftUI's touch radius and Compose's 48 dp widening count; contrast comes from the app's pixels at the end of the take, not the worst frame; text of a disabled control and SwiftUI text under 4.5:1 but over 3:1 (its size is estimated) are not reported.
- `check jank` reports refreshes the app missed, counted by the library from the platform's frame times without its own capture time, instead of frame gaps over three times the usual one. `JANK the app missed 16 refreshes in one place, ...`.
- `get --curve` says `matches easeOut (off by 1%)` and `overshoots by 12%`: a comparison with named curves, not the curve the app ran. `spring` and `loose fit` are gone.
- `--all-frames` is gone from `check`: pixel checks judge the end of the take.

### Added

- Control endpoint in the library: JSON-RPC over a WebSocket. Android: the abstract socket `agentddp.<package>.control`, open to adb and root only. iOS simulator: the port in `Library/Caches/agent-ddp/control.json`. Requests from web pages (with an `Origin`) and with a foreign `Host` are refused. Methods, documented in docs/protocol.md:
  - `app.info`, `app.commands`, `app.run`: the app's details, and named actions the app adds in Debug with `AgentDDP.command` (iOS) or `AgentDdpCommands.add` (Android), in its own Swift or Kotlin.
  - `screen.elements`, `screen.capture`, `screen.frame`, `screen.watch`, `screen.value`: the elements, an image of every window, one frame, a frame stream and one property.
  - `screen.hit`: where a touch on an element would land, from the platform's own hit test (UIKit, SwiftUI's gesture hit test with UIKit's touch radius and SwiftUI's responders, ViewGroup's own dispatch methods, Compose `LayoutNode.hitTest`), without touching, and with `touch` how far a touch reaches each control. On Android it also finds a parent that takes the touch first: an `onInterceptTouchEvent`, a Compose `PointerEventPass.Initial` consumer. What the platform cannot decide (app code decides it, or a hook is missing) counts as reaching.
  - `screen.pixels`: the pixels of the app's own windows, as composited (`drawHierarchy`, `PixelCopy`), and on iOS the screen's colors at each text's ink and clear points.
  - `app.hooks`: every engine piece the library reaches outside public API, its kind, and how often it answered, failed or fell back in this run.
  - `input.touch`, `input.type`, `input.key`: real touches and typing, delivered the way the system delivers them, so gesture recognizers, SwiftUI and Compose see a real finger.
  - `net.list`, `net.get`, `net.watch`, `net.send`, and `net.intercept` with `net.continue`, `net.respond`, `net.fail`, `net.release`: the request log, a request sent with the app's own client, and pausing the app's requests to let them go on changed, answer them or fail them the way the app's client fails (`offline`, `timeout`, ...).
  - `log.watch`: the app's log lines (os_log on iOS, logcat on Android).
  - `prefs.files`, `prefs.read`, `prefs.write`, `prefs.remove`, `prefs.clear`: UserDefaults and SharedPreferences. A write keeps the type the key holds; secrets read as `[redacted]`.
  - `time.*`, `history.files`, `history.read`: the virtual clock and the flight recorder's files.
- Elements carry `disabled` when the platform says so. Frames carry `missed` (refreshes the app missed); nodes carry `corners` (per-corner radii), `effects` (under a filter, blend mode or mask) and, on text, `colors` and `unknownColor`.
- `ddp doctor` prints every engine hook with its state, and fails on a failing one. docs/hooks.md lists them and the versions they were checked on.
- Every check, `check tappable` and `explore` print a line for each hook that failed in the app run, and takes keep them in their `inspect` line. The exit code does not change.
- Takes recorded by a check end with an `inspect` line: what the platform answered at the end (hit test, pixels).
- iOS: `AgentDDP.send(request) { ... }` for the app's Debug network decorator, so its requests can be paused and answered.
- New commands:
  - `ddp check tappable`: every control on screen that a touch does not reach, and why. Nothing is touched.
  - `ddp type <text> [--into <element>]` and `ddp press Enter|Tab|Backspace`.
  - `ddp net send <url>`, `ddp net mock <pattern> --status 503`, `ddp net fail <pattern> --error offline`. `mock` and `fail` answer the matching requests for `--for`, while `--then` runs.
  - `ddp prefs [list|set|rm|clear|files]`.
  - `ddp run [<command> [params]]`.
  - `ddp logs --follow`.

### Changed

- The CLI talks to the control endpoint. `ddp connect` reads `control.json` on iOS and forwards `agentddp.<package>.control` on Android. The output of the other commands is unchanged.
- `ddp explore` skips the controls a touch does not reach and says why.
- `ddp connect` and `ddp doctor` recognise a library before 0.2.0 and fail with `library_too_old` and the fix.
- iOS: the closure given to `AgentDDP.send` is `@escaping`, because `net.send` reuses the last one.

### Fixed

- Android: `prefs.*` with `"file": null` wrote to a file called `null` instead of the app's own preferences.
- Android: a streamed frame could carry the flight recorder's frame number and time when the recorder stamped it before it was sent, so a 1 s `ddp read` could end after a few frames.
- `check a11y` called a Compose control small although Compose widens its touch to 48 dp, and judged contrast against fills the recorder guessed were behind the text.
- `check corners` called a corner bleed where `maskedCorners` or per-corner radii keep that corner square.
- Android, with the keyboard up in a panned window (`adjustPan`): popups' frames, the screenshot, and the points `tap` and `check tappable` asked about were off by the pan, so controls read as covered by what was drawn one keyboard height above them. Frames and touches now go through the window's own place on screen.
- `--max-output 0` printed a `[truncated ...]` note; it now prints nothing, as `explore` expects.
- iOS: `ddp logs --follow` showed hundreds of lines a minute that the library itself causes (its own connections in `com.apple.network`, the accessibility tree it turns on in `com.apple.Accessibility`). They are left out.

## 0.1.5 (2026-10-23)

- Protocol version 1: the libraries send `"protocol": 1` in every header. The CLI stops with `protocol_too_new` when a library is newer than it, and still reads older ones.
- `ddp history` works on iOS. The library records every change, tap, swipe and screen for the last 15 minutes, like on Android.
- History throttles to 4 captures a second (was 10) while only a clock or spinner changes, and detects those animations even when they skip captures. About 4% CPU on the samples with a clock that never stops.
- History stamps a touch when the finger went down, so a swipe shows before what it caused.
- The libraries have unit tests (`ChangeTracker`, frame signatures, gzip), run in CI.
- Code layout: `checks/` has one file per check; capture code moved to `Capture.kt` and `Capture.swift`.
- docs/README.md, STATUS.md, DECISIONS.md, a decision log and ADRs 0001 to 0006.
- README rewritten with real output, prompts to give an agent, MCP setup and a FAQ.
- ARCHITECTURE.md, docs/protocol.md, CONTRIBUTING.md, SECURITY.md and AGENTS.md for contributors.
- CI also checks `rustfmt` and `clippy`, and builds the iOS and Android libraries.

## 0.1.4 (2026-10-23)

- npm is published from GitHub Actions with Trusted Publishing and provenance.

## 0.1.3 (2026-10-23)

- The npm package shows the README.

## 0.1.2 (2026-10-23)

- Fix the JitPack build.

## 0.1.1 (2026-10-23)

- The release workflow builds the CLI binaries. JitPack takes its version from the tag.

## 0.1.0 (2026-10-23)

First public release.

- `ddp` CLI as one native Rust binary, on npm as `agent-ddp`. iOS library by SwiftPM, Android library by JitPack.
- Live mode: `connect`, `tree`, `find`, `get`, `read`, `watch`, `eval`, `time`, `net`, `tap`, `wait`, `logs`, `set`, `open`, `annotate`, `explore`.
- Checks: overlap, corners, jank, settle, a11y, timeline, parity, baseline, all. Each exits 1 on a problem.
- `ddp history` (Android): the last 15 minutes from a flight recorder in the library.
- For agents: guides inside the CLI (`ddp skills`), `doctor`, errors with a next step, `@eN` refs, sessions, `batch`, output limits and an MCP server.
