# Changelog

Changes that users or agents will notice. Before 1.0, a minor version may change the protocol or the output; those changes are marked.

## 0.2.0 (not released yet)

The library now serves a control endpoint with names taken from Android and iOS (ADR 0008), and the CLI uses only that. Agents can act on the app, not only read it: touch and type, answer or fail its requests, edit its preferences and run commands the app adds in Debug. `ddp tap` checks first that a touch reaches the element.

### Upgrading

- Update the CLI and the library together. CLI 0.2.0 speaks only the control endpoint, which libraries before 0.2.0 do not have: `ddp connect` and `ddp doctor` say so with the code `library_too_old`. CLI 0.1.x cannot reach a 0.2.0 library and says `can't reach the app`. Recordings and history files from 0.1.x still open in 0.2.0.
- Scripts that read `ddp tap` output, or expect it to always succeed, see the new output and errors below. `ddp tap --force` is the old behaviour.

### Breaking

- Protocol version 2. The v1 JSON lines endpoint is gone from both libraries: iOS no longer writes `live.json`, Android no longer opens the socket `agentddp.<package>`. `app.info` and every header say `"protocol": 2`.
- `ddp tap` asks the app's hit test before it touches. It waits up to 2 s for the element to hold still, be enabled and have a point a touch reaches, and fails with `disabled`, `covered`, `untouchable`, `outside` or, on Android, `intercepted` (a parent takes the touch on its way down) otherwise. After the touch it prints what changed, or `nothing changed within 1000ms; look again before a second tap`.
- `ddp tap` on iOS is a real touch, as on Android, instead of the element's accessibility action. Elements with no accessibility action can now be tapped; the button's pressed look shows.

### Added

- Control endpoint in the library: JSON-RPC over a WebSocket. Android: the abstract socket `agentddp.<package>.control`, open to adb and root only. iOS simulator: the port in `Library/Caches/agent-ddp/control.json`. Requests from web pages (with an `Origin`) and with a foreign `Host` are refused. Methods, documented in docs/protocol.md:
  - `app.info`, `app.commands`, `app.run`: the app's details, and named actions the app adds in Debug with `AgentDDP.command` (iOS) or `AgentDdpCommands.add` (Android), in its own Swift or Kotlin.
  - `screen.elements`, `screen.capture`, `screen.frame`, `screen.watch`, `screen.value`: the elements, an image of every window, one frame, a frame stream and one property.
  - `screen.hit`: where a touch on an element would land, from the platform's own hit test (UIKit, SwiftUI's gesture hit test, View dispatch, Compose `LayoutNode.hitTest`), without touching. On Android it also finds a parent that takes the touch first: an `onInterceptTouchEvent`, a Compose `PointerEventPass.Initial` consumer.
  - `input.touch`, `input.type`, `input.key`: real touches and typing, delivered the way the system delivers them, so gesture recognizers, SwiftUI and Compose see a real finger.
  - `net.list`, `net.get`, `net.watch`, `net.send`, and `net.intercept` with `net.continue`, `net.respond`, `net.fail`, `net.release`: the request log, a request sent with the app's own client, and pausing the app's requests to let them go on changed, answer them or fail them the way the app's client fails (`offline`, `timeout`, ...).
  - `log.watch`: the app's log lines (os_log on iOS, logcat on Android).
  - `prefs.files`, `prefs.read`, `prefs.write`, `prefs.remove`, `prefs.clear`: UserDefaults and SharedPreferences. A write keeps the type the key holds; secrets read as `[redacted]`.
  - `time.*`, `history.files`, `history.read`: the virtual clock and the flight recorder's files.
- Elements carry `disabled` when the platform says so.
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
