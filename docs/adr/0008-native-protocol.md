# 0008 A native protocol over a WebSocket; agent-browser is a guide, not a spec

Status: accepted, 2026-10-23. Supersedes 0007.

## Decision

The library serves its own protocol, built from what Android and iOS have, over a WebSocket. It does not imitate a browser.

- Transport: JSON-RPC messages over a WebSocket, as in ADR 0007, with no dependency. `{"id", "method", "params"}` is answered by `{"id", "result"}` or `{"id", "error": {"code", "message"}}`. Events are `{"method", "params"}` with no `id`. The JSON-RPC error codes stay: -32700, -32600, -32601 for an unknown method, -32000 for anything else.
- Method names are native and short: `screen.elements`, `screen.capture`, `input.touch`, `net.intercept`, `prefs.write`. The full list is in [protocol.md](../protocol.md).
- Values keep native types and units: points on iOS, pixels on Android, the same as the frames. Preferences keep Bool, Int and sets. Errors are named the way the app code sees them (`offline`, `timeout`).
- A name is shared between the platforms only when both mean the same thing (`fill`, `border`, `corner`, `opacity` in frames). Otherwise the platform's own name is used. No third vocabulary is taken from the browser: no HTML document, no CSS, no localStorage.
- The `ddp` CLI takes agent-browser's good ideas as a guide: short commands, `@e` refs, small text output, `batch`. Compatibility with agent-browser or Chrome is not a goal.

## Why

ADR 0007 picked CDP so that browser clients could attach without changes. The maintainer does not want 1:1 compatibility, so that benefit is gone. What was left was cost: a fake `#document > html > body` over the element list, CSS selectors, filler fields that Chrome needs (`loaderId`, `referrerPolicy`), "localStorage" for preferences, and a planned JavaScript DOM with `getComputedStyle`. Each one is a translation that someone has to learn and keep working, and it hides what the app really has.

The native cores built for ADR 0007 stay: real touches, request pausing in the app's own client, the app's log, window screenshots, typed preferences. Only their names and message shapes change. Removing the browser layer took out about 600 lines.

## Patterns

Dispatch table (method name to handler) on each platform, as in v1 and ADR 0007. Each method calls a native module (`Input`, `Fetch`, `NetworkLog`, `AppLog`, `Preferences`) and only converts JSON.

## Consequences

- The v1 JSON lines endpoint stays until the CLI reads everything through this one, then it is removed and the protocol version goes to 2 (ADR 0004). Done in phase 7 (2026-10-23): one version, 2, in `app.info` and in every header.
- The phases of ADR 0007 keep their order, numbered in STATUS.md: 1 transport, 2 reading, 3 acting, 4 scripts, 5 frames and history, 6 the CLI, 7 v1 goes. Phase 4 decided against JavaScript: a script is an app command, a named Swift or Kotlin closure the app adds in Debug (`app.run`). Android has no JavaScript engine without a new dependency, and JavaScript could not call the app's own code without a bridge per object.
- Chrome DevTools and other CDP clients cannot attach. That was never a goal.
