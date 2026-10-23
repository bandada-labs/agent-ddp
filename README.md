# Agent-DDP

DDP (Device DevTools Protocol) gives a coding agent what the Chrome DevTools Protocol gives a browser agent, but for native iOS and Android apps. The agent connects to a running Debug build and asks what is on screen, what each element drew, and how it moved frame by frame.

It works in the style of CDP. A small library lives inside the Debug build and opens a local endpoint. The `ddp` CLI connects to it and runs queries, watches values while an animation plays, reads network requests and pauses or steps the app's time. Release builds contain none of it.

```
ddp connect <simulator UDID | adb serial> --app <bundle or application id>
ddp tree                                   what is on screen now, per element
ddp watch loginButton --until 'border>3'   values frame by frame until a condition
ddp time pause | step 10 | resume          stop an animation and move it frame by frame
ddp check overlap                          elements covered by something else
ddp net list                               requests the app made, secrets redacted
```

Per element it reports the frame, fill, border, corner radius, opacity, clipping and what covers it. It reads SwiftUI and UIKit layers on iOS, and Compose and Views on Android. Recordings are JSON lines, so an agent can check an animation without watching a video.

## Why native

A lot of teams choose React Native, Flutter or another layer on top of the platform because one codebase is cheaper, and because tooling for agents has been much better on the web. DDP tries to close the second gap. An agent can now see and check a SwiftUI or Compose screen in the same way it checks a web page, with the platform's own layers and nothing in between.

So if the extra layer is there mostly for the tooling, it may not be needed anymore. Native apps get the platform's own animations, accessibility and performance, and with DDP the agent can still review them.

## Getting started

- iOS: add the Swift package `ios/AgentDDP` (one folder, no dependencies) and call `AgentDDP.start()` at launch. Simulator only for now.
- Android: add `debugImplementation(project(":agent-ddp"))` pointing at `android/agent-ddp`. It starts by itself, no app code.
- CLI: `cli/ddp` is one Python 3 file with no dependencies. It needs `xcrun simctl` for iOS and `adb` for Android.

`samples/ios` and `samples/android` are small apps with the library already wired in.

## Docs

- `ddp skills get core`: the guide for agents, shipped with the CLI ([docs/skills](docs/skills)). [skills/ddp/SKILL.md](skills/ddp/SKILL.md) is a thin skill file that only points there.
- [docs/agents.md](docs/agents.md): every command, its output and the recording format.
- `ddp skills get reviewer`: how an agent reviews an animation with ddp, step by step.
- [SURVEY.md](SURVEY.md): what existing tools could and could not see before DDP.

## Limits

Debug builds only. iOS works on the simulator, not on devices yet. Not seen yet: the single shapes inside a SwiftUI `Canvas` (read by their pixels instead), and system UI outside the app such as iOS alerts.

## License

MIT, see [LICENSE](LICENSE).
