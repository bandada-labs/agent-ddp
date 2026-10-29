# DDP protocol

This is the contract between the library inside the app and any client. The `ddp` CLI is one client. Another tool can speak the same protocol without it.

Version: 2. `app.info` and the `header` of every stream, recording and history file carry it as `"protocol": 2`. Version 2 removed the v1 JSON lines endpoint (`live.json` on iOS, the socket `agentddp.<package>` on Android). A header without the field comes from a library older than the field and reads as 0.

- Adding an optional field does not change the version. A client must ignore fields it does not know.
- Changing what a field means, or removing a field or a command, raises the version and is listed in [CHANGELOG.md](../CHANGELOG.md).
- A client stops when the version is higher than the one it knows, instead of guessing. The `ddp` CLI fails with code `protocol_too_new` and tells the user to update. It still reads every lower version.

## Control endpoint

The library serves one endpoint, the control endpoint (ADR 0008), built from what Android and iOS have, not from a browser.

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
| `app.launch` | `names` (array) | `received`: the names the app was started with, as the platform gave them: environment variables on iOS, extras of the running activity's intent on Android. Never the values. `ddp launch --env` asks it |
| `state.list` | | `values`: `[{name, writable}]`, the values the app added with `AgentDDP.state` (iOS) or `AgentDdpState.add` (Android); `writable` when it gave a setter |
| `state.get` | `names` (array, optional: all) | `values`: `{name: value}`, any JSON value, read on the main thread. An unknown name or a value that is not JSON is an error |
| `events.list` | | `events`: the last 200 the app sent with `AgentDDP.event` (iOS) or `AgentDdpEvents.send` (Android), oldest first: `{t, name, fields}`, `t` in ms since 1970. Each is also written to History as an `event` line with `kind: "app"`, `name` and `fields` |
| `events.watch` | | Sends the kept events, then each new one, as `events.entry` events with the same fields. `events.unwatch` stops it |
| `state.set` | `name`, `value` (any JSON) | `value`: what the getter reads back after the setter ran on the main thread. A value without a setter is an error |
| `screen.elements` | | `elements`, the same shape as in a `frame` |
| `screen.capture` | `format` png or jpeg, `quality`, `clip` `[x, y, width, height]` | `data`, base64, in device pixels. Every window of the app, dialogs included. On Android, SurfaceView and TextureView content is not drawn |
| `screen.frame` | | One `frame` line, as in a recording: `elements` and `nodes` |
| `screen.watch` | `frames`: number, 0 for no limit | `screen.stream` events: the `header`, `frame` and `end` lines of a recording, one frame per drawn frame. On Android a client that falls 64 frames behind ends the stream |
| `screen.unwatch` | | The stream ends with its `end` line |
| `screen.value` | `id`: element name, `path` | `path`, `value`: one property, read-only. iOS: `element.<key path>` or `layer.<key path>` (presentation values, so mid-animation). Android: `<modifier>.<property>` (`border.width`) or `semantics.<Key>` on the Compose node with that test tag or text |
| `screen.hit` | `id`: element name; none for every element with `tap`. `touch`: the smallest touch size in the frame's unit (0: not measured) | `elements`: each element, as in a `frame`, plus `points` (how many of 9 sample points a touch reaches, center first), `point` (the first such point), `reach` (`[width, height]` a touch reaches through its center, when under `touch` either way), and when the center misses: `reason` (`disabled`, `covered`, `untouchable`, `outside`, `intercepted`) and `by` (what takes the touch, such as `TapGesture`, `pointerInput`, a view's id or class). `intercepted` (Android): the touch reaches the element but a parent takes it on its way down; `points` is 0. Nothing is touched. iOS asks UIKit's `hitTest` and, in SwiftUI content, the hosting view's gesture hit test with UIKit's touch radius (private API); the winning gesture is the control's when SwiftUI's responders say so; which gesture wins during a touch is UIKit's call at touch time and is not reported. Android walks View dispatch with ViewGroup's own methods (hidden API) and calls Compose's `LayoutNode.hitTest` (internal, by reflection), then asks each parent's `onInterceptTouchEvent` and Compose Initial pass with a press and a cancel. A point the platform cannot decide counts as reaching. A parent that takes only the release is not seen. Android also answers `origin`, the activity window's place on the screen |
| `screen.pixels` | `rects`: `[[x, y, width, height], ...]` in the frame's unit | `scale` (pixels per unit) and `regions`: per rectangle `x`, `y`, `width`, `height` in pixels and `rgba` (base64, sRGB), or `missing` with why. Only the app's own windows, as composited: `drawHierarchy` on iOS, `PixelCopy` on Android (any window from API 34, before it the windows with a `Window` object). iOS also answers `texts`: per text layer its `frame` and the screen's colors at its ink (`ink`) and clear (`clear`) points, from the layer's own alpha |
| `app.hooks` | | `hooks`: every engine piece the library reaches outside public API, with `kind` (`private`, `hidden`, `internal`), `feeds` (what depends on it), `status` (`ok`, `fallback`, `mixed`, `failing`, `unused`), the counts `ok`, `failed`, `fallback`, and the last `error` |
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
| `time.pause`, `time.resume`, `time.status` | | `rate`, `paused`, and `sincePauseMs` while paused. The app's clock: animations, and timelines that read `AgentDDP.elapsedTime()` or `AgentDdpTime` |
| `time.step` | `frames` (1 if none) | Paused only |
| `time.rate` | `rate`: factor, 0 or more | |
| `time.seek` | `ms` from the pause point | Paused only |
| `history.files` | | `files`: the flight recorder's files, oldest first, the open one included |
| `history.read` | `file` | `data`: the file's bytes in base64, gzipped when the name ends in `.gz`. See History below |
| `log.watch`, `log.unwatch` | | The last 100 lines, then each new one, as `log.entry` events: `{t, level, tag, text}`. `t` is ms since 1970, `level` debug, info, warn or error, `tag` the logcat tag or the os_log category. iOS reads os_log, Logger and NSLog once a second (not `print`), without the lines the library itself causes (`com.apple.network` from its connections, `com.apple.Accessibility` from the accessibility tree it turns on); Android reads `logcat --pid` |
| `prefs.files` | | `files`: the preferences files (Android) or suites (iOS) on disk, the app's own included |
| `prefs.read` | `file`: none for the app's own preferences | `values`: key to text. Sets and arrays are JSON, dates ISO 8601, data base64. Secrets (keys like `token`, and registered values) read as `[redacted]` |
| `prefs.write` | `file`, `key`, `value` (text) | Keeps the type the key holds: a Boolean stays a Boolean, a set takes a JSON array. A new key is a string. `[redacted]` leaves the real value |
| `prefs.remove`, `prefs.clear` | `file`, and `key` for remove | |

Only requests through `AgentDDP.send` (iOS) or `AgentDdpInterceptor` (Android) are paused. Bodies in `net.*` are UTF-8 text. `app.run` and `net.send` answer when they finish, maybe after other messages, so match answers by `id`.

The rest comes in the order in [STATUS.md](STATUS.md).

## Messages

### header

```json
{"type": "header", "protocol": 2, "platform": "ios", "unit": "pt", "scale": 3, "frames": 240, "startedAt": 1761210000000}
```

`unit` is `pt` on iOS and `px` on Android. `scale` converts between them. `startedAt` is ms since 1970. `missed: true` says the frames count the refreshes the app missed (`missed` on a frame).

### frame

```json
{"type": "frame", "i": 0, "t": 0,
 "elements": [{"id": "flashBox", "label": "Flash", "frame": [x, y, w, h], "tap": true}],
 "nodes": [{"z": 5, "kind": "layer", "frame": [x, y, w, h], "fill": "#RRGGBBAA",
            "border": {"width": 2, "color": "#RRGGBBAA"}, "corner": 12}]}
```

- `i` is the frame index, `t` the ms since the first frame. `missed`: refreshes the main thread missed before this frame by the platform's own frame times (`CADisplayLink`, Choreographer), counting only what the app's work alone took, without the library's capture.
- `elements` are what a person would tap or read: accessibility elements on iOS, semantics nodes and Views with a test tag, text or content description on Android.
- `nodes` are what was drawn, in paint order. A higher `z` paints over a lower one. Only nodes that draw something are listed.
- Coordinates are window coordinates in `unit`.

The full list of element and node fields, and which platform fills each one, is in [agents.md, Recording format](agents.md#recording-format-json-lines). Optional fields appear only when they apply. A client must ignore fields it does not know.

### end

```json
{"type": "end", "frames": 240, "averageCaptureMs": 2.07, "slowestCaptureMs": 4.27}
```

The capture stats are the main thread time the library spent per frame.

### inspect

Written by the CLI, not the library: after the frames of a take a check recorded live, what the platform answered at its end (`screen.hit` with `touch`, `screen.pixels`, two `screen.frame`). `hit`, `frame`, `minimum`, and per text and per corner place what the app's pixels show: `texts` (iOS: `ink`, `clear` and their shares; Android: `colors`, `seen`, `background`, `uniform`) and `corners` (`color`, `near`, `screen`, `behind`). What changed between the two frames is left out. `hooks`, when one failed in this app run: the `app.hooks` entries with status `failing` or `mixed`. The checks read only this line for overlap, corners, touch size and contrast.

### request

```json
{"type": "request", "id": "n12", "method": "GET", "url": "https://...", "status": 200, "ms": 84, "responseBytes": 512, "startedAt": 1761210000000}
```

`net.get` adds headers and bodies. Redaction happens inside the app before the entry is kept: `Authorization`, `Cookie`, any header, query item or JSON key that matches `secret|token|key|password|attest`, and values the app registers. Bodies are cut at 16 KB.

## Recordings

A recording is the `header`, `frame` and `end` lines of a stream saved to a file, plain or gzipped. Every client feature that reads frames must work the same on a recording as live.

Without a live connection, a recording can be asked for directly:

- iOS: write `{"frames": N}` to `Library/Caches/agent-ddp/request.json` and post the Darwin notification `dev.agentddp.record` (`xcrun simctl spawn <udid> notifyutil -p dev.agentddp.record`). The app writes `recording.jsonl` next to it.
- Android: `adb shell am broadcast -a dev.agentddp.RECORD -p <application id> --ei frames N`. The app writes `files/agent-ddp/recording.jsonl`, read with `run-as`.

## History

The flight recorder writes one file per minute to `history/<startedAt>.jsonl` next to the recordings (`Library/Caches/agent-ddp/` on iOS, `files/agent-ddp/` on Android), gzipped when closed, kept for 15 minutes. Each file is a `header` with `"history": true` and its `startedAt`, then `frame` and `event` lines with `t` in ms since `startedAt`, then `end` when the file closes. A frame is kept only when something changed. An event is `{"type": "event", "kind": ...}` where `kind` is `tap` or `swipe` (with its coordinates, `t` at touch-down), `screen` (with a `name`: the Android activity, or the iOS top view controller's title or class) or `launch`.
