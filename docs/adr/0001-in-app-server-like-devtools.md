# 0001 A server inside the Debug build and a CLI client, like Chrome DevTools; no daemon

Status: accepted, 2026-10-23

## Decision

A small library runs inside the Debug build of the app and serves what the app draws on a local endpoint: TCP on 127.0.0.1 with a token on the iOS simulator, an abstract unix socket that only adb and root may open on Android. The `ddp` CLI is a client: each command is a short process that opens one connection, sends one command and reads JSON lines back. There is no daemon on the host.

## Why

- Only code inside the process can read what SwiftUI, UIKit, Compose and Views actually drew: layer properties, presentation values during an animation, Compose draw operations, pixels of a Canvas. The accessibility tree that outside tools use has no colors, borders or corner radii (SURVEY.md).
- The app is already the long-running process. A host daemon would duplicate its state and add a second thing to start, update and kill.
- A short process per command fits how agents call tools, and each command's output is complete on its own.

## Patterns

Client-server with a facade on each side (`AgentDDP` / `AgentDdpStarter` in the app, the CLI commands on the host). A recording is the stream saved to a file, so the same readers work live and offline (pipes and filters).

## Consequences

- Release builds must contain none of it: `#if DEBUG` on iOS, `debugImplementation` on Android.
- The library pays for captures on the main thread, so capture cost is measured and reported in every `end` line.
- Physical iOS devices need a different transport than the simulator's localhost; not done yet.
