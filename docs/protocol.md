# DDP protocol

This is the contract between the library inside the app and any client. The `ddp` CLI is one client. Another tool can speak the same protocol without it.

Version: 1. The `header` of every stream, recording and history file carries it as `"protocol": 1`. A header without the field comes from a library older than the field and reads as 0.

- Adding an optional field does not change the version. A client must ignore fields it does not know.
- Changing what a field means, or removing a field or a command, raises the version and is listed in [CHANGELOG.md](../CHANGELOG.md).
- A client stops when the version is higher than the one it knows, instead of guessing. The `ddp` CLI fails with code `protocol_too_new` and tells the user to update. It still reads every lower version.

## Control endpoint

The library also serves a control endpoint (ADR 0008), built from what Android and iOS have, not from a browser. Both endpoints run side by side until the `ddp` CLI has moved; everything below this section is the v1 JSON lines endpoint.

- Android: the abstract socket `agentddp.<package>.control`. From a host: `adb forward tcp:9333 localabstract:agentddp.<package>.control`. Only the shell user and root may connect.
- iOS simulator: TCP on 127.0.0.1, the port in `Library/Caches/agent-ddp/control.json`. A `Host` other than localhost, 127.0.0.1 or ::1 gets a 403.
- Connect with a WebSocket upgrade on any path. A request with an `Origin` header (a web page) gets a 403.

Messages are JSON-RPC: `{"id", "method", "params"}`, answered by `{"id", "result"}` or `{"id", "error": {"code", "message"}}`. Events are `{"method", "params"}` with no `id`. Codes: -32700 not JSON, -32600 no integer `id`, -32601 unknown method, -32000 anything else, with a message for a person.

Coordinates are window coordinates in the platform's unit, the same as in frames: points on iOS, pixels on Android. `app.info` gives the unit and the scale.

| Method | Params | Result or events |
| --- | --- | --- |
| `app.info` | | `protocol` (2), `platform`, `app` (bundle id or package), `name`, `unit`, `scale` |
| `app.commands` | | `commands`: the names the app added with `AgentDDP.command` (iOS) or `AgentDdpCommands.add` (Android) |
| `app.run` | `name`, `params` (object) | `value`: what the command returned, any JSON value. Commands are the app's own Swift or Kotlin code (log in a test user, reset state, open a screen) and run off the main thread |
| `screen.elements` | | `elements`, the same shape as in a `frame` |
| `screen.capture` | `format` png or jpeg, `quality`, `clip` `[x, y, width, height]` | `data`, base64, in device pixels. Every window of the app, dialogs included. On Android, SurfaceView and TextureView content is not drawn |
| `input.touch` | `phase` down, move or up; `x`, `y` | One finger, delivered as the system delivers it: an IOHIDEvent handed to UIKit (private API) on iOS, a `MotionEvent` to the window under the point on Android. Gesture recognizers, SwiftUI and Compose see a real finger. Scroll with down, moves, up |
| `input.type` | `text` | Types into the focused field as the keyboard does. Nothing happens with no field focused |
| `input.key` | `key`: `Enter`, `Tab`, `Backspace` | Enter in a single-line field runs its action (Done, Search) |
| `net.list` | | `requests`: the `request` summaries of `ddp net` |
| `net.get` | `id` | The full entry with headers and bodies, redacted, bodies cut at 16 KB |
| `net.watch`, `net.unwatch` | | `net.request` event with each summary as the request ends |
| `net.intercept` | `urls`: wildcards with `*` and `?`; none pauses every request | `net.paused` event: `{id, request: {url, method, headers, body}}`, redacted |
| `net.continue` | `id`, and any of `url`, `method`, `headers` (object), `body` (text) | The request goes on. A value sent back as it was shown, such as a `[redacted]` header, keeps its real value |
| `net.respond` | `id`, `status`, `headers`, `body` (text) | The app gets this answer; nothing is sent |
| `net.fail` | `id`, `error`: `offline`, `host`, `timeout`, `refused`, `unreachable`, `lost`, `cancelled`, `failed` | The app gets the error its client would throw: a `URLError` on iOS, the OkHttp `IOException` on Android |
| `net.send` | `url`, and any of `method`, `headers` (object), `body` (text) | `status`, `headers`, `body`, redacted. Sent with the app's own client: the last one that ran through `AgentDDP.send` or `AgentDdpInterceptor` (URLSession.shared or a plain OkHttp client before that), so the app's interceptors add what they add. It shows in `net.list` and can be paused like any request |
| `net.release` | | Paused requests go on unchanged. The same happens when the client disconnects. One client intercepts at a time: the last `net.intercept` wins |
| `log.watch`, `log.unwatch` | | The last 100 lines, then each new one, as `log.entry` events: `{t, level, tag, text}`. `t` is ms since 1970, `level` debug, info, warn or error, `tag` the logcat tag or the os_log category. iOS reads os_log, Logger and NSLog once a second (not `print`); Android reads `logcat --pid` |
| `prefs.files` | | `files`: the preferences files (Android) or suites (iOS) on disk, the app's own included |
| `prefs.read` | `file`: none for the app's own preferences | `values`: key to text. Sets and arrays are JSON, dates ISO 8601, data base64. Secrets (keys like `token`, and registered values) read as `[redacted]` |
| `prefs.write` | `file`, `key`, `value` (text) | Keeps the type the key holds: a Boolean stays a Boolean, a set takes a JSON array. A new key is a string. `[redacted]` leaves the real value |
| `prefs.remove`, `prefs.clear` | `file`, and `key` for remove | |

Only requests through `AgentDDP.send` (iOS) or `AgentDdpInterceptor` (Android) are paused. Bodies in `net.*` are UTF-8 text. `app.run` and `net.send` answer when they finish, maybe after other messages, so match answers by `id`.

The rest comes in the order in [STATUS.md](STATUS.md).

## Transport

Every message is one JSON object on one line, ending with `\n`. A client opens one connection per command.

| | iOS simulator | Android |
| --- | --- | --- |
| Endpoint | TCP on 127.0.0.1, random port | abstract unix socket `agentddp.<application id>` |
| Discovery | the app writes `{"port", "token"}` to `Library/Caches/agent-ddp/live.json` in its data container | `adb forward tcp:0 localabstract:agentddp.<application id>` |
| Auth | the token from `live.json` | the socket accepts only uid 2000 (shell, so adb) and 0 (root). The token line is still sent and ignored |

A session is:

1. The client sends `{"token": "<token>"}`.
2. The client sends one command: `{"cmd": "<name>", ...}`.
3. The app answers with message lines. A finished answer ends with `{"type": "end"}`. A stream runs until the client closes the connection.
4. An empty line is a keep-alive and is skipped.

When something fails the app sends `{"type": "error", "error": "<message for a person>"}` and the answer is over.

## Commands

| `cmd` | Fields | Answer |
| --- | --- | --- |
| `snapshot` | | `header`, one `frame`, `end` |
| `stream` | `frames`: number, 0 for no limit | `header`, then one `frame` per drawn frame, `end` with stats when `frames` is reached |
| `eval` | `id`: element name, `path`: property path | `{"type": "value", "path", "value"}` |
| `tap` (iOS) | `id`: element name | `{"type": "value", "path": "tap", "value": <bool>}`. Runs the accessibility action. Android taps through `adb shell input` instead |
| `time` | `action`: `pause`, `resume`, `step`, `rate`, `seek`, `status`; `value`: number for `step`, `rate` and `seek` (ms) | `{"type": "time", "rate", "paused", ...}` |
| `net` | | one `request` summary per logged request, `end` |
| `net-get` | `id`: request id | the full `request` entry with headers and bodies |
| `net-watch` | | one `request` summary per request as it finishes, until the client closes |

`eval` paths. iOS: `element.<key path>` on the accessibility element, `layer.<key path>` on the topmost layer with the element's frame. Android: `<modifier>.<property>` from the Compose inspector (`border.width`) or `semantics.<Key>`. An unknown name answers with an error that lists what is there.

## Messages

### header

```json
{"type": "header", "protocol": 1, "platform": "ios", "unit": "pt", "scale": 3, "frames": 240, "startedAt": 1761210000000}
```

`unit` is `pt` on iOS and `px` on Android. `scale` converts between them. `startedAt` is ms since 1970.

### frame

```json
{"type": "frame", "i": 0, "t": 0,
 "elements": [{"id": "flashBox", "label": "Flash", "frame": [x, y, w, h], "tap": true}],
 "nodes": [{"z": 5, "kind": "layer", "frame": [x, y, w, h], "fill": "#RRGGBBAA",
            "border": {"width": 2, "color": "#RRGGBBAA"}, "corner": 12}]}
```

- `i` is the frame index, `t` the ms since the first frame.
- `elements` are what a person would tap or read: accessibility elements on iOS, semantics nodes and Views with a test tag, text or content description on Android.
- `nodes` are what was drawn, in paint order. A higher `z` paints over a lower one. Only nodes that draw something are listed.
- Coordinates are window coordinates in `unit`.

The full list of element and node fields, and which platform fills each one, is in [agents.md, Recording format](agents.md#recording-format-json-lines). Optional fields appear only when they apply. A client must ignore fields it does not know.

### end

```json
{"type": "end", "frames": 240, "averageCaptureMs": 2.07, "slowestCaptureMs": 4.27}
```

The capture stats are the main thread time the library spent per frame.

### request

```json
{"type": "request", "id": "n12", "method": "GET", "url": "https://...", "status": 200, "ms": 84, "responseBytes": 512, "startedAt": 1761210000000}
```

`net-get` adds headers and bodies. Redaction happens inside the app before the entry is kept: `Authorization`, `Cookie`, any header, query item or JSON key that matches `secret|token|key|password|attest`, and values the app registers. Bodies are cut at 16 KB.

## Recordings

A recording is the `header`, `frame` and `end` lines of a stream saved to a file, plain or gzipped. Every client feature that reads frames must work the same on a recording as live.

Without a live connection, a recording can be asked for directly:

- iOS: write `{"frames": N}` to `Library/Caches/agent-ddp/request.json` and post the Darwin notification `dev.agentddp.record` (`xcrun simctl spawn <udid> notifyutil -p dev.agentddp.record`). The app writes `recording.jsonl` next to it.
- Android: `adb shell am broadcast -a dev.agentddp.RECORD -p <application id> --ei frames N`. The app writes `files/agent-ddp/recording.jsonl`, read with `run-as`.

## History

The flight recorder writes one file per minute to `history/<startedAt>.jsonl` next to the recordings (`Library/Caches/agent-ddp/` on iOS, `files/agent-ddp/` on Android), gzipped when closed, kept for 15 minutes. Each file is a `header` with `"history": true` and its `startedAt`, then `frame` and `event` lines with `t` in ms since `startedAt`, then `end` when the file closes. A frame is kept only when something changed. An event is `{"type": "event", "kind": ...}` where `kind` is `tap` or `swipe` (with its coordinates, `t` at touch-down), `screen` (with a `name`: the Android activity, or the iOS top view controller's title or class) or `launch`.
