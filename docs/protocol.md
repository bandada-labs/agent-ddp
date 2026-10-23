# DDP protocol

This is the contract between the library inside the app and any client. The `ddp` CLI is one client. Another tool can speak the same protocol without it.

Version: 1. The `header` of every stream, recording and history file carries it as `"protocol": 1`. A header without the field comes from a library older than the field and reads as 0.

- Adding an optional field does not change the version. A client must ignore fields it does not know.
- Changing what a field means, or removing a field or a command, raises the version and is listed in [CHANGELOG.md](../CHANGELOG.md).
- A client stops when the version is higher than the one it knows, instead of guessing. The `ddp` CLI fails with code `protocol_too_new` and tells the user to update. It still reads every lower version.

## Chrome DevTools Protocol endpoint

The library is moving to the Chrome DevTools Protocol (ADR 0007). Both endpoints run side by side until the `ddp` CLI has moved; everything below this section is the v1 JSON lines endpoint.

Discovery and messages are the same as Chrome's: `GET /json/version` and `GET /json/list` (or `/json`), then a WebSocket at the `webSocketDebuggerUrl` from the list, carrying `{"id", "method", "params"}` and answering `{"id", "result"}` or `{"id", "error": {"code", "message"}}`. An unknown method answers `-32601` with `'<method>' wasn't found`.

- Android: the abstract socket `agentddp_<package>_devtools_remote`, listed by `chrome://inspect`. From a host: `adb forward tcp:9222 localabstract:agentddp_<package>_devtools_remote`. Only the shell user and root may connect.
- iOS simulator: TCP on 127.0.0.1, the port in `Library/Caches/agent-ddp/cdp.json`. A `Host` other than localhost, 127.0.0.1 or ::1 gets a 500, a WebSocket `Origin` other than `devtools://` gets a 403, as in Chrome.

The app is one target with id `app`, type `page`, and the URL `android-app://<package>` or `ios-app://<bundle id>`.

`/json/version` points at `/devtools/browser/app`, the list at `/devtools/page/app`. Both reach the same page. The difference is the one Chrome has: on the browser connection `Target.setAutoAttach` attaches the page, on the page connection it attaches nothing.

Coordinates are CSS pixels the way the platform's browser counts them: points on iOS, dp on Android. Screenshots are in device pixels.

Methods today:

- `Browser.getVersion`, `Schema.getDomains`.
- `Target.getTargets`, `getTargetInfo`, `setDiscoverTargets`, `attachToTarget`, `detachFromTarget`, `setAutoAttach`. Sessions are flat (`flatten: true`): a message with a `sessionId` gets it back on its answer. Attaching sends `Target.attachedToTarget` first, as Chrome does.
- `enable` and `disable` of Accessibility, DOM, Log, Network, Page and Runtime. Network and Log send events; the others do not send any yet. `Runtime.runIfWaitingForDebugger`.
- `Accessibility.getFullAXTree`: a `RootWebArea` node named after the app, with one child per element. `role` is the element's role, `name` its label or else its id. Node ids are positions in that read and hold only until the screen changes.
- DOM: `getDocument` (`depth`, default 1, -1 for all), `describeNode`, `getAttributes`, `getOuterHTML`, `querySelector`, `querySelectorAll`, `getBoxModel`, `requestChildNodes`. The tree is `#document > html > body`, then one node per element in the same order as the accessibility tree, so `backendDOMNodeId` points at the same node. The tag is the role (`text` for StaticText); the attributes are `role`, `id` and `label` on both platforms. Selectors are compound selectors with tag, `#id`, `[attr]` and `[attr=, ^=, $=, *= value]`, in comma lists. The tree has no view hierarchy yet, so combinators fail with `DOM Error while querying`. The box model has no padding, border or margin: all four quads are the frame.
- Network: `enable` sends `requestWillBeSent`, `responseReceived` and `loadingFinished` or `loadingFailed` for each request the app's Debug network decorator reports (the same requests as `ddp net`), with the `sessionId` that enabled it. The decorator only hears of a request when it ends, so the events of one request arrive together; `timestamp` and `wallTime` are seconds since 1970, start and end. `getResponseBody` and `getRequestPostData` return the redacted text, cut at 16 KB, never base64. Request ids are the `ddp net` ids (`n1`).
- Log: `enable` replays the last 100 entries, as Chrome replays its buffer, then sends `Log.entryAdded` for each new one. iOS reads the app's process log (os_log, Logger, NSLog; not `print`) once a second, with the category before the text. Android reads `logcat --pid` of the app, with the tag before the text. The same lines `ddp logs` shows.
- `Page.captureScreenshot`: `format` png or jpeg, `quality`, `clip`. Every window of the app, like a frame capture. On Android, SurfaceView and TextureView content is not drawn.

The rest of the domains come in the order in [STATUS.md](STATUS.md).

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
