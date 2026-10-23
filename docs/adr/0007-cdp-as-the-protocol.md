# 0007 The library speaks the Chrome DevTools Protocol

Status: accepted, 2026-10-23. Amends 0001 (the endpoint) and the "the library reads" rule in ARCHITECTURE.md.

## Decision

The library inside the Debug build serves the Chrome DevTools Protocol itself: the same transport, messages, domain and method names, parameters and events as Chrome. `ddp` becomes one CDP client among others.

- Transport as in Chrome: HTTP discovery (`/json/version`, `/json/list`) and JSON-RPC over a WebSocket. On Android an abstract socket named `<package>_devtools_remote`, the name `chrome://inspect` looks for. On the iOS simulator a TCP port on 127.0.0.1, written to `Library/Caches/agent-ddp/cdp.json`.
- Messages as in Chrome: `{"id", "method", "params"}` answered by `{"id", "result"}` or `{"id", "error": {"code", "message"}}`, events as `{"method", "params"}`. An unknown method answers `-32601` like Chrome does.
- Each domain keeps its CDP names and does the native thing underneath: DOM is the view tree, `getComputedStyle` in the JavaScript DOM the drawn values (no CSS domain, see the decisions log), Network and Fetch the app's own client, DOMStorage UserDefaults and SharedPreferences, Input touches inside the app.
- What CDP has no name for (per-frame fills, borders, what covers what, the flight recorder) goes in one vendor domain, `DDP`.
- `Runtime.evaluate` runs real JavaScript: JavaScriptCore on iOS (part of the system), QuickJS on Android, compiled with the NDK into a separate artifact the app adds with `debugImplementation`. Scripts reach the app through one `app` object with the same API on both platforms (`app.fetch`, `app.storage`, `app.element`, `app.action`), and through `ObjC` and `Java` bridges for platform code.
- Debugger, Profiler and HeapProfiler are made for JavaScript sources the app does not have. They answer "not supported".
- The library may change app state in Debug through the domains that exist for it (Input, Fetch, DOMStorage, Emulation, Runtime). Release builds still contain none of it.

## Why

- Any CDP client can attach: Chrome DevTools through `chrome://inspect`, and tools that already speak CDP. Agents know CDP from the browser; they do not have to learn a second protocol.
- What only the app can do (send requests with its tokens and pinning, mock responses, read its storage) needs code inside the app anyway. CDP already has names for all of it.
- JavaScript lets an agent combine steps in one call. JavaScriptCore costs nothing on iOS; QuickJS is about 1 MB per CPU type in the Debug APK and runs inside production apps through Frida and Cash App's Zipline. A JavaScript engine written in Java (Rhino) has no real `async`/`await`; compiling Kotlin per script needs the SDK and seconds per call; a Rust core would share only the protocol, since reading views stays in Swift and Kotlin.

## Patterns

Dispatch table per domain (method name to handler) on each platform, like the v1 commands. Adapter: each domain maps native objects to CDP shapes. Facade: the `app` object in JavaScript.

## Consequences

- Security on iOS follows Chrome's model: discovery needs no token, so `chrome://inspect` can use the port. The server refuses a `Host` other than localhost (DNS rebinding) and a WebSocket `Origin` other than `devtools://` (web pages). Android keeps the uid check (adb and root only).
- Until the CLI moves to CDP, the v1 JSON lines endpoint stays next to it, unchanged. The v1 endpoint is removed once `ddp` reads everything through CDP; that change raises the protocol version (ADR 0004).
- The library implements HTTP and WebSocket by hand on both platforms, with no dependency. QuickJS is the only third-party code, vendored at a pinned version, used only by `Runtime.evaluate`.
- The CDP server is written twice, in Swift and Kotlin. If that duplication becomes a problem, the shared part can move to Rust later.
