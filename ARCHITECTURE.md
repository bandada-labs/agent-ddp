# Architecture

agent-ddp follows the Chrome DevTools model. The app carries a small server, a documented protocol sits in the middle, and any client can speak it. The `ddp` CLI is the client we ship.

```
 Debug build of the app                          host
┌──────────────────────────────┐   protocol   ┌───────────────────────────┐
│ library (iOS or Android)     │  WebSocket   │ ddp CLI (Rust)            │
│  capture: views, layers,     │◄────────────►│  live client, recordings, │◄── agent (shell or MCP)
│  Compose, draw ops, pixels   │  local only  │  queries, checks, history │
│  control endpoint, history   │              │  device ops: simctl, adb  │
└──────────────────────────────┘              └───────────────────────────┘
                 │ writes                                  │ reads
                 └──── recordings and history (.jsonl) ────┘
```

There are three parts. Each one can change on its own as long as the protocol holds.

1. The library, one per platform: `ios/AgentDDP` (Swift) and `android/agent-ddp` (Kotlin). It reads what the app draws on each frame and serves it.
2. The protocol: the commands, the messages and the recording format. It is the contract. See [docs/protocol.md](docs/protocol.md).
3. The CLI in `cli/`. It talks to the library, saves takes and turns frames into short text for agents.

## Rules every change keeps

- Debug only. On iOS everything is inside `#if DEBUG`, and outside Debug `start()` is empty. On Android the library is a `debugImplementation` dependency. A change that adds code to a Release build is a bug.
- The library reads unless a method exists to act: `input.*`, `net.intercept` and its answers, `prefs.write|remove|clear`, plus `tap`, `time` and the opt-in network log (ADR 0008). Anything else that changes app state needs its own reason in the PR.
- The core is Android and iOS. Names, types and units come from the platforms; a name is shared only when both platforms mean the same thing. No browser concepts (HTML document, CSS, localStorage) (ADR 0008).
- The main thread pays for every capture, so it stays cheap. The `end` message reports `averageCaptureMs` and `slowestCaptureMs`. A change to capture code shows these numbers before and after on a sample screen. The flight recorder costs nothing on a still screen and must keep it that way.
- The endpoint is local only: 127.0.0.1 on iOS (a `Host` other than localhost and any `Origin` get a 403), an abstract socket that accepts only the shell user and root on Android.
- One wire format for both platforms: the control protocol (ADR 0008). The CLI never branches on the platform to read a frame. It branches only for device work (`simctl` vs `adb`).
- No daemon. The library is the server, every `ddp` command is a short process, and a recording is a plain file. Every read and check works the same live or offline on a saved take.
- Output is for agents: short, stable and deterministic. Errors carry a stable `code` and a `next` command. Checks exit 1 when they find something. A change in output is a change in the contract and goes in the changelog.

## The library

Both platforms have the same pieces, with the same names where the language allows it.

| Piece | iOS | Android |
| --- | --- | --- |
| Entry point | `AgentDDP.start()` in `AgentDDP.swift` | `AgentDdpStarter`, a ContentProvider, in `AgentDdp.kt` |
| One frame | `FrameCapture` in `AgentDDP.swift`; `LayerReader`, `AccessibilityReader`, `PixelSampler` in `Capture.swift` | `FrameCapture` in `AgentDdp.kt`; `Capture`, `Hidden` in `Capture.kt`; `DrawRecorder` |
| Frames over time | `FrameRecorder` (offline), `FrameStream` (live) | same |
| Property reads (`screen.value`) | `LiveEval` in `Eval.swift` | same in `Eval.kt` |
| Network log | `NetworkLog`, `Redactor` in `Network.swift` | `AgentDdpNetwork`, `AgentDdpInterceptor`, `NetworkLog` in `Network.kt` |
| Virtual clock | `TimeControl` in `Time.swift` | `AgentDdpTime`, `TimeControl` in `Time.kt` |
| Control endpoint (ADR 0008) | `ControlServer`, `ControlSession` in `Control.swift` | same in `Control.kt` |
| Acting and scripts | `Input`, `Fetch`, `Preferences`, `AppLog`, `Commands`, `AppClient` (in `Network.swift`) | `Input`, `Fetch`, `Preferences`, `AppLog`, `AgentDdpCommands`, `AppClient` (in `Network.kt`) |
| Flight recorder | `History`, `ChangeTracker`, `FrameSignatures` in `History.swift` and `ChangeTracker.swift` | `History`, `ChangeTracker` in `History.kt` and `ChangeTracker.kt` |

The iOS library has no dependencies. The Android one depends on Compose UI (the app's version wins) and compiles against OkHttp only for the optional interceptor. New dependencies in the library need a strong reason, because they end up in every app that adds it.

## The CLI

One binary, one module per area. `main.rs` maps each command to its handler.

| Module | What it owns |
| --- | --- |
| `args.rs` | every command, its options and help text |
| `state.rs` | per-command context (session paths, `--json`), errors with `code` and `next`, output limits |
| `live.rs` | the protocol client, `connect`, live-only commands (`watch`, `eval`, `net`, `time`) |
| `device.rs` | `simctl` and `adb` work: `record`, `tap`, `wait`, `logs`, `set`, `open`, `annotate`, `explore` |
| `model.rs` | loading recordings, naming elements, which element owns each node |
| `queries.rs` | `tree`, `get`, `find`, `diff` and `@eN` refs |
| `summary.rs` | `read`, sparklines, `get --curve` |
| `checks/` | one file per check family, `mod.rs` holds `check all` and the list of checks |
| `history.rs` | `ddp history` over the flight recorder files |
| `tools.rs` | `view`, `mcp`, `skills`, `doctor`, `batch` |
| `py.rs` | number and JSON printing. The first CLI was Python, and the output stays byte for byte the same |

The guides in `docs/skills/` and `docs/agents.md` are compiled into the binary, so `ddp skills get core` always matches the installed version.

## How a command runs

`ddp read --for 2s --then "ddp tap animatedButton"`, live:

1. `live.rs` opens the control endpoint from `.ddp/session.json` (`control.rs`, a WebSocket) and calls `screen.watch`.
2. The library starts a `FrameStream` on the main thread and sends `screen.stream` events: a `header`, then one `frame` per drawn frame.
3. On the first frame the CLI runs the `--then` program without a shell.
4. After 2 s the CLI closes the stream, saves the take to `.ddp/last.jsonl` and a gzipped copy in `.ddp/recordings/`.
5. `summary.rs` reads the take through `model.rs` and prints one block per element.

Offline, step 1 to 4 are replaced by `--file <recording>`.

## Patterns in use

Use a pattern when it removes branching or duplication, not to add a layer. These are already in the code; copy them when the same problem comes up. A new ADR names the ones it uses.

| Pattern | Where | Why |
| --- | --- | --- |
| Facade | `AgentDDP` (iOS), `AgentDdpNetwork`, `AgentDdpTime` (Android) | A small public API over internal capture code |
| Strategy | `OfflineDevice` with `IosDevice` and `AndroidDevice` (`live.rs`) | Offline recording works the same on both platforms |
| Dispatch table | `handler()` in `main.rs` | One map from command to handler; `batch` and `mcp` reuse `run_command` |
| Decorator | `TouchWatcher` over `Window.Callback` (Android); the output sinks in `state.rs` | Adds behavior without touching what it wraps |
| Chain of responsibility | `AgentDdpInterceptor` in OkHttp | Network logging as one more link in the app's own chain |
| Observer | draw listener, run loop observer, `NetworkLog.watch`, lifecycle callbacks | The library reacts to the app instead of polling |
| Adapter | `ControllableFrameClock` (Compose clock), `py.rs` | Fits a foreign interface: Compose's clock, Python's printing |
| Producer-consumer | the outbox queue in Android `ControlConnection`; the history writer queues | The main thread never waits on a socket or a file |
| Errors as values | `Failure` with `code` and `next` | Every error tells the agent what to run next |

## Adding things

A new field in a frame:

1. Capture it on both platforms, or document why one platform does not have it.
2. Add it to [docs/protocol.md](docs/protocol.md) as optional. Old CLIs ignore fields they do not know.
3. Read it in `model.rs` and show it where it helps (`tree`, `read`, a check).
4. Add a case to the sample apps and a test in `cli/test_ddp.py`.

A new command: add it in `args.rs` and `main.rs`, put its logic in the module that owns the area, document it in `docs/agents.md` and in the guide that should teach it, and cover it in `cli/test_ddp.py`. If it needs the app, add the protocol command on both platforms first.

A new check: one function in `checks.rs` that returns how many problems it found. `run_check` turns that into the exit code and `--json`.

## Versions

All parts share one version and one tag (`vX.Y.Z`). The tag builds the CLI binaries, publishes npm, and is the version JitPack and SwiftPM resolve. Until 1.0 a minor version may change the protocol or the output. Every such change is listed in [CHANGELOG.md](CHANGELOG.md).

## Decisions

Why things are the way they are: [docs/adr/](docs/adr/README.md). What the maintainer decided and what is in progress: [docs/README.md](docs/README.md).
