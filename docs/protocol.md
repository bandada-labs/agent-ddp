# DDP protocol

This is the contract between the library inside the app and any client. The `ddp` CLI is one client. Another tool can speak the same protocol without it.

Version: 0 (before 1.0). Fields may be added in any release. Removing or renaming a field or a command is listed in [CHANGELOG.md](../CHANGELOG.md).

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
{"type": "header", "platform": "ios", "unit": "pt", "scale": 3, "frames": 240, "startedAt": 1761210000000}
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

## History (Android)

The flight recorder writes one file per minute to `files/agent-ddp/history/<startedAt>.jsonl`, gzipped when closed, kept for 15 minutes. Each file is a `header` with `"history": true` and its `startedAt`, then `frame` and `event` lines with `t` in ms since `startedAt`, then `end` when the file closes. A frame is kept only when something changed. An event is `{"type": "event", "kind": ...}` where `kind` is `tap` or `swipe` (with its coordinates), `screen` (with the activity `name`) or `launch`.
