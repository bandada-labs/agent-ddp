---
name: app
description: Show ddp the app's own values and events (a total, a mode, a rule that fired), then read, watch and set them.
---

# The app's own values and events

ddp sees what the app draws. What the app knows (a cart total, the logged-in user, a value its engine computed) and
what happens inside it (a rule that fired, a sync that ended) it sees only when the app tells it. You add the lines in
the app's Debug code, in the app's own language, then read them with `ddp state` and `ddp events`. Nothing is shared
between the platforms: write the Swift line in the iOS app and the Kotlin line in the Android app.

## 1. Add the values (Debug only)

iOS, where the app starts (after `AgentDDP.start()`):

```swift
AgentDDP.state("total_spent", get: { engine.value("total_spent") })
AgentDDP.state("budget", get: { engine.budget }, set: { engine.budget = $0 as? Double ?? engine.budget })
```

Android, in debug code (a `src/debug` file, or behind `BuildConfig.DEBUG`), for example in `Application.onCreate`:

```kotlin
AgentDdpState.add("total_spent", get = { engine.value("total_spent") })
AgentDdpState.add("budget", get = { engine.budget }, set = { engine.budget = (it as? Number)?.toDouble() ?: engine.budget })
```

- `get` returns a JSON value: a string, number, Bool, null, a list or a dictionary/map of those. Anything else is an
  error that names the value, so convert first (`date.timeIntervalSince1970`, `enum.name`).
- `set` is optional. Without it the value is read-only and `ddp state set` refuses it. It gets what the agent sent,
  parsed as JSON: `500` is a number, `true` a Bool, `demo` text.
- Both run on the main thread, where app state usually lives. Keep them quick: `state watch` calls `get` every 50 ms.
- Outside Debug `AgentDDP.state` does nothing on iOS; on Android the library is a `debugImplementation`, so keep the
  calls in debug code.

## 2. Check them

```bash
ddp launch <device> --app <id>        # or ddp connect, if the app runs already
ddp state                             # every value, (writable) when it has a setter
```

`no values` means the lines did not run: check they are in the build you installed and run at start.

## 3. Use them

```bash
ddp state get total_spent
ddp state set budget 500                          # answers what the getter reads back
ddp state watch total_spent --until '>100'        # each change, then exit 0; exit 1 if --for (10s) ends first
ddp state watch mode --until '=done' --for 30s
```

Put the app in a state without the UI (`state set`), then check what it draws with `ddp tree` or `ddp check all`.

## Events

Send one where it happens, from any thread:

```swift
AgentDDP.event("rule", ["event": "save:expenses", "condition": "spent > budget", "then": "notify"])
```

```kotlin
AgentDdpEvents.send("rule", mapOf("event" to "save:expenses", "condition" to "spent > budget", "then" to "notify"))
```

```bash
ddp events                         # the last ones the app kept (up to 200)
ddp events --follow --for 10s      # the kept ones, then each new one
ddp history                        # events next to frames and taps, for the last 15 minutes, even after a crash
```

Fields are JSON values. The app decides what an event is and what it carries; ddp only shows them.
