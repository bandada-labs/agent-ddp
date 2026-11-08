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
| `app.info` | | `protocol` (2), `platform`, `app` (bundle id or package), `name`, `unit`, `scale`, `screen` (`[width, height]` in the frames' unit: the screen on iOS, the activity window on Android) |
| `app.commands` | | `commands`: the names the app added with `AgentDDP.command` (iOS) or `AgentDdpCommands.add` (Android) |
| `app.run` | `name`, `params` (object) | `value`: what the command returned, any JSON value. Commands are the app's own Swift or Kotlin code (log in a test user, reset state, open a screen) and run off the main thread |
| `app.launch` | `names` (array) | `received`: the names the app was started with, as the platform gave them: environment variables on iOS, extras of the running activity's intent on Android. Never the values. `ddp launch --env` asks it |
| `net.cookies` | | `cookies`: `[{name, domain, path, secure, expires}]`, never the values: iOS `HTTPCookieStorage.shared`, Android the default `java.net.CookieManager` (`unread` when the app set none) |
| `net.clearCookies` | | `removed`: how many it deleted from that store |
| `screen.highlight` | `rect` (`[x, y, w, h]` in the frames' unit), `ms` (default 3000) | Draws a box over that place for `ms`: iOS in a window of its own that takes no touches and that ddp's reads leave out; Android in the activity window's `ViewOverlay` (it takes no touches; pixel checks meanwhile see it) |
| `media.list` | | `players`: `[{source, state, position, duration, frame}]` (seconds), every drawn `AVPlayerLayer` (with `rate`, `url` redacted; a hidden or clear one and what is under it are left out) or shown `VideoView`; iOS also `nowPlaying` (`title`, `artist`, `album`, `duration`, `position`, `rate`) from the now-playing info the app gave the system. Android's `nowPlaying` (`source: "media session"`, `state`, `position`, `rate`, `description`) is not the library's: the CLI reads the app's first active media session from `dumpsys media_session`, so it needs no protocol change and works without the library |
| `perf.metrics` | `ms` (100..60000, default 5000) | Answers after `ms`: `refreshMs`, `refreshes` (vsync callbacks on the main thread: Choreographer, CADisplayLink), `missed` (refreshes it missed: a gap of two periods is one), `worstGapMs`, `fps` (vsync callbacks a second, over their own span); `drawMeanMs`, `drawP95Ms`, `drawnOverRefresh` (over one refresh period): Android the app's work per frame drawn (FrameMetrics: input, animation, layout, draw, sync, commands; not the GPU wait) with `drawn` (frames drawn); iOS `drawn: null` (iOS does not tell an app which refreshes drew) and the main run loop's busy time per refresh, from waking to sleeping after Core Animation's commit. History captures nothing meanwhile |
| `storage.files` | `path` (default the home folder) | `path`, `entries`: `[{name, folder, bytes}]` under the app's home (iOS `NSHomeDirectory()`) or data folder (Android `dataDir`). A path that leads outside it is an error |
| `storage.read` | `path` | `bytes`, `truncated` (over 256 KB), and `text` redacted as in `net.*`, or `binary: true` |
| `storage.remove` | `path` | Removes a file or folder in the app's folder, never the folder itself |
| `storage.query` | `path`, `sql` | `columns`, `rows` (up to 200), `more`. The database is opened read-only, so SQLite itself refuses a write. Text redacted, a column with a secret name masked, blobs as their size |
| `storage.keys` | | `keys`: `[{store, service, name}]`, Keychain items by class (iOS) or Keystore aliases (Android), never their data. `unread` when the Keychain refused to answer, so what it holds is unknown |
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
| `screen.hit` | `id`: element name; none for every element with `tap`. `touch`: the smallest touch size in the frame's unit (0: not measured) | `elements`: each element, as in a `frame`, plus `points` (how many of 9 sample points a touch reaches, center first), `point` (the first such point), `reach` (`[width, height]` a touch reaches through its center, when under `touch` either way), and when the center misses: `reason` (`disabled`, `covered`, `untouchable`, `outside`, `intercepted`) and `by` (what takes the touch, such as `TapGesture`, `pointerInput`, a view's id or class). `intercepted` (Android): the touch reaches the element but a parent takes it on its way down; `points` is 0. A control all under the keyboard, as the platform places it (UIKit's keyboard notifications, Android's IME insets from API 30), is `covered` by `the keyboard` with `points` 0. Nothing is touched. iOS asks UIKit's `hitTest` and, in SwiftUI content, the hosting view's gesture hit test with UIKit's touch radius (private API); the winning gesture is the control's when SwiftUI's responders say so; which gesture wins during a touch is UIKit's call at touch time and is not reported. Android walks View dispatch with ViewGroup's own methods (hidden API) and calls Compose's `LayoutNode.hitTest` (internal, by reflection), then asks each parent's `onInterceptTouchEvent` and Compose Initial pass with a press and a cancel. A point the platform cannot decide counts as reaching. A parent that takes only the release is not seen. Android also answers `origin`, the activity window's place on the screen |
| `screen.pixels` | `rects`: `[[x, y, width, height], ...]` in the frame's unit | `scale` (pixels per unit) and `regions`: per rectangle `x`, `y`, `width`, `height` in pixels and `rgba` (base64, sRGB), or `missing` with why. Only the app's own windows, as composited: `drawHierarchy` on iOS, `PixelCopy` on Android (any window from API 34, before it the windows with a `Window` object). iOS also answers `texts`: per text layer its `frame` and the screen's colors at its ink (`ink`) and clear (`clear`) points, from the layer's own alpha |
| `app.hooks` | | `hooks`: every engine piece the library reaches outside public API, with `kind` (`private`, `hidden`, `internal`), `feeds` (what depends on it), `status` (`ok`, `fallback`, `mixed`, `failing`, `unused`), the counts `ok`, `failed`, `fallback`, and the last `error` |
| `input.touch` | `phase` down, move or up; `x`, `y` | One finger, delivered as the system delivers it: an IOHIDEvent handed to UIKit (private API) on iOS, a `MotionEvent` to the window under the point on Android. Gesture recognizers, SwiftUI and Compose see a real finger. Scroll with down, moves, up |
| `input.type` | `text` | Types into the focused field as the keyboard does. Nothing happens with no field focused |
| `input.key` | `key`: `Enter`, `Tab`, `Backspace` | Enter in a single-line field runs its action (Done, Search) |
| `screen.animations` | | iOS: `animations`, each `{key, type, duration, frame}` plus `keyPath`, `repeat` (a count or `forever`), `speed` (when not 1) and `id` (the view's accessibility identifier) when there is one: the Core Animation animations `animationKeys` lists on the app's layers. Android answers an error |
| `screen.source` | `id`: element name | `status` `ok` with `calls` (innermost first, each `{name, file, line, offset, length}` plus `library`, the androidx package, for a call made inside a library) and `modifiers` (chain order, `{name, ...plain values}`, `inspector: false` when the modifier has no inspector info); or `status` `none` with `why`. Android: the Compose compiler's source information from the slot table, read with the app's ui-tooling-data; a View answers `none`. Lines count from 1. iOS, SwiftUI (app started with `SWIFTUI_VIEW_DEBUG=1`): `status` `ok` with `element` (SwiftUI's type name), `views` (`{type, role}`: first the app view whose body wrote it, `role` `writes`, then the app views around that one, innermost first, `role` `holds`; the CLI adds `file`, `line` and `calls` from the Debug image) and `path` (the node types SwiftUI recorded, innermost first) and `modifiers` (SwiftUI's modifier types around it, innermost first, each proven by a `ModifiedContent` type above it). iOS, UIKit (app started with `MallocStackLogging`): `status` `uikit` with `class` and `address`; the CLI turns it into `created`, the app's frames of the allocation stack, `{function, file, line}` |
| `compose.recompositions.start` | `ms`: the window, 100..60000 | `compositions`: how many root compositions are observed. Android: installs a composition tracer and composition observers; they remove themselves 5 s after the window if no stop comes. iOS answers an error |
| `compose.recompositions.stop` | | `ms` measured, `composables`: most runs first, each `{name, runs}` (`name` is the compiler's `pkg.Function (File.kt:N)`) plus `invalidatedBy`, each `{state, times}` (`MutableState<Type>`, `State<Type>` for a derived state, or `invalidated directly`; never the value) |
| `screen.setValue` | `id`, `path` as in `screen.value`, `value`: a number, a Bool or `#RRGGBB(AA)` | iOS: sets it with KVC (a layer's model value; a color becomes a `CGColor` on a layer, a `UIColor` elsewhere) and answers `value` read back. A property without a setter is an error. Android answers an error: Compose takes its values from the app's state |
| `net.list` | | `requests`: the `request` summaries of `ddp net` |
| `net.get` | `id` | The full entry with headers and bodies, redacted, bodies cut at 16 KB. With the app's timing hook (`AgentDDP.timing`, `AgentDdpNetwork.timing`): `timing` with the phases in ms the platform measured, of `dns`, `connect` (without TLS), `tls`, `wait` (request sent to first response byte) and, on iOS, `download`; `protocol` (`http/1.1`, `h2`, `h3`) and `reused` (an open connection). iOS reads the last network transaction of URLSessionTaskMetrics, and sets `cache` (true) when URLSession answered from its cache without the network; Android gives none for a call that sent more than one request (a redirect, a retry) |
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
| `log.watch`, `log.unwatch` | | The last 100 lines, then each new one, as `log.entry` events: `{t, level, tag, text}`, and on iOS `subsystem` and `sender` (the library that wrote it) when the line has them. `t` is ms since 1970, `level` debug, info, warn or error, `tag` the logcat tag or the os_log category. iOS reads os_log, Logger and NSLog once a second (not `print`), without the lines the library itself causes (`com.apple.network` from its connections, `com.apple.Accessibility` from the accessibility tree it turns on); Android reads `logcat --pid` |
| `prefs.files` | | `files`: the preferences files (Android) or suites (iOS) on disk, the app's own included |
| `prefs.read` | `file`: none for the app's own preferences | `values`: key to text. Sets and arrays are JSON, dates ISO 8601, data base64. Secrets (keys like `token`, and registered values) read as `[redacted]` |
| `prefs.write` | `file`, `key`, `value` (text) | Keeps the type the key holds: a Boolean stays a Boolean, a set takes a JSON array. A new key is a string. `[redacted]` leaves the real value |
| `prefs.remove`, `prefs.clear` | `file`, and `key` for remove | |

Only requests through `AgentDDP.send` (iOS) or `AgentDdpInterceptor` (Android) are paused. Bodies in `net.*` are UTF-8 text. `app.run` and `net.send` answer when they finish, maybe after other messages, so match answers by `id`.

The rest comes in the order in [STATUS.md](STATUS.md).

### Runner (Android, apps without the library)

`android/runner` is an instrumentation that serves the same JSON-RPC over the abstract socket
`agentddp.runner.control` (shell user and root only), started with `am instrument -w dev.agentddp.runner/.Runner`.
It reads the platform's accessibility tree and touches through UiAutomation, so it answers what the system knows,
never the app's engine. Places are `x`,`y` in screen pixels or `label` (text, content description or resource id).
Every result has `ms`, the time spent on the device.

| Method | Params | Result |
| --- | --- | --- |
| `app.info` | | `platform`, `unit` (`px`), `scale`, `screen`, `app` (package in front) |
| `input.ready` | | nothing: fails as every `input.*` method does while the screen is off or locked (version 8) |
| `input.tap`, `input.doubleTap` | a place | |
| `input.longPress` | a place, `ms` (600) | |
| `input.drag` | `from`, `to` places, `ms` (300), `hold` (0), `still` (0: ms held at the end before letting go, so nothing flings) | |
| `input.type` | `text` | key presses; text the keyboard map has no keys for is set into the focused field at its cursor (`ACTION_SET_TEXT`) |
| `input.clear` | | empties the focused field: Ctrl+A, then Delete (fails when nothing has input focus) |
| `input.key` | `key`: `Enter`, `Tab`, `Backspace`, `Back`, `Escape`, `Up`, `Down`, `Left`, `Right`; `Home`, `Recents`, `Notifications` as global actions | |
| `screen.find` | `label` | `found`, `frame` |
| `screen.elements` | | `elements`: `type`, `label`, `id`, `frame` [x,y,w,h], `tap`, `enabled`; `shown: false` for one the app holds off screen (scrolled away, under the keyboard's pan); `parent`, the place in `elements` of the nearest listed node above it (version 4); `value` (a field's typed text; its `label` is the hint), `checked`, `selected`, `focused`, `password`, `error`, `expanded` (version 5); `window` (`id`, `title`, `frame`): the app's window it is in, read bottom first, and `under`, the frame of a higher window of the app over its middle (version 9) |
| `runner.stop` | | |

`app.info` answers `runner`, the runner's version (9). It takes UiAutomation with
`FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES`, so TalkBack and other services keep running while it does, and holds a
screen wake lock while it runs. Every `input.*` method fails while the screen is off or locked
(`the screen is locked: unlock the device`), and a key or touch the system refuses fails it. A ddp that finds a runner with another version running stops it
and installs its own. The runner ends itself after 5 minutes without a request: it holds UiAutomation, which one process
at a time may have, and the app's own UI tests need it.

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
- Android elements carry `parent`, the index in `elements` of the element whose subtree holds them (null at the top), and `visible` when their clips cut them (null: none of it shows). iOS elements are VoiceOver's leaves: they never nest, and SwiftUI's clips are not on the accessibility path, so iOS sends neither.
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
