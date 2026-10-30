<p align="center">
  <img alt="agent-ddp: see and check native iOS and Android UI from your coding agent, frame by frame" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/banner.svg" width="100%">
</p>

<p align="center">
  <a href="https://github.com/bandada-labs"><img alt="Bandada Labs" src="https://img.shields.io/badge/BANDADA-LABS-F5C22E.svg?style=for-the-badge&labelColor=26231C" height="28"></a>
  <a href="https://www.npmjs.com/package/agent-ddp"><img alt="npm version: agent-ddp" src="https://img.shields.io/npm/v/agent-ddp.svg?style=for-the-badge&labelColor=26231C&color=F5C22E" height="28"></a>
  <a href="https://github.com/bandada-labs/agent-ddp/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/bandada-labs/agent-ddp/ci.yml?branch=main&style=for-the-badge&labelColor=26231C&label=CI" height="28"></a>
  <a href="https://github.com/bandada-labs/agent-ddp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/github/license/bandada-labs/agent-ddp.svg?style=for-the-badge&labelColor=26231C&color=F4F4F2" height="28"></a>
  <img alt="Platforms: iOS and Android" src="https://img.shields.io/badge/iOS%20%C2%B7%20Android-native-F4F4F2.svg?style=for-the-badge&labelColor=26231C" height="28">
</p>

# agent-ddp

See and check native iOS and Android UI from a coding agent, frame by frame.

DDP (Device DevTools Protocol) gives a coding agent what the Chrome DevTools Protocol gives a browser agent, but for native apps. A small library inside the Debug build opens a local endpoint, and the `ddp` CLI talks to it. The agent asks what is on screen, what each element drew and how it moved, and gets short text back instead of a screenshot to guess from. Release builds contain none of it.

Works with Claude Code, Codex, Cursor and any agent that can run a CLI or connect over MCP. SwiftUI and UIKit on iOS, Compose and Views on Android.

<p align="center">
  <img alt="An agent records a tap on an iOS simulator with ddp read, gets each element's changes with sparklines, then runs ddp check all, which flags a covered label, a small target and low contrast text" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/demo.gif" width="100%">
</p>

## Quick start

```bash
npm i -g agent-ddp
ddp doctor
```

`doctor` checks the simulator or emulator, the app and the connection, and prints the fix for whatever is missing. The CLI is one native binary for macOS, Linux and Windows. It needs `xcrun simctl` for iOS and `adb` for Android.

### Give this to your agent

Paste this in your agent's chat inside your app's repo. It adds the library and learns the commands by itself.

```
Set up agent-ddp so you can see and check this app's UI: https://github.com/bandada-labs/agent-ddp

1. Install the CLI: npm i -g agent-ddp
2. Add the library to the Debug build only:
   - iOS: add the Swift package https://github.com/bandada-labs/agent-ddp (from 0.3.0) and call
     AgentDDP.start() at launch. It does nothing in Release. Simulator only for now. Xcode 16 or later.
     Without Xcode or SwiftPM: ios/build-swiftc.sh builds a static library and module for plain swiftc.
   - Android: add maven("https://jitpack.io") to the repositories and
     debugImplementation("com.github.bandada-labs:agent-ddp:v0.3.0"). It starts by itself.
3. Run `ddp skills get core` and follow it. Start with `ddp doctor`.
```

`samples/ios` and `samples/android` are small apps with the library already wired in.

## What the agent sees

The screen as elements, each with what it drew. Refs like `@e1` work in the next command.

```
$ ddp tree
t=5050ms (frame 299 of 300), unit pt
@e1 animatedButton "Selected" [134, 122, 134.33, 48.33]
  z=1 layer fill #0088FFFF corner 12 [134, 122, 134.33, 48.33]
  z=2 bitmap [166, 136, 70.33, 20.33]
  z=3 layer border 6 #FF8D28FF corner 15 [131, 119, 140.33, 54.33]
@e2 gradientCapsule [91, 194.33, 220, 30]
  z=4 layer corner 15 [91, 194.33, 220, 30]
  z=5 gradient corner 15 [91, 194.33, 220, 30]
```

An animation as tracks per element, with sparklines, so the agent checks timing and easing without a video:

```
$ ddp read --for 5s --then "ddp tap animatedButton"
ios 300 frames over 5050 ms, unit pt
animatedButton: frame [134, 122, 134.33, 48.33] fill #0088FFFF border 6 kinds bitmap,layer
  fill: #FFFFFFFF -> #0088FFFF, 112 values, changing 700..2583.33ms
  border: 2 -> 6 ▁▁▂▅▇███████, changing 666.67..2566.67ms, min 2 @0ms, max 6 @2566.67ms
  opacity: 1 -> 1 █▁▇█████████, changing 521.81..1050ms, min 0.2 @521.81ms  TRANSIENT (same at start and end, different in between)
  label: Tap me -> Selected, 2 values, changing 598.82..598.82ms
```

Checks that exit 1 on a problem, so the same command works as a gate in CI:

```
$ ddp check all
COVERED coveredLabel: the screen shows #8E8E93FF where its text has ink and around it, so the text cannot be seen in [199, 352.33, 104.33, 20.33]
SMALL TARGET smallTarget: a touch reaches it over 34x43, under 44x44
LOW CONTRAST faintText: 1.6:1, #CCCCCCFF on #FFFFFFFF, needs 3:1 at least (its size is estimated)
check all over 61 frames: overlap 1, a11y 2 (ran overlap, corners, jank, settle, a11y)
```

## What it does today

- Reads the screen per element: frame, fill, border, corner radius, opacity, clipping, text style and what covers it.
- Records takes frame by frame as JSON lines, and summarizes each element's tracks as sparklines and easing curves.
- Watches a value until a condition, waits for an element or for the screen to settle, and pauses or steps the app's time.
- Checks: covered text, corners past their radius, jank, animations that never settle, accessibility (touch targets, labels, contrast), an expected timeline, iOS vs Android parity and a saved baseline. A check reports a problem only when the platform itself says so: its hit test, the pixels of the app's own windows, its frame times. What it cannot tell, it does not report.
- Taps, deep links, appearance, text size, reduce motion and locale. Logs and network requests from the take, secrets redacted.
- Keeps the last 15 minutes inside the app: every change, tap and screen. `ddp history` shows them, `ddp history at <time>` turns one moment into a take to check.
- `explore` taps every control on a screen and runs every check after each tap.
- Output made for agents: element refs (`@e3`), small trees, a size cap, batches and an MCP server.

## What to ask your agent

Once it is set up, prompts like these work:

- "Build the onboarding screen and check it on the iOS simulator and the Android emulator. Fix anything `ddp check all` finds."
- "The like button animation feels slow. Measure it and make it match the spec: 250 ms, ease out."
- "Compare the settings screen on iOS and Android and list what is different."
- "Something flickered on the home screen a few minutes ago. Find it in `ddp history`."
- "Check this screen with the largest text size and in dark mode."

## MCP

`ddp mcp` runs the same commands as MCP tools on stdio:

```json
{
  "mcpServers": {
    "ddp": {
      "command": "ddp",
      "args": ["mcp", "--tools", "all"]
    }
  }
}
```

`--tools` takes `core`, `live`, `checks` or `all`. The plain CLI is usually cheaper in tokens, so prefer it when the agent can run commands.

## How it works

The library runs inside the Debug build, reads the platform's own view and layer trees on every frame and serves them on a local endpoint. On iOS the CLI finds the port in the app's simulator container. On Android it reaches the app through `adb forward`. It asks for frames and turns them into text an agent can read in a few hundred tokens.

There is no daemon. The library in the app is the server, and every `ddp` command is a short call to it. A recording is a plain JSON lines file, so every read and check also works offline on a saved take.

## Why native

A lot of teams choose React Native, Flutter or another layer on top of the platform because one codebase is cheaper, and because tooling for agents has been much better on the web. DDP tries to close the second gap. An agent can now see and check a SwiftUI or Compose screen in the same way it checks a web page, with the platform's own layers and nothing in between.

So if the extra layer is there mostly for the tooling, it may not be needed anymore. Native apps get the platform's own animations, accessibility and performance, and with DDP the agent can still review them.

## FAQ

### How is it different from XCUITest, Espresso, Maestro or agent-device?

Those tools drive the app through the accessibility tree and see what it exposes: roles, labels and frames. DDP reads what was drawn: colors, borders, corners, opacity, what covers what, and how each value moved over the frames. They work well together. agent-device or a test runner is the hands, ddp is the eyes.

### Does it ship in my release build?

No. On iOS `AgentDDP.start()` does nothing outside Debug. On Android the library is a `debugImplementation` dependency, so Release does not include it.

### Does it work on a physical device?

iOS works on the simulator only for now. Android goes through `adb`, so it reaches what `adb devices` lists.

## Docs

- `ddp skills get core`: the guide for agents, shipped with the CLI so it always matches the version ([docs/skills](docs/skills)). [skills/ddp/SKILL.md](skills/ddp/SKILL.md) is a thin skill file that points there.
- [docs/agents.md](docs/agents.md): every command, its output and the recording format.
- [docs/protocol.md](docs/protocol.md): the wire protocol between the library and a client.
- [ARCHITECTURE.md](ARCHITECTURE.md): how the parts fit and the rules every change keeps.
- [docs/README.md](docs/README.md): status, decisions and ADRs.
- [SURVEY.md](SURVEY.md): what existing tools could and could not see before DDP.

Without npm: `cargo install --git https://github.com/bandada-labs/agent-ddp agent-ddp`.

## Limits

Debug builds only. iOS works on the simulator, not on devices yet. Not seen yet: text and images inside a SwiftUI `Canvas` (its shapes are read), and system UI outside the app such as iOS alerts.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md), and [SECURITY.md](SECURITY.md) for anything security related. Changes are listed in [CHANGELOG.md](CHANGELOG.md).

## License

MIT, see [LICENSE](LICENSE).
