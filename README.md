<p align="center">
  <img alt="agent-ddp: see and check native iOS and Android UI from your coding agent, frame by frame" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/banner.svg" width="100%">
</p>

<p align="center">
  <strong>Your coding agent uses a native iOS or Android app like a person, and is told what changed after every touch.</strong>
</p>

<p align="center">
  <a href="https://github.com/bandada-labs"><img alt="Bandada Labs" src="https://img.shields.io/badge/BANDADA-LABS-F5C22E.svg?style=for-the-badge&labelColor=26231C" height="28"></a>
  <a href="https://www.npmjs.com/package/agent-ddp"><img alt="npm: agent-ddp" src="https://img.shields.io/badge/npm-agent--ddp-F5C22E.svg?style=for-the-badge&labelColor=26231C" height="28"></a>
  <a href="https://github.com/bandada-labs/agent-ddp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-F4F4F2.svg?style=for-the-badge&labelColor=26231C" height="28"></a>
  <img alt="Platforms: iOS and Android" src="https://img.shields.io/badge/iOS%20%C2%B7%20Android-native-F4F4F2.svg?style=for-the-badge&labelColor=26231C" height="28">
  <a href="https://github.com/bandada-labs/agent-ddp/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/bandada-labs/agent-ddp/actions/workflows/ci.yml/badge.svg?branch=main" height="28"></a>
</p>

<p align="center">
  <img alt="Recorded on an Android emulator: the agent walks a task (tap, long-press, tap Rename), each command prints what changed, then the walk replays by itself with no model, 4 steps passed" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/walk.gif" width="100%">
  <br>
  <em>Real recording, Android emulator: walk a task once, replay it with no model.</em>
</p>

## Give this to your agent

Paste it in your agent's chat, inside your app's repo. It learns the rest by itself.

```
Set up agent-ddp so you can use and check this app's UI: https://github.com/bandada-labs/agent-ddp

1. Install the CLI: npm i -g agent-ddp
2. Run `ddp skills get core` and follow it. Start with `ddp doctor`. With nothing in the app you can
   already drive it on a simulator, an emulator or an Android phone.
3. To see what each element drew and run the checks, add the library to the Debug build only:
   - iOS: add the Swift package https://github.com/bandada-labs/agent-ddp (from 0.6.1) and call
     AgentDDP.start() at launch. It does nothing in Release. Simulator only for now. Xcode 16.4 or later.
     Without Xcode or SwiftPM: ios/build-swiftc.sh builds a static library and module for plain swiftc.
   - Android: add maven("https://jitpack.io") to the repositories and
     debugImplementation("com.github.bandada-labs:agent-ddp:v0.6.1"). It starts by itself.
```

## What it does

<table width="100%">
  <tr>
    <td width="50%"><img alt="ddp taps through iOS Settings; each tap prints the elements that appeared, went away or changed" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/card-drive.svg" width="100%"></td>
    <td width="50%"><img alt="A walk recorded on iOS replays on iOS and on an Android emulator, and walk parity reports 0 steps differ" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/card-walk.svg" width="100%"></td>
  </tr>
  <tr>
    <td width="50%"><img alt="ddp read prints an animation as per-element tracks with sparklines; ddp tree prints the layers of a button" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/card-read.svg" width="100%"></td>
    <td width="50%"><img alt="ddp check all reports a covered label, a small target and low contrast text, and exits 1" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/card-check.svg" width="100%"></td>
  </tr>
</table>

<p align="center">
  <img alt="An agent records a tap on an iOS simulator with ddp read, gets each element's changes with sparklines, then runs ddp check all, which flags a covered label, a small target and low contrast text" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/demo.gif" width="100%">
  <br>
  <em>With the Debug library, iOS simulator: one tap recorded frame by frame, then <code>ddp check all</code>.</em>
</p>

<p align="center">
  <img alt="Without the library, on any build: tap, type, swipe, scroll, back, the accessibility tree with refs, walks with replay and iOS vs Android parity. The Debug library adds fill, border, corners, opacity and what covers what, frame by frame recording with sparklines, checks for covered text, jank, settling and accessibility, and the last 15 minutes kept in ddp history" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/matrix.svg" width="100%">
</p>

## Quick start

```bash
npm i -g agent-ddp
ddp up --platform android --install app-debug.apk --app com.example.myapp
# iOS: ddp up --platform ios --install MyApp.app --app com.example.myapp
ddp tree
```

## Then ask

<p align="center">
  <img alt="Prompts to give your agent: sign up as a new user with ddp walk and replay it on Android; build the onboarding screen and fix what ddp check all finds; make the like animation 250 ms ease out; find a flicker in ddp history" src="https://raw.githubusercontent.com/bandada-labs/agent-ddp/main/docs/assets/prompts.svg" width="100%">
</p>

## More

[FAQ, MCP and limits](https://github.com/bandada-labs/agent-ddp/blob/main/docs/faq.md) • [Every command](https://github.com/bandada-labs/agent-ddp/blob/main/docs/agents.md) • [Protocol](https://github.com/bandada-labs/agent-ddp/blob/main/docs/protocol.md) • [Architecture](https://github.com/bandada-labs/agent-ddp/blob/main/ARCHITECTURE.md) • [Status and decisions](https://github.com/bandada-labs/agent-ddp/blob/main/docs/README.md) • [Changelog](https://github.com/bandada-labs/agent-ddp/blob/main/CHANGELOG.md) • [Contributing](https://github.com/bandada-labs/agent-ddp/blob/main/CONTRIBUTING.md)

MIT licensed. Works with Claude Code, Codex, Cursor and any agent that runs a CLI or speaks MCP.
