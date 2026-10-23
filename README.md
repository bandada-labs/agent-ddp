# Agent-DDP

DDP (Device DevTools Protocol) gives a coding agent what the Chrome DevTools Protocol gives a browser agent, but for native iOS and Android apps. The agent connects to a running Debug build and asks what is on screen, what each element drew, and how it moved frame by frame.

It works in the style of CDP. A small library inside the Debug build opens a local endpoint and the `ddp` CLI talks to it. Release builds contain none of it.

## What it does today

- Reads the screen per element: frame, fill, border, corner radius, opacity, clipping, text style and what covers it. SwiftUI and UIKit on iOS, Compose and Views on Android.
- Records takes frame by frame as JSON lines, and summarizes each element's tracks as sparklines and easing curves, so an agent checks an animation without a video.
- Watches a value until a condition, waits for an element or for the screen to settle, and pauses or steps the app's time.
- Checks: overlap, corners past their radius, jank, animations that never settle, accessibility (touch targets, labels, contrast), an expected timeline, iOS vs Android parity and a saved baseline. Each check exits 1 on a problem, so it works in CI.
- Taps, deep links, appearance, text size, reduce motion and locale. Logs and network requests from the take, secrets redacted.
- `explore` taps every control on a screen and runs every check after each tap.
- Output made for agents: element refs (`@e3`), small trees, a size cap, batches and an MCP server.

## Give this to your agent

Nobody runs this by hand. Paste this in your agent's chat inside your app's repo:

```
Set up agent-ddp so you can see and check this app's UI: https://github.com/sebospc/agent-ddp

1. Clone it next to this repo and run `python3 agent-ddp/cli/build.py`.
   Copy agent-ddp/dist/ddp into this repo (for example tools/ddp) and use it as `ddp`.
2. Add the library to the Debug build only:
   - iOS: add the Swift package agent-ddp/ios/AgentDDP and call AgentDDP.start() at launch.
     It does nothing in Release. Simulator only for now.
   - Android: include agent-ddp/android/agent-ddp as the module ":agent-ddp" in
     settings.gradle and add debugImplementation(project(":agent-ddp")). It starts by itself.
3. Run `ddp skills get core` and follow it. Start with `ddp doctor`.
```

The CLI needs Python 3 and nothing else, plus `xcrun simctl` for iOS and `adb` for Android. `samples/ios` and `samples/android` are small apps with the library already wired in.

## Why native

A lot of teams choose React Native, Flutter or another layer on top of the platform because one codebase is cheaper, and because tooling for agents has been much better on the web. DDP tries to close the second gap. An agent can now see and check a SwiftUI or Compose screen in the same way it checks a web page, with the platform's own layers and nothing in between.

So if the extra layer is there mostly for the tooling, it may not be needed anymore. Native apps get the platform's own animations, accessibility and performance, and with DDP the agent can still review them.

## Docs

- `ddp skills get core`: the guide for agents, shipped with the CLI ([docs/skills](docs/skills)). [skills/ddp/SKILL.md](skills/ddp/SKILL.md) is a thin skill file that only points there.
- [docs/agents.md](docs/agents.md): every command, its output and the recording format.
- [SURVEY.md](SURVEY.md): what existing tools could and could not see before DDP.

## Limits

Debug builds only. iOS works on the simulator, not on devices yet. Not seen yet: the single shapes inside a SwiftUI `Canvas` (read by their pixels instead), and system UI outside the app such as iOS alerts.

## License

MIT, see [LICENSE](LICENSE).
