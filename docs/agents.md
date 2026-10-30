# ddp command reference

Sees what a Debug iOS or Android app draws, frame by frame: per element its frame, fill, border, corner, opacity, clipping, and what covers it. Live, like agent-browser over Chrome DevTools: the app keeps running, `ddp connect` attaches, and you query, watch, pause time and read network requests. agent-device stays the hands (tap, type); `ddp` is the eyes. Start with `ddp skills get core`; `ddp skills get reviewer` walks through a review step by step.

Works only in Debug builds. Release builds contain none of it.

## Live mode

```
ddp connect <sim UDID | adb serial> --app <bundle or application id>
ddp launch <device> --app <id> [--env KEY=VALUE ...]   start the app fresh with values for it, then connect
ddp state [list] | get <name> | set <name> <value> | watch <name> [--until '>100'] [--for 10s]   the app's own values
ddp events [--follow] [--for 10s] [--grep <text>]   events the app sent in Debug, also in ddp history
ddp crash [--device <id> --app <id>]               the app's last crash as the platform recorded it, with its stack
ddp perf [--for 5s]                                refreshes the main thread missed, worst gap; Android: draw time
ddp net cookies | clear-cookies                    the app's cookies by name and domain, never the values
ddp media                                          video players on screen and now-playing info, with their state
ddp storage [files [<folder>]] | read <file> | rm <file> | query <db> "<sql>" | keys   the app's own data
ddp launch <device> --app <id> --reset             clear the app's data first, like a fresh install
ddp doctor [<device> --app <id>]                  what is wrong between the device and a frame, with the fix, and every engine hook's state
ddp tree [-i] [-c] [--within <el>] [-d N]         what is on screen now, per element, with @refs
ddp find text <part of a label> | id <id>          elements by name, with @refs
ddp watch <element> [--until 'border>3'] [--for 5s] [--then "<cmd>"]
ddp get <element> [--for 2s] [--then "<cmd>"] [--from 0.5s --to 1.5s]
ddp find --where 'kind=draw and alpha>0' [--for 2s] [--then "<cmd>"] [--between 0.5s 1.5s]
ddp read | check overlap|corners|a11y|jank|all|timeline ... [--for 2s] [--then "<cmd>"]
ddp check tappable                                 live: every control a touch does not reach, and why
ddp record [--for 5s] [--then "<cmd>"] [--name N] saves .ddp/last.jsonl for offline queries
ddp eval <element> <path>                          one property, read-only
ddp time pause | resume | step [frames] | rate 0.1 | seek <time> | status
ddp net list | get <id> | watch [--until 'status>=400']
ddp net send <url> [--method POST] [--body <text>]  a request through the app's own client
ddp net mock <url pattern> [--status 503] [--body <text>] [--for 10s] [--limit N] [--then "<cmd>"]
ddp net fail <url pattern> [--error offline|timeout|...] [--for 10s] [--limit N] [--then "<cmd>"]
ddp tap <element> [--force]                        a real touch where it reaches the element; says what changed
ddp tap <x,y>                                      a real touch at a point, then what changed
ddp swipe <from> <to> [--for 300ms]                a quick move (scroll, page); places are elements or x,y
ddp drag <from> <to> [--for 600ms]                 hold 500ms, move slowly, let go (reorder, drag and drop)
ddp double-tap <place> | long-press <place> [--for 800ms]
ddp tap|swipe|drag|long-press <x,y> ... --device <adb serial>   without the library: Android adb input, screen pixels
ddp type <text> [--into <element>]                 type as the keyboard does, into the focused field
ddp press Enter | Tab | Backspace
ddp prefs [list | set <key> <value> | rm <key> | clear | files] [--in <suite or file>]
ddp run [<command> ['<params JSON>']]              run a command the app added in Debug; no name lists them
ddp wait id <id> | text <label> | --settled 300ms [--ignore <el>] [--timeout 10s]
ddp logs [--grep <text>]                           app log lines during the last take, at their time in it
ddp logs --follow [--for 10s] [--grep <text>] [--source]   the last 100 lines, then each new one; --source: subsystem and library (iOS)
ddp set appearance dark | text-size large | reduce-motion on | locale es-ES | location 4.71,-74.07 | orientation landscape
ddp biometric enroll | match | fail                Face ID on the simulator, a fingerprint on the emulator
ddp push <payload.json>                            iOS: a push as APNs would deliver it
ddp job <id>                                       Android: run one of the app's scheduled jobs now
ddp back | background [--for 2s]                   go back; leave the app for a while and come back
ddp audits [--since 5m]                            runtime warnings the platform wrote in the app's log
ddp memory [summary|leaks|heap]                    the app's memory now, its leaks (iOS), a heap dump
ddp profile [--for 3s]                             the functions the app spends its time in
ddp inspect [--for 30s]                            a person taps an element on the phone; ddp says which
ddp highlight <element> [--for 3s]                 a box over the element in the live app, for a person watching
ddp open <deep link or URL>
ddp annotate [out.png]                             screenshot with numbered boxes, label N is @eN
ddp compare --reference <image> [--out ddp-compare.png]   the screen against a reference screenshot
ddp compare <element> --reference <image>          one element against a reference image of it
ddp compare <element> --on ios,android             one element on two phones, pixel by pixel
ddp compare --reference a.png --screen b.png --max 5%   two saved screenshots; exit 1 over the caller's bar
ddp compare --on ios,android --structure           what each phone draws by label: only on one, other kind
ddp explore [--per 2s] [--skip <el> ...]           tap each control once, record, check all on each
ddp record --on ios,android --for 3s --then "<cmd with {platform}>"   both at once, then parity
ddp view [out.html] [--file rec]                   a local HTML page of a take, for people
ddp mcp [--tools core,live,checks,all]             the commands as MCP tools on stdio
ddp batch [--bail] "<cmd>" ...                    several commands in one call (or one per stdin line)
ddp skills list | get <name> [--full] [--filter <heading>]
ddp disconnect
```

An element goes by its id, its label, or an `@ref` from the last `tree` or `find`.

- `connect` saves `.ddp/session.json`. While it exists every command runs live; `--file <recording>` on a command reads that file instead (offline mode).
- Commands that look at a time window record it live first: `--for` (default 2 s) or `--frames`, and `--then` runs your action as soon as the first frame arrives. `--then` runs without a shell: one program and its arguments, no pipes, `;` or `$VAR`.
- `watch` prints only what changes, frame by frame, until `--until` is true (fields: `t present x y w h alpha fill border borderColor opacity draws`) or `--for` runs out.
- `eval` paths. iOS: `element.<key path>` on the accessibility element, `layer.<key path>` on the topmost layer with the element's frame, presentation values (`layer.shadowRadius`, `layer.opacity`). Android: `<modifier>.<property>` from the Compose inspector (`border.width`, `background.color`, `graphicsLayer.alpha`) or `semantics.<Key>` (`semantics.Selected`). An unknown name lists what is there.
- `time` runs one virtual clock. `pause` freezes it, `step 30` moves it 30 frames, `rate 0.1` slows it, `seek 500ms` / `seek -400ms` moves it relative to the pause point, `resume` lets it run. What follows it: iOS Core Animation / UIKit animations; Android Compose animations; on both, any app timeline that reads `AgentDDP.elapsedTime()` (iOS) or `AgentDdpTime.elapsedMillis()` (Android) from its clock port, and only those rewind. What does not: SwiftUI `withAnimation` (its clock is not reachable), Android View `ValueAnimator`s and system UI.
- `net` lists requests the app reported from its Debug network decorator: `list` (id, method, status, ms, size, URL), `get <id>` (headers, bodies), `watch`. Redaction happens inside the app before anything is kept: `Authorization`, `Cookie`, any header, query item or JSON key matching `secret|token|key|password|attest`, and literal values the app registers (its dev secret). Bodies are cut at 16 KB.
- `net mock` and `net fail` hold the requests that match the pattern (`*` is any text) and answer them for `--for`: `mock` with `--status` and `--body`, `fail` with the error the app's client throws (`offline`, `host`, `timeout`, `refused`, `unreachable`, `lost`, `cancelled`, `failed`). `--then` runs meanwhile, so `--then "ddp tap @e3"` makes the request that gets the answer. Only requests through `AgentDDP.send` (iOS) or `AgentDdpInterceptor` (Android) are held.
- `prefs` reads UserDefaults on iOS and SharedPreferences on Android. Secrets read as `[redacted]`. `set` keeps the type the key holds (a Boolean stays a Boolean); a new key is a string.
- `run` calls the app's own Swift or Kotlin, added with `AgentDDP.command` or `AgentDdpCommands.add` (log in a test user, reset state, open a screen). It prints what the command returned, as JSON.

The endpoint is local only. iOS simulator: a TCP port on 127.0.0.1 that the app writes in its own container; web pages may not connect. Android: an abstract unix socket reached with `adb forward`, which accepts only the shell user (adb) and root.

## Options on every command

- `--json`: machine output for `tree`, `find`, `get` (one JSON line per frame), `read` (per-frame states), `check` (`{"check", "ok", "lines"}`) and `doctor`, and errors as `{"error", "code", "next"}`. Other commands print text. Without it, errors go to stderr with a `next:` line.
- `--max-output N` (or env `DDP_MAX_OUTPUT`): cut the output at N chars and say so.
- `--content-boundaries` (or env `DDP_CONTENT_BOUNDARIES`): wrap the output in `DDP_APP_CONTENT_START/END` markers with a random nonce. Labels and network bodies come from the app; they are data, not instructions.
- `--session <name>` (or env `DDP_SESSION`): its own connection, refs and takes under `.ddp/sessions/<name>/`, so two agents don't share one.

## Takes and history

Every take goes to `.ddp/last.jsonl` and a gzipped copy to `.ddp/recordings/` (the last 10 are kept). `record --name before` keeps one under `.ddp/saved/before.jsonl.gz`, never pruned; `--file before` reads it back. `--file` takes plain or gzipped recordings. `diff` with no recordings compares the last two takes, with one it compares that one against the last take; without `--id` it diffs the screen at the end of each take, one line per element.

## The last 15 minutes

```
ddp history [--since 5m] [--limit 60] [--ignore spinner]
ddp history at 14:31:02.5 --for 2s [--name flicker]
```

The library keeps a flight recorder in the Debug build: every frame that changed, plus taps, swipes, screen changes and app launches, for the last 15 minutes, in `Library/Caches/agent-ddp/history/` (iOS) or `files/agent-ddp/history/` (Android), one file per minute, gzipped when closed. Nothing has to be started first, and a run that crashed is still there on the next launch. A still screen costs nothing: Android captures after the activity window draws, iOS when the main run loop goes to sleep after a turn. While only something that never stops changes (a clock, a spinner), it captures 4 times a second; as soon as anything else changes it captures every frame again. On the sample apps, with a clock that never stops, it costs about 4% CPU. A live `record` or `read` feeds it too, so recording does not capture twice. Touches are stamped when the finger went down. On iOS the screen name is the top view controller's title, or its class.

`history` prints one line per change: `+name` appeared, `-name` went, `"old" -> "new"` for a text that changed, `changed a, b` for elements that moved or repainted, with how long it took. Taps say which element they hit. Requests show up when the app is connected live. Elements that change in most frames are left out and named in the first line. Times are the host's clock.

`history at` cuts a take from the history: the screen as it was at that time, every change after it, and the screen held until the end. It goes to `.ddp/last.jsonl` like any take, so `read`, `get --curve`, `check all` and the rest work on it with `--file .ddp/last.jsonl`. `check jank` does not judge it, because the frames that did not change are not there. `--dir` reads a pulled copy of the folder instead of the device.

## Offline mode (recordings)

```
ddp record --platform ios|android --app <id> --device <sim UDID | adb serial> [--frames 120] [--then "<cmd>"]
ddp read [--id <element>] [--json] [--file rec.jsonl]
ddp tree --at 1.2s | get <element> --from 1s --to 2s | find --where ... --between 1s 2s   [--file rec.jsonl]
ddp check overlap|corners|a11y|jank|settle|all | check timeline --expect cases.json   [--file rec.jsonl]
ddp check parity --file ios.jsonl --against android.jsonl
ddp diff ios.jsonl android.jsonl --id <element> [--ignore frame]
```

Without a session, `record` asks the app by Darwin notification (iOS) or broadcast (Android, needs the DUMP permission adb has) and fetches the file with `simctl` / `run-as`. `--frames` is 1..600 there. Queries read `.ddp/last.jsonl` unless `--file` says otherwise. `--device` can come from env `DDP_DEVICE`.

## What the commands report

Every `check` exits 1 when it finds a problem and 0 when it finds none.

Two rules decide what a check may say (ARCHITECTURE.md). It reports a problem only when it is sure. And the answer comes from the platform itself, never from ddp's own model of it. Live, `check`, `explore` and `record` end each take by asking the app for those answers, between two frames (`inspect`, about half a second): its hit test for every control, with how far a touch reaches it, and the pixels its own windows show (`screen.pixels`: the render server on iOS, `PixelCopy` on Android). Only what is the same in both frames is judged from the pixels, and a system alert or notification over the app is not in them. The answers go into the take as an `inspect` line, so `--file` reads them too. A take without them (history, or a file recorded before 0.3.0) says `not judged` for overlap, corners, touch size, contrast and jank.

### read

`read` prints, per element, its last state and every property that changed: first and last value, a sparkline of numeric tracks, time window, min and max. `TRANSIENT` marks a value that is the same at start and end but different in between, such as a border that flashes during an animation. `--id` prints the per-frame timeline of one element instead. It also tracks `pixels` (main color and peak alpha) for SwiftUI `Canvas` / `drawingGroup`, sampled at up to 64 px a side, so a hairline reads fainter than it is.

### tree, get, find, diff

`tree` prints each element and the nodes it owns at one moment; `get` the per-frame rows of one element (repeated rows skipped); `find` the nodes matching `--where` (fields: `t z kind op owner source fill border borderColor corner opacity alpha x y w h`; operators `= != > < >= <= ~`, `~` is contains), with a count and the first `--limit` rows; `diff` one element across two recordings lined up on their first change, sizes in pt.

### check overlap

`check overlap` reports text something is drawn over, so it cannot be seen, from the app's pixels at the end of the take. iOS: the screen shows the same color where the text's own bitmap has ink as around it. Android: none of the text's colors shows anywhere in its frame. Text that is only partly covered, see-through, under a filter or blend mode, or in a text field is not judged.

```
COVERED coveredLabel: the screen shows #8E8E93FF where its text has ink and around it, so the text cannot be seen in [199, 352.33, 104.33, 20.33]
```

A control covered for touch is `check tappable`'s.

### check corners

`check corners` reports a fill, hatch or line drawn past an element's rounded corners. The recorded geometry says where to look (line ends, 8 points round ovals, rect corners, each corner with its own radius), and the app's pixels decide: the node's color must show there, and differ from what the screen shows right outside the corner. So a clip the renderer applied, or a square corner the app asked for, is never reported.

### check timeline

`check timeline` compares a recording with what should be on screen, and exits 1 when something is off. Per case it prints `ok`, `MISSING` (with why: not in the recording, too faint, or not at the expected frame), `LATE` or `EARLY` (by how many ms). Cases file:

```json
{"origin": "first-change", "toleranceMs": 50, "expect": [
  {"id": "finalGlow", "at": 2000, "until": 2400, "minAlpha": 0.5, "frame": [x, y, w, h], "tolerancePt": 4},
  {"text": "welcome back", "absent": true, "at": 0, "until": 800}
]}
```

`id` is the element id or label; `text` matches part of a label instead (any case). `"absent": true` turns a case around: the element must not be drawn between `at` and `until` (default: the end of the recording), and it reports `SHOWN` with how many frames and when. The absent window is narrowed by the tolerance at both ends, so a fade that ends on time does not count. `at` / `until` are ms from the origin: `first-change` (the action, default) or `start` (the first recorded frame). `until`, `minAlpha` (default 0.05, peak alpha of what the element owns), `frame` (in pt) and `tolerancePt` are optional. An element on screen from the first frame counts as on time for a case at 0. Record long enough to cover the last case: a recording that ends early reports the late cases as missing.

### get --curve

`get <element> --curve` splits each numeric track (x, y, w, h, alpha, border, opacity) into moves and prints `from -> to over <ms> from <ms>`, plus which named curve the recorded values match (`linear`, `easeIn`, `easeOut`, `easeInOut`, `fastOutSlowIn`) and by how much, or how far it overshoots. It is a comparison with named curves, not the curve the app asked for: SwiftUI and Compose do not say which one they run. A match off by more than 5% says `matches no named curve` with the nearest one. A flash that comes back is two moves, out and back.

### check a11y

`check a11y` reports:

- `SMALL TARGET`: a control a touch reaches over less than 44x44 pt (iOS) or 48x48 dp (Android), measured with the app's hit test from its center outward. SwiftUI hit-tests a touch with a radius (20 pt) and Compose widens small controls to 48 dp, so a small control alone is usually fine; one crowded by a neighbor is not. Controls no touch reaches, and disabled ones, are `check tappable`'s.
- `NO LABEL`: a control the accessibility tree gives no label.
- `LOW CONTRAST`: text under WCAG AA (4.5:1, or 3:1 for text of 18 pt, or 14 pt bold), from the app's pixels at the end of the take. iOS takes both colors from the screen, at the text's ink and around it; Android takes the text's color and the screen's background. A SwiftUI text's size is estimated, so only the 3:1 limit is used for it. Text of a disabled control is not judged, as WCAG asks. Text over an image or a gradient, in more than one color, or covered, is listed as not judged.

### tap, check tappable

`tap` asks the app's own hit test (`screen.hit`) where a touch on the element would land before it touches, the same calls a real finger goes through. iOS: UIKit's `hitTest`, and in SwiftUI content the gesture hit test with the radius UIKit gives a touch; whether the gesture that wins is the control's own is SwiftUI's answer too (the responders its accessibility node keeps). Android: the walk ViewGroup dispatch makes, with its own methods for order, transforms and scroll, and Compose's `hitTest`. When the platform cannot say (app code decides, or a hook does not work on that version), the point counts as reaching: nothing is reported that the platform did not decide. It waits up to 2 s for the element to hold still, become enabled and have a point a touch reaches (the center first, then 8 more points), then touches there and looks for a change for 1 s:

```
$ ddp tap @e1
tapped animatedButton at (201, 146); changed: animatedButton, flashBox
$ ddp tap trapCovered
ddp: trapCovered is covered by TapGesture: no point of it takes a touch
next: ddp check tappable, or ddp tap trapCovered --force to touch it anyway
```

The error codes are `disabled`, `covered` (something on top takes the touch), `untouchable` (nothing takes it: SwiftUI `allowsHitTesting(false)`, iOS `isUserInteractionEnabled = false`) `outside` (the element is drawn outside its parent's bounds, so the parent never passes the touch on) and `intercepted` (Android only: the touch reaches the element, but a parent's `onInterceptTouchEvent` or a Compose parent that consumes the press in `PointerEventPass.Initial` takes it on its way down). For `intercepted`, each parent is asked with a press and then a cancel, so nothing is clicked. `nothing changed within 1000ms` after a tap means the touch reached nothing that reacts, or the action is slow (a request): look again before a second tap. `--force` touches the center with no checks.

`check tappable` runs the same hit test on every control on screen and touches nothing:

```
$ ddp check tappable
DISABLED trapDisabled
COVERED trapCovered: covered by TapGesture, no point of it takes a touch
UNTOUCHABLE trapIgnored: taking no touch, no point of it takes a touch
OUTSIDE trapOutside: outside its parent (UIView bounds), no point of it takes a touch
7 of 11 controls take a touch at their center
```

`PARTLY` is a control whose center is blocked while other points take a touch; `tap` uses one of those. `explore` skips the controls a touch does not reach and says why.

### state

`state` reads values the app added in its Debug code, in its own language: `AgentDDP.state(name, get:, set:)` on iOS, `AgentDdpState.add(name, get, set)` on Android. ddp knows nothing about the app's model or engine; the app decides what to show. `ddp state` lists every value with `(writable)` when the app gave a setter. `set` sends the value as JSON when it parses (`500`, `true`, `{"a": 1}`), else as text, and prints what the getter reads back. `watch` reads every 50 ms and prints each change, the first 20 and then only how many more came; with `--until` it exits 0 when it holds and 1 when `--for` (default 10s) ends first. A bare `--until '>100'` is about the watched value; `'total>100 and mode=done'` names values. `ddp skills get app` is the guide for adding the lines.

`events` shows events the app sent in its Debug code: `AgentDDP.event(name, fields)` on iOS, `AgentDdpEvents.send(name, fields)` on Android, from any thread. The library keeps the last 200; `--follow` prints them, then each new one. Each event also goes into History, so `ddp history` prints `event <name> key=value ...` next to frames and taps, after a crash too.

### storage

`storage` reads the app's own data through the library, with paths from the app's home folder (iOS) or data folder (Android); a path that leads outside is refused. `files` lists a folder, `read` prints a file's text (50 lines unless `--limit`, from the first 256 KB, redacted as in `net`), `rm` removes a file or folder. `query` runs one SQL statement on a database opened read-only and prints 50 rows unless `--limit`, so SQLite itself refuses a write; a column with a secret name (token, password, key) is masked. `keys` lists Keychain items (class, service, account) or Keystore aliases, never their data. When the Keychain refuses (an unsigned build has no Keychain entitlement), it says `not all read`, not `no keys`.

`ddp launch --reset` clears the app's data before it starts: Android's `pm clear`, or on iOS the simulator's data container emptied (only a folder that is the app's data container on that simulator). The iOS Keychain lives outside the container and stays.

### perf

`perf` measures for `--for` (default 5s) whatever is on screen, from the platform's own clocks, without a recording. Both platforms count the refreshes the main thread missed from their vsync callbacks (Choreographer, CADisplayLink): a gap of two refresh periods is one missed. Android adds the app's work for each frame it drew (FrameMetrics: input, animation, layout, draw, sync and commands; mean, p95, how many took longer than a refresh). It leaves out the wait for the GPU and the buffer queue, so a pipelined frame, or an emulator drawing in software, is not counted as slow. iOS gives an app no such time, so it says not measured. A still screen draws nothing on Android: `no frame drawn` is not a problem. History captures nothing during the measure, so ddp's own cost is not in it. A measurement: it exits 0.

```
$ ddp perf --for 3s
3.0s: the main thread missed 12 of 178 refreshes (16.67 ms each), worst gap 217.07 ms; work per frame not measured (iOS gives an app none)
```

### crash

`crash` prints the app's last crash from the platform's own record, so it works after the app is gone and needs nothing in the app. iOS: the crash report macOS writes for a simulator app (`~/Library/Logs/DiagnosticReports/*.ips`, matched by bundle id and simulator), already symbolicated, plus the message the app logged as it died (`Fatal error: ...` or the uncaught exception's reason). It names the first frame in the app's own bundle. Android: the crash log buffer (`logcat -b crash`), for Java exceptions (with `Caused by:`) and native crashes (signal, abort message, backtrace). The first line says how long ago it was: an old crash is not the one you just caused. `no crash` means the platform kept none for this app on this device.

```
$ ddp crash
crash at 2026-10-29 15:50:49 (3 min ago) on ios 5A9A96DD-...: EXC_BREAKPOINT (SIGTRAP)
Fatal error: the sample crashed on purpose
first frame in the app: DDPSample.debug.dylib static SampleState.crash(_:) DDPSampleApp.swift:41
thread 0, com.apple.main-thread:
  libswiftCore.dylib _assertionFailure(_:_:file:line:flags:)
  DDPSample.debug.dylib static SampleState.crash(_:) DDPSampleApp.swift:41
  ...
report: /Users/you/Library/Logs/DiagnosticReports/DDPSample-2026-10-29-155054.ips
```

### gestures

`swipe`, `drag`, `double-tap` and `long-press` move one real finger with `input.touch`, the same finger `tap` uses, so gesture recognizers, SwiftUI and Compose see a touch. A place is an element (its center, read just before) or `x,y` in the frames' unit. `swipe` moves in 12 steps over `--for`; `drag` holds 500 ms first so the app sees a long press that picks the item up; `double-tap` leaves 100 ms between taps. Each prints what changed, leaving out what changes on its own (a clock). A swipe can leave a list moving: wait (`ddp wait --settled 300ms`) before you touch something in it.

Without the library, `--device <adb serial>` uses Android's own `input` command with `x,y` in screen pixels (the screenshot's), and cannot say what changed. A double tap needs the library: two `adb input` commands come too far apart. iOS has no public way to touch a simulator from the host, so ddp refuses there and points to agent-device.

### device actions

`set location LAT,LON` moves the device: `simctl location` on iOS, the emulator console's `geo fix` on Android (an emulator only). `set orientation portrait|landscape` turns Android's auto-rotate off and sets the rotation; the simulator has no public way to rotate from the host, so ddp refuses there. For a time zone on iOS, launch with `ddp launch --env TZ=America/Bogota`.

`biometric enroll|match|fail` does what the simulator's and emulator's own menus do: Face ID enrollment and a matching or failing face on iOS, a finger touch on the emulator (`match` touches finger 1, the one enrolled first in Settings; `fail` touches one not enrolled). Android enrolls only through Settings. Neither platform confirms, so ddp says it sent it: check what the app shows next.

`push payload.json` delivers a push to the app on the simulator as APNs would (`simctl push`). iOS refuses it while the app has no notification permission (`not_authorized`); a `.provisional` request is granted without a dialog. Android has no local way to deliver an FCM push, so ddp refuses and points to running the app's handler as an app command. `job <id>` runs one of the app's scheduled jobs now on Android (`cmd jobscheduler run -f`, WorkManager included); iOS runs a background task early only from the debugger, so ddp refuses and gives the debugger line.

`back` is Android's Back key, or on iOS the edge swipe UIKit turns into back (a real finger from the screen's very edge; a screen with no back gesture does not move). `background --for 2s` leaves the app the way a person does (Home on Android, another app on iOS) and brings it back; the libraries write `background` and `foreground` into History from the platform's own notifications, so `ddp history` shows what the app did meanwhile.

### memory

`memory` reads the app's process with each platform's own tools. iOS: a simulator app is a macOS process, so `footprint` gives its footprint and biggest categories, `memory leaks` runs `leaks` (exit 1 when it finds one, with the first few), `memory heap` runs `heap` and lists the classes by bytes. Android: `dumpsys meminfo` gives the App Summary (Java heap, native heap, graphics, total PSS); `memory heap` runs `am dumpheap` and saves the `.hprof` in `.ddp/` for Android Studio. Android has no leak detector of its own, so `memory leaks` points to LeakCanary.

### profile

`profile --for 3s` samples the app with the platform's own sampler and prints the top functions by their own samples, with their library: `sample` on the simulator process, `simpleperf record --app` on Android (Android 10 and later, a debuggable build). On iOS, threads asleep in a system call are idle time: they are counted on one line, not listed. A measurement: it exits 0.

### audits

`audits` prints the runtime warnings the platform itself wrote in the app's log in the last `--since` (default 5m), as `WARNING <the line>`, and exits 1 when there is one. They are the platform's words, so nothing is guessed. iOS: SwiftUI's "Publishing changes from within view updates" and "Modifying state during view update", AttributeGraph cycles, Auto Layout's "Unable to simultaneously satisfy constraints" (iOS 27 writes it only when the app runs with `-_UIConstraintBasedLayoutLogUnsatisfiable YES`, a scheme argument or `xcrun simctl launch <udid> <bundle> -_UIConstraintBasedLayoutLogUnsatisfiable YES`), the Main Thread Checker (when the app runs with it), a collection mutated while enumerated. Android: StrictMode violations (when the app turns StrictMode on; `penaltyLog` writes them), Choreographer's "Skipped N frames", a leaked window, a resource not closed.

### launch

`launch` stops the app if it runs, starts it again and connects, so the next command runs live on the new run. `--env KEY=VALUE` (repeatable) puts the app in a state at start, such as a backend URL or a demo mode. iOS gets each value as an environment variable (`ProcessInfo.processInfo.environment["KEY"]`, sent as `SIMCTL_CHILD_KEY`). Android has no per-launch environment for an app, so each value is a string extra of the launch intent (`intent.getStringExtra("KEY")`). On Android it starts the launcher activity the package manager resolves (`am start -S -W -n`), not `monkey`, which does not start every app. `launch` then asks the app which names it received (`app.launch`, the platform's answer) and prints `app received KEY`, or a warning for a name that did not arrive. Read them in the app with the platform's own API. To run app commands after it, chain them: `ddp launch $UDID --app ID --env MODE=demo && ddp run like`.

### doctor and engine hooks

Some answers need platform pieces that are not public API: SwiftUI's hit test and responders, UIKit's touch radius and touch events on iOS; hidden View members and Compose internals on Android. Each one is a hook the library counts in every run (`app.hooks`). `ddp doctor` runs the hit test once, then prints every hook: what it is (`private` or `hidden`: an OS member; `internal`: a framework's internals), how often it answered, failed, or let the platform's documented rule answer instead (`fallback`), and what depends on it. A failing hook fails doctor: the checks it feeds say nothing until it works again. [hooks.md](hooks.md) lists them with the versions they were checked on.

```
ok   hook swiftui.responders (internal): answered 7 times
ok   hook viewGroup.touchOrder (hidden): the platform's rule answers instead, 508 times (blocked; drawing order, then Z)
```

Every check, `check tappable` and `explore` also print one line per hook that failed in this app run, and a take keeps them in its `inspect` line. It is not a problem in the app and does not change the exit code: it says which answers ddp could not get. Tell the user, with the OS version, and run `ddp doctor`.

```
hook swiftui.responders (internal) failing: no AGGraphGetWeakValue or attachmentsStorage; not judged: whose gesture takes a touch (AGGraphGetWeakValue) (ddp doctor)
```

### check jank, settle

`check jank` reports refreshes the app missed, as the libraries count them from the platform's own frame times (`CADisplayLink` on iOS, Choreographer and the display's refresh rate on Android), with their own capture time left out: a refresh counts only when the app's own work took longer than it. History takes are not judged. `check settle` reports elements still changing in the last 500 ms (`NOT SETTLED`: a loop, or a take that ended too early) and positions that shake by 1 pt or less (`JITTER`). It needs a take of 1 s or more.

A simulator booted with `xcrun simctl boot` and no Simulator.app window still draws frames, so `read`, `check jank` and the other live commands work headless. Measured on the iOS 27.0 simulator with the sample app, whose clock is a SwiftUI `TimelineView(.animation)`: 121 frames in 2 s, the clock moved every 16.7 ms, and `check jank` said `no jank: 121 frames, no refresh missed`. The frames are read by the library, so the app must have it.

### check all, baseline

`check all` runs overlap, corners, jank, settle and a11y on one take, prints only problems and a summary, and exits 1 on any. Live, it records first, then asks the app for its answers (see above). `baseline save <name>` stores the last take (or `--file`) in `ddp-baselines/<name>.jsonl.gz`, next to the code so it can be committed. `check baseline --against <name>` lines the take up with it on the first change and prints `CHANGED <element> from t=...` with the first differences; `--junit <file>` also writes a JUnit report for CI.

### compare

`compare` measures how far two images are apart and writes them side by side with a third panel, red where they differ. It is a measurement, not a check: it reports no problem and exits 0. The agent decides what the numbers mean. Only a bar the caller sets changes the exit code (see `--max`).

- `compare --reference ref.png` takes the device's own screenshot (status bar included), scales the reference to the screen's width, puts the top edges together and compares the height both have. It works without a session when given `--device`, so the app does not need the library. `--screen saved.png` takes a saved screenshot instead of the device's, so both phones can be captured first and compared later with the devices off (`--ignore` needs the live screen there; use `--mask`).
- `compare <element> --reference card.png` and `compare <element> --on ios,android` take the element's pixels from the app's own windows (`screen.pixels`), and scale both images to the smaller width and height. `--on` takes two named sessions, as in `record --on`.
- The reference is a PNG, or on macOS any image `sips` opens (JPEG, WebP, HEIC).
- `--ignore <element>...` leaves elements out of the scores, such as a clock or a spinner: on the screen it needs a session to find them, and across two phones each phone's place for it is left out. `--mask x,y,w,h` (whole screen only, repeatable) leaves out a rectangle in screenshot pixels, such as the status bar, whose time never matches a reference. Left-out pixels are dark gray in the picture and count in no score.
- `difference` is the mean of each pixel's largest channel difference. `pixels that differ` is the share more than 32 of 255 apart, which is the same tolerance the pixel checks use for "the same color". Then the same two numbers per band, top to bottom: 8 bands for the screen, 4 for an element. Resampling a reference to another width moves edges by a pixel, so text and borders show as small red lines even when nothing changed.
- `compare --on ios,android --structure` compares what the two phones draw by label, not pixels, so platform idiom (a large title, an inset list, a FAB) does not hide a real gap. It prints the labels only one phone has (a missing tab, search field or chip), and a label drawn as another kind on each (`Home (ios button, android tab)`). Labels match case and spaces aside; a label twice on one phone and once on the other leaves one over. Both sessions need the library: iOS gives the host no accessibility tree.
- `--max 5%` and `--max-band 20%` set a pass bar on `pixels that differ`, overall and in any one band. Over it, ddp prints `over --max: ...` or `over --max-band: band ...` and exits 1; under it, `within the bar` and exits 0. The bar is the caller's, so a run over many screens can count passes.

```
$ ddp compare --reference player-mobbin.png
screen 1206x2622 against player-mobbin.png 1180x2565 (scaled 1.022)
difference 0.3% (pixels that differ: 0.7%)
by band, top to bottom (328 px each):
      0-328     0.1%  differ 0.3%
   ...
   1312-1640    0.7%  differ 1.4%
   ...
ddp-compare.png: reference | screen | difference (red: differs)
```

### check parity

`check parity` compares two recordings of the same flow, element by element. It prints `DIFF <element>` with what differs: timing of the change window (over 50 ms), peak alpha (over 0.1), corner and shadow radius in pt (over 1 pt), shadow alpha. Corner is the outer edge of what is drawn: SwiftUI `.stroke` draws half its width outside the shape, Compose `border` draws inside.

Example:

```
$ ddp connect $UDID --app dev.agentddp.sample
connected to dev.agentddp.sample on ios ... (port 59405): 10 elements, 23 nodes on screen
$ ddp watch flashBox --until 'border>3' --then "agent-device press id=animatedButton"
t=0ms present True; x 98.67; y 342.33; w 60.33; h 40.33; alpha 1; fill None; border 0; ...
t=700ms ... border 0 -> 0.04; borderColor None -> #FF383CFF
...
until met at t=950ms
$ ddp check overlap
COVERED coveredLabel: the screen shows #8E8E93FF where its text has ink and around it, so the text cannot be seen in [199, 352.33, 104.33, 20.33]
```

## Elements and names

An element is what you would tap: an accessibility element on iOS, a semantics node with a test tag, text or content description on Android, or a View with text, content description or an app resource id. Its name is the accessibility id / test tag, or its label when it has none (so an untagged text that changes label shows up as two elements). Tag what you want to watch.

On Android a clickable that merges its children (Button, FilterChip, `Modifier.clickable`) is one element with its children's text; untagged children inside it are not elements.

A drawn node belongs to the element whose subtree drew it when the recorder knows (Android), otherwise to the smallest element whose frame holds it (4 pt tolerance). Ownership only names things in the output; no check decides a problem from it.

## Recording format (JSON lines)

```
{"type":"header","protocol":1,"platform":"ios","unit":"pt","scale":3,"frames":240}
{"type":"frame","i":0,"t":0,"elements":[{"id":"flashBox","label":"Flash","frame":[x,y,w,h]}],
 "nodes":[{"z":5,"kind":"layer","frame":[x,y,w,h],"visible":[x,y,w,h],"fill":"#RRGGBBAA",
           "border":{"width":2,"color":"#RRGGBBAA"},"corner":12,"opacity":0.5,"clips":true,"source":"ActionBarContainer"}]}
{"type":"end","frames":240,"averageCaptureMs":2.07,"slowestCaptureMs":4.27}
{"type":"inspect","minimum":44,"hit":[...],"frame":{...},"texts":[...],"corners":[...]}
```

- Coordinates are window coordinates: points on iOS, pixels on Android (`scale` converts).
- `nodes` are in paint order, `z` 0 first; a higher `z` paints over a lower one. Only nodes that draw something are listed.
- `kind`: `layer` (fill and/or border), `shape` (custom SwiftUI `Path` shapes), `gradient` (`gradient` holds the colors), `bitmap` (text, images, drawables), `opaque-drawing` (SwiftUI `Canvas` or `drawingGroup`: the shapes inside are gone, so `pixels` reads what it shows: `colors` (top 3 with their `share` of painted pixels), `peakAlpha`, `coverage` of its box), `draw` (Android: one shape a Compose `Canvas`, `drawBehind`, `drawWithContent` or `drawWithCache` drew this frame, with `op` such as `circle`, `rect`, `path`, `line` or `text` (with its `text` where the platform's call carries the string; Android 10 and up draw text through `MeasuredText`, which gives its bounds only), and its fill or stroke; color alpha shows fades). Tag the Canvas with `testTag` so it is reached. `read` reports `draws`, the number of visible draw ops an element owns, so a ring that flashes shows as `draws: 1 -> 1 ... max 2 TRANSIENT`.
- `corner` is the largest corner radius; `corners` (`[top-left, top-right, bottom-right, bottom-left]`) when they differ (iOS `maskedCorners`, Android per-corner radii).
- `effects` when the node is under something that changes its pixels on screen: a filter, a compositing filter or a mask (iOS), a layer with its own paint, a render effect, a blend mode or a blur (Android). Pixel checks leave it out.
- `missed` on a frame: refreshes the app missed before it, its own work alone, with the library's capture left out. The header says `"missed": true` when frames count them.
- The `inspect` line, after the frames of a take a check recorded live: the app's hit test (`hit`, with `reach` for a control a touch reaches over less than `minimum`), the frame it was taken on, and per text and per corner place what the app's pixels show (`texts`, `corners`). The checks read only this line for overlap, corners, touch size and contrast.
- `points` (Android `draw` lines): `[x0, y0, x1, y1]` in window coordinates. `clipRound` (Android): `[x, y, w, h, radius]` of the rounded clip around the node.
- `shadow` (iOS): `{"radius", "opacity", "color"}` of a layer shadow, which is how SwiftUI draws `.shadow` and most glows.
- Optional fields appear only when they apply: `visible` when clipped, `opacity` when under 1, `clips` when the node clips its children, `source` when the native view class is known.
- `text` on a text node: `{"size", "color", "weight"}`, size in the recording's unit; `colors` when it draws in more than one (attributed text, spans, links); `unknownColor` when another color paints over it that the recorder cannot read (Compose `BasicText` with a color producer). Android reads it from the laid-out Compose style or the TextView; iOS from UILabel / UITextField, and for SwiftUI `Text` from its pixels every frame, sampled at 24 px a side (`"estimated": true`, size from the line height).
- `own` (Android): the index in `elements` of the element whose subtree drew the node, `-1` for none. The CLI uses it for ownership instead of geometry, so a plain box drawn over a label counts as covering it.
- Elements: `role`, the ARIA role a browser would give it (`button`, `link`, `textbox`, `searchbox`, `checkbox`, `radio`, `switch`, `slider`, `tab`, `combobox`, `heading`, `image`, `StaticText`, `generic`), from iOS accessibility traits or the Android Compose role, actions and View class. `tap` when the element is a control (iOS button, link or adjustable trait; Android click action), and `touch` (Android) when its touch area is bigger than what it draws, such as a Material chip padded to 48 dp. `disabled` when the platform says so (iOS `notEnabled` trait, Android `Disabled` semantics or a disabled View).
- The header carries `startedAt` (ms since 1970) so `ddp logs` can line log lines up with the take.
- Android walks every Compose layout node in draw order, so nodes without semantics and `zIndex` count, and every window of the app, so menus, dialogs and popups are read. iOS sorts sublayers by `zPosition`. Android Views follow a custom child drawing order (API 29+). A `GradientDrawable` stroke and corner radius are read on Android.

## What it does not see yet

- Physical iOS devices: the CLI uses `simctl`.
- `history`, Android: it is woken by the activity window only, so a dialog that animates while the screen under it is still shows up at the next draw of the screen. iOS: animations Core Animation runs by itself (`UIView.animate`) do not wake the app, so only their start and end are kept; SwiftUI and display-link animations are kept frame by frame.
- The single shapes inside a SwiftUI `Canvas` or `drawingGroup`: one bitmap, read by its pixels.
- System UI outside the app: iOS alerts like "Open in ...?", Android system dialogs. Pixel checks read only the app's own windows. The keyboard is placed from the platform's own answer (UIKit's keyboard notifications; Android's IME insets, API 30 and up): a control all under it is `COVERED by the keyboard` in `tap` and `check tappable`. A control partly under it, or any on Android before API 30, still counts as reaching.
- Text that is only partly covered; a corner bleed in the middle of an animation (pixel checks look at the end of the take); Android popups' pixels before API 34.
- A Compose `drawWithCache` that records into a graphics layer (`obtainGraphicsLayer`): its draws are not read, and the `compose.drawCache` hook says why.
- A parent that takes a touch only at its release or after a move, and on iOS any gesture that wins over the control once the touch is under way (a parent's `UITapGestureRecognizer`, a SwiftUI `highPriorityGesture`): UIKit decides that only during a real touch, so `screen.hit` says the touch reaches the control and `tap` shows it as `nothing changed`. Android: a View on top whose own code decides whether it takes a touch (a touch listener, a TouchDelegate, its own `onTouchEvent`, such as most Material widgets) is unknown and not reported.

## Adding it to an app (Debug only)

- The CLI: `npm i -g agent-ddp` (one native binary, no runtime), or `cargo install --git https://github.com/bandada-labs/agent-ddp agent-ddp`.
- iOS: add the Swift package `https://github.com/bandada-labs/agent-ddp` (or the local folder `ios/AgentDDP`; no dependencies) and call `AgentDDP.start()` at launch. Outside Debug every call is empty. Simulator only for now (the CLI uses `simctl`).
- Android: `debugImplementation("com.github.bandada-labs:agent-ddp:<tag>")` with `maven("https://jitpack.io")` in the repositories, or `debugImplementation(project(":agent-ddp"))` pointing at `android/agent-ddp`. It starts by itself through a ContentProvider; no app code. Needs a debuggable build. In Debug it installs its own Compose window recomposer factory (for `ddp time`); an app that sets its own factory replaces it.

Opt-ins from the app's Debug composition root (the app's own ports stay as they are):

- Network, iOS: wrap the app's network port in a decorator that sends through `AgentDDP.send(request) { try await session.data(for: $0, delegate: AgentDDP.timing) }`, and call `AgentDDP.redact(values: [devSecret])` once. `send` logs the request and lets a client pause, answer or fail it (`net.intercept`). A decorator that only calls `AgentDDP.logRequest(...)` when a request ends still logs, without pausing. `AgentDDP.timing` hands the request's URLSessionTaskMetrics to `net get` (`timing`); without it there is no timing. See `ObservedSession` in `samples/ios`.
- Network, Android: add `AgentDdpInterceptor()` to the Debug OkHttp client with `addInterceptor` (not `addNetworkInterceptor`, so a client can answer without sending) and call `AgentDdpNetwork.redact(devSecret)`; other clients call `AgentDdpNetwork.log(...)`, without pausing. `eventListenerFactory(AgentDdpNetwork.timing)` on the same client gives `net get` OkHttp's phases (`timing`); a client with its own event listener keeps it and gets no timing. See `samples/android/app/src/debug`.
- Commands: add named actions for agents from Debug code, `AgentDDP.command("login") { params in ... }` (iOS, async) or `AgentDdpCommands.add("login") { params -> ... }` (Android, on a background thread). Return any JSON value. See `addSampleCommands` in `samples/android` and `DDPSampleApp` in `samples/ios`.
- Time: let the app's clock port read `AgentDDP.elapsedTime()` (iOS) or `AgentDdpTime.elapsedMillis()` (Android) in Debug, so `ddp time` can pause and rewind the app's own timelines.

The library keeps recordings in `Library/Caches/agent-ddp/` (iOS) or `files/agent-ddp/` (Android) inside the app's own sandbox. On iOS it turns on the private accessibility automation switch that XCTest uses, so SwiftUI builds its accessibility tree.

Checks for the CLI: `cargo test --manifest-path cli/Cargo.toml`, then `python3 cli/test_ddp.py` runs the built binary from the outside.
