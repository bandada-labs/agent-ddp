# FAQ

## How is it different from XCUITest, Espresso, Maestro or agent-device?

Every action says what changed, and a walk replays without writing a test or calling a model. With the library ddp also reads what was drawn: colors, borders, corners, opacity, what covers what, and how each value moved over the frames. Those tools work well next to it.

## Does it call a model?

No. The agent you already use is the model. ddp records, replays and checks without tokens.

## Does it ship in my release build?

No. Driving adds nothing to the app. The library is Debug only: on iOS `AgentDDP.start()` does nothing outside Debug, and on Android it is a `debugImplementation` dependency.

## Does it work on a physical device?

iOS works on the simulator only for now. Android goes through `adb`, so it reaches emulators and the phones `adb devices` lists. A MIUI phone asks you to tap Install once per ddp version.

## How does it work?

Without the library, ddp works through the platform. On Android it installs a small instrumentation from its own binary and sends touches as the system's own input. On iOS it talks to the simulator's HID and accessibility services from the Mac, with no XCTest. Each action reads the screen before and after and prints the difference once the screen settles.

With the library, the app is the server: it reads the platform's own view and layer trees on every frame and serves them on a local endpoint. There is no daemon. A recording or a walk is a plain JSON lines file, so reads and checks also work offline. More in [ARCHITECTURE.md](../ARCHITECTURE.md).

A check reports a problem only when the platform confirms it: its hit test, the pixels of the app's own windows, its frame times. What it cannot tell, it does not report.

## Why native?

Teams often pick React Native or Flutter partly because agent tooling was better on the web. ddp closes that gap for SwiftUI and Compose: the agent uses and checks a native screen with the platform's own layers, and the app keeps the platform's animations, accessibility and performance.

## MCP

`ddp mcp` runs the same commands as MCP tools on stdio. `--tools` takes `core`, `live`, `checks` or `all`. The plain CLI is usually cheaper in tokens, so prefer it when the agent can run commands.

```json
{
  "mcpServers": {
    "ddp": { "command": "ddp", "args": ["mcp", "--tools", "all"] }
  }
}
```

## Limits

iOS works on the simulator, not on devices yet. Without the library nothing is checked (each line ends `no checks (no SDK)`), keys follow a US keyboard layout, iOS assumes portrait, and the iOS tree leaves out what the keyboard covers, as VoiceOver does. The library works in Debug builds only. Not seen yet: text and images inside a SwiftUI `Canvas` (its shapes are read), and system UI outside the app such as iOS alerts.
