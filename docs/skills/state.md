---
name: state
description: Show ddp the app's own values (a total, a mode, a value of the app's engine), then read, watch and set them.
---

# The app's own values

ddp sees what the app draws. What the app knows (a cart total, the logged-in user, a value its engine computed) it
sees only when the app shows it. You add one line per value in the app's Debug code, in the app's own language, then
read it with `ddp state`. Nothing is shared between the platforms: write the Swift line in the iOS app and the
Kotlin line in the Android app.

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
