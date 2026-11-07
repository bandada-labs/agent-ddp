# Changelog

Changes that users or agents will notice. Before 1.0, a minor version may change the protocol or the output; those changes are marked.

## Unreleased

### Added

- Every action says what it changed, with refs: `tap`, `swipe`, `drag`, `double-tap`, `long-press` (and `type`, `press` without the library) wait for the screen to settle (the first change within 1 s, then until it stops, at most 3 s) and print `changed`, `new` and `gone` with the `@eN` of each, so the next step needs no `tree`. An element keeps its ref for the session: `tree` no longer renumbers refs an agent may still hold.
- `ddp walk`: a task walked step by step and kept as a file. `walk start "<task>"`, then every action is a step with what it touched, what changed and a screenshot taken in the background; `walk expect <element>` adds a check; `walk end pass|fail` prints one line per step. `walk replay` does it again with no model: targets are found again by id or label, never by pixels, and a replay fails only when a target or an expected element is not on screen. `walk parity` compares the last iOS and Android replays step by step. ddp calls no model; the agent walking is the model.
- App lifecycle without adb: `ddp stop` (force-stop / terminate), `ddp clear` (the app's data, like a fresh install), `ddp logs --since-launch` (the app process's lines since it started, no take needed) and, on Android, `ddp test`: the app's instrumentation tests with `FAIL` lines and passed, failed and skipped counts, exit 1 on a failure. `test` frees UiAutomation from ddp's runner first.
- `ddp perf` prints fps (the main thread's vsync callbacks a second) and, on iOS, the main thread's work per refresh (mean, p95, how many over a refresh): the run loop's busy time up to Core Animation's commit. iOS gives no frames-drawn count, so the work is per refresh. Missed refreshes are now counted out of all refreshes in the window, not out of those the main thread ran. New `perf.metrics` field `fps`; iOS fills `drawMeanMs`, `drawP95Ms` and `drawnOverRefresh`.
- `ddp compare --on A,B --structure` works without the library: a side can be a device id, and when either app has no library both come from the platform's accessibility tree (merged labels split, a text field's placeholder counted, only text fields told apart as a kind).
- `ddp doctor` starts the app when it is not running, or brings it to the front (never a restart), and the session follows the new run.
- `ddp launch` with an app that has no library keeps a session through the platform, as `ddp up` does, instead of failing.
- `ddp media` on Android shows the app's media session as the system lists it (a Media3 `MediaSession` too): state, position, rate and the system's description, also without the library (`--device`, `--app`).
- `ddp up` with an app that has no library keeps a session for it: actions and `tree` work without `--device`, and commands that need the library say so.
- `ddp up --platform ios|android [--install <build>] [--app <id>]`: a simulator or emulator of ddp's own for a run. iOS clones a base simulator (`ddp-base`, made and booted once on first use); Android starts the base AVD (`ddp_base`) read-only from its quickboot snapshot on the first free even port from 5560, never 5554-5558. Then it installs and starts the app and connects. `ddp down [<device>] [--all]` shuts down and removes only devices `ddp up` made, listed in `.ddp/devices.json`.
- Android apps without the library: `tap`, `double-tap`, `long-press`, `swipe`, `drag`, `type`, `press` and `tree` with `--device` go through ddp's runner, an instrumentation inside the ddp binary that it installs and starts when needed. Touches are the system's own input (a tap in about 35 ms, `adb shell input` took 70-115 ms), and a place can be a label or resource id from the platform's accessibility tree, not only `x,y`. iOS simulators work the same way, from the Mac, with nothing installed in the simulator and no XCTest: touches and keys go into the simulator's own HID service and `tree --device` reads its accessibility service (two private hooks, `simulator.hid` and `simulator.accessibility`, asked by `ddp doctor`). Before, iOS refused `--device`. Nothing is checked without the library: the output says `no checks (no SDK)`. Double tap now works too.

### Fixed

- Android: `doctor` said "agent-ddp library in the build" for an app without it on a phone that also had ddp's sample: the check matched any package named in the dump. It now looks for the library's starter in the app's own manifest, and `launch`/`up` do not wait 15 s for an app without it.
- iOS: `tap` refused SwiftUI's `VideoPlayer` ("taking no touch") although a tap shows its controls. AVKit puts an empty SwiftUI view over the video and takes the tap with UIKit recognizers on a view around it; the hit test now counts those for what is inside that view and shown. A hidden control in there (the volume while the controls are hidden) is still refused.
- iOS: `ddp media` listed a second player, paused with nothing loaded, for every SwiftUI `VideoPlayer`: AVKit's hidden spare layer. Hidden and clear layers, and what is under them, are left out.
- `ddp media` prints the now-playing fields in one order (iOS hands them over in none).
- iOS without the library: `tree --device` showed SwiftUI's navigation bar and tab bar (iOS 26 and later) as one empty `Group`, so Close, toolbar buttons and tabs could not be tapped by name. Their children now come from the full accessibility tree.
- Without the library, a label or id that names more than one element (the tab "Sets" and a bar titled "Sets") touched the first one. It is now refused with the refs to pick from.
- Android without the library: an element that scrolled out of view, or that the keyboard pushed off screen, was reported `gone`. It now counts as moved, as with the library; the runner marks such elements `shown: false` and `tree` leaves them out. A runner left running by another ddp is replaced.
- Android library: nodes under `clearAndSetSemantics` were elements (with `tap`) although TalkBack never reads them. They are now paint of the element that cleared them.
- `long-press` and `double-tap` with the library did not wait for the hit test as `tap` does, so a press on a covered element went to the cover. They now refuse it the same way (`--force` presses anyway).
- `type --into` on an Android Compose field returned before the field held the text. It now waits until the field's text changed (2 s at most, and says so).
- iOS `set text-size largest` said done and changed nothing: simctl ignores names it does not know. `default` and `largest` now work on both platforms, and a name the platform does not take is refused.
- iOS without the library: after an `xcodebuild test` UI test run, `tree --device` showed 0 elements (or only the status bar) for every app until the simulator rebooted. XCTest sets the simulator's `ApplicationAccessibilityEnabled` to false when it ends. ddp now reads that preference on each look, turns it back on when it is off, says so, and reads the tree again (about 2.5 s, once).
- `check overlap` said COVERED for a text its list had scrolled almost away (7 px left at the list's edge): the strip held none of its glyphs. A text cut by its container is no longer judged.
- Without the library, an action on an element failed at once when the tree did not list it at that moment, as while an iOS context menu opens. It now waits up to 2 s for the element, as `tap` with the library does.
- Android without the library: ddp's runner kept running after the last command and held UiAutomation, so the app's own UI tests on that device failed with "UiAutomationService already registered". It now ends itself after 5 minutes without a command, and `ddp disconnect [--device <device>]` stops it at once.
- iOS without the library: a tap on an element below the fold (a long consent sheet at the largest text size) touched outside the screen and said "nothing changed". VoiceOver lists a scroll view's whole content; ddp now marks elements with no part on screen `shown: false` as on Android, leaves them out of `tree`, and refuses a touch on one with its frame. An element partly on screen has the frame of that part.
- Without the library, an element whose id showed up between two looks (SwiftUI's tab buttons, a moment after launch) was reported gone and new. The same label, type and place is now one element.
- iOS `set locale es-ES` wrote only `AppleLanguages`, so dates and numbers kept the old region's format (`AppleLocale` stayed `en_CO`). It now writes both (`es_ES`).
- iOS without the library: a tap on an element under a bar (a chat's composer over the end of the list) touched the bar and said "nothing changed", or opened the keyboard. The touch now goes to the point UIKit's accessibility says reaches the element (its visible point), and is refused with `is covered` when none does; `tap --force` touches the middle anyway.
- iOS without the library: the keyboard's prediction bar came as plain buttons. Every element in the keyboard's window is now `window: "keyboard"` in `tree --json`.
- iOS without the library: in a full screen cover over a sheet, VoiceOver's flat tree has no navigation bar, so Back and the title had no `parent`. `tree` now lists named containers the flat tree leaves out from the full tree, as their elements' parent (also the keyboard's `Typing Predictions`).
- `--help` said `--device` was Android only on `tap`, `type`, `long-press` and others; it works on iOS simulators too.

### Added (beta.2)

- `tree --device --json`: each element's `parent` (the key of the nearest listed element above it) on iOS and Android, and on iOS its UIAccessibility `traits` (`keyboardKey`, `selected`, `header`, ...), so a checker can tell the keyboard and the bars from the app's own elements. Android runner version 4.

- Android library: each element has its `parent` from the semantics and View trees and, when a clip or the window cuts it, `visible` (null when none of it shows). `tree` nests by that parent, and `tree --json` gives both, so a message scrolled half out of its list still has the list as parent. Before, the parent came from frame containment and was null there. iOS sends neither (its elements are VoiceOver's leaves).
- `wait id|text`, `wait --settled` and `find text|id` take `--device`: they read the platform's accessibility tree for an app without the library.
- `tree --json` with the library has the screen size and the scale (`screen`, `scale`), so a dp or pt check needs no `wm size`.

## 0.4.0 (2026-11-03)

- `ddp recompositions [--for 3s] [--then CMD]` (Android): which composables ran in the window, how often, and which state objects made Compose run them again; androidx's own composables are counted and listed with `--full`. The counts come from the trace calls the Compose compiler writes in every composable body, the causes from Compose's composition observer. A tracer the app has (Perfetto's runtime-tracing) keeps working. Two new engine hooks: `compose.tracer`, `compose.wrappedComposition`.
- `ddp source <element>`: which line of the app's code made an element. Android: the composable calls above it with file and line, then its modifiers in chain order, from the source information the Compose compiler keeps in Debug builds (Layout Inspector's data). Google's `ui-tooling-data` reads it from the app's own copy, so ddp ships no Compose library of its own. Two new engine hooks: `compose.inspectionTables`, `compose.sourceInformation`. iOS: for a UIKit view, the app's functions that created it, with file and line, from its allocation stack (launch with `--env MallocStackLogging=1`). For a SwiftUI element, the app view whose `body` wrote it and the views around that one, with the line of each `body` and the modifier calls in it (launch with `--env SWIFTUI_VIEW_DEBUG=1`). ddp matches SwiftUI's view debug tree against the static type of each view's `body`, and answers only when exactly one view can have written the element; otherwise it says the view is not known. The app answers in about 160 ms on the sample screen. Two new engine hooks: `swiftui.viewDebugData`, `swiftui.viewTypes`.
- Android: a Compose `Icon` or `Image` is a `bitmap` node, with the painter's class and the icon's `tint`. Before, a glyph inside an `IconButton` had no node, so nothing showed under a badge drawn over it.
- Fixed: `tree --within` left out what draws inside the element's frame but belongs to another element, such as an unlabeled overlay on an icon inside a button. Those nodes now show after the element, each with its owner (`inside` in `--json`).
- Fixed: Android `record --then "am instrument ..."` said "no recording arrived". The instrumentation starts a new app process after the request went out. ddp now runs the `--then` command while it waits and asks again each second until the app starts recording.
- `ddp connect --wait 15s` keeps trying while the app starts, for a test run that opens the app's endpoint seconds later. `ddp launch` uses the same wait.
- Fixed: on iOS a layer under a filter, a blend or a material (a toolbar item under Liquid Glass) showed its own fill, white, as if it were the drawn color. Colors from before an effect now show as `fill before effects` and `color before effects`, and no state, query or check reads them. This applies on both platforms.
- Fixed: Android `check a11y` said NO LABEL for an `IconButton` named only by its `Icon`'s `contentDescription`. A control that merges its children and has no text now takes the description Compose merged into it, the one TalkBack reads.
- `ddp skills get core` now starts with which command answers which question: slow, leaking, crashed, platform warnings, a breakpoint, the app's storage and network, motion, and device actions. Before, most of the 0.3.0 commands were only in `--full`. The `ddp --help` summary lists every command, and each MCP tool says what its command does instead of pointing to the guide.

## 0.3.0 (2026-10-30)

- iOS needs Xcode 16.4 or later (was 15.4). GitHub retires its last runner with Xcode 15.4 on November 2, and 16.4 is the oldest Xcode left there with an iOS simulator, so older ones can no longer be checked.
- iOS: the shapes inside a SwiftUI `Canvas` or `drawingGroup` are `draw` nodes, like Android's Canvas replay: circle, rect, rounded rect, oval, path or line, with bounds, fill or stroke, and color alpha (a dimmed context too). They come from RenderBox, SwiftUI's renderer, through a new engine hook, `renderbox.displayList`, which turns on RenderBox's XML recorder for the app run (Debug only). Text and images in a Canvas are not read yet and are counted as `unread`. Measured on the sample: no change in capture time (5.6-6.1 ms a frame) or app CPU and memory.
- Fixed: `check a11y` said SMALL TARGET for a control partly past the screen's edge, such as a list row half scrolled off; the edge cut its reach, not its size. Such a control is no longer measured.
- `ddp skills get app`: stepping back in an app with a pure engine, through an app `replay` command; ddp does not model the engine.
- `ddp debug <FILE:LINE> [--for 30s] [--then CMD]`: stop once at a line with the platform's debugger (LLDB on the simulator, jdb over JDWP on Android), print the stack and the locals, then the app runs on. Exits 1 when the line does not run in time.
- `ddp animations` (iOS): the Core Animation animations on screen, by layer, with key path, class, duration and repeat. `ddp eval <id> layer.speed --set 0` holds one. Android refuses: Compose animations are not listed by the platform.
- `ddp eval <id> <path> --set VALUE` (iOS): try a style live, a number, `true`/`false` or a color set on the element or its layer, then read back from UIKit. Android refuses: Compose takes its values from the app's state.
- `ddp launch --arg` (iOS, repeatable): a launch argument as an Xcode scheme passes it, such as `-_UIConstraintBasedLayoutLogUnsatisfiable YES` so iOS 27 logs Auto Layout conflicts for `ddp audits`. Android refuses it and points to `--env`.
- Android: a layer dimmed in a `graphicsLayer { alpha = ... }` block now shows its opacity; only `graphicsLayer(alpha = ...)` and `Modifier.alpha` were read before. The block runs on Compose's own `GraphicsLayerScope()`, so it reads the same animated state.
- `ddp net get` shows a request's phases (`timing`: dns, connect, tls, wait, and download on iOS), its `protocol` and whether it `reused` a connection, when the app passes `AgentDDP.timing` to URLSession (iOS) or `AgentDdpNetwork.timing` as OkHttp's event listener factory (Android). On iOS a response from URLSession's cache says `cache`.
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
- `ios/build-swiftc.sh`: builds the iOS library as a static library and module with plain `swiftc`, for apps built without an Xcode project or SwiftPM. CI builds it, and the iOS sample app, on Xcode 16.4, the oldest Xcode supported.
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

- Every action says what it changed, with refs: `tap`, `swipe`, `drag`, `double-tap`, `long-press` (and `type`, `press` without the library) wait for the screen to settle (the first change within 1 s, then until it stops, at most 3 s) and print `changed`, `new` and `gone` with the `@eN` of each, so the next step needs no `tree`. An element keeps its ref for the session: `tree` no longer renumbers refs an agent may still hold.
- `ddp walk`: a task walked step by step and kept as a file. `walk start "<task>"`, then every action is a step with what it touched, what changed and a screenshot taken in the background; `walk expect <element>` adds a check; `walk end pass|fail` prints one line per step. `walk replay` does it again with no model: targets are found again by id or label, never by pixels, and a replay fails only when a target or an expected element is not on screen. `walk parity` compares the last iOS and Android replays step by step. ddp calls no model; the agent walking is the model.
- `ddp up` with an app that has no library keeps a session for it: actions and `tree` work without `--device`, and commands that need the library say so.
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

- Every action says what it changed, with refs: `tap`, `swipe`, `drag`, `double-tap`, `long-press` (and `type`, `press` without the library) wait for the screen to settle (the first change within 1 s, then until it stops, at most 3 s) and print `changed`, `new` and `gone` with the `@eN` of each, so the next step needs no `tree`. An element keeps its ref for the session: `tree` no longer renumbers refs an agent may still hold.
- `ddp walk`: a task walked step by step and kept as a file. `walk start "<task>"`, then every action is a step with what it touched, what changed and a screenshot taken in the background; `walk expect <element>` adds a check; `walk end pass|fail` prints one line per step. `walk replay` does it again with no model: targets are found again by id or label, never by pixels, and a replay fails only when a target or an expected element is not on screen. `walk parity` compares the last iOS and Android replays step by step. ddp calls no model; the agent walking is the model.
- `ddp up` with an app that has no library keeps a session for it: actions and `tree` work without `--device`, and commands that need the library say so.
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
