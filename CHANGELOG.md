# Changelog

Changes that users or agents will notice. Before 1.0, a minor version may change the protocol or the output; those changes are marked.

## Unreleased

- New control endpoint in the library (ADR 0008), next to the v1 one: JSON-RPC over a WebSocket, with names taken from the app. Android: the abstract socket `agentddp.<package>.control`. iOS simulator: the port in `Library/Caches/agent-ddp/control.json`. Web pages may not connect. `app.info` answers `"protocol": 2`. The v1 endpoint is unchanged.
- `screen.elements` and `screen.capture`: the elements of the screen, in the same shape as in frames, and an image of every window.
- `input.touch`, `input.type`, `input.key`: real touches and typing inside the app, in points on iOS and pixels on Android. Taps, long presses and drags reach gesture recognizers, SwiftUI and Compose.
- `net.list`, `net.get`, `net.watch`: the requests `ddp net` sees, redacted the same way.
- `net.intercept`, `net.continue`, `net.respond`, `net.fail`: pause the app's requests, then let them go on changed, answer them, or fail them the way the app's code sees a failure (`offline`, `timeout`). New on iOS: `AgentDDP.send(request) { ... }` for the app's Debug network decorator.
- `log.watch`: the app's own log lines (os_log on iOS, logcat on Android).
- `prefs.files`, `prefs.read`, `prefs.write`, `prefs.remove`, `prefs.clear`: UserDefaults and SharedPreferences. A write keeps the type the key holds; secrets read as `[redacted]`.
- `app.commands`, `app.run`: named actions the app adds in Debug with `AgentDDP.command` (iOS) or `AgentDdpCommands.add` (Android), in its own Swift or Kotlin.
- `screen.frame`, `screen.watch`, `screen.value`, `time.*`, `history.files`, `history.read`: what the v1 endpoint gives (snapshot, stream, eval, time) and the flight recorder's files, over the control endpoint.
- `net.send`: a request sent with the app's own client, logged and pausable like the app's requests. On iOS, the closure given to `AgentDDP.send` is now `@escaping`, because `net.send` reuses the last one.

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
