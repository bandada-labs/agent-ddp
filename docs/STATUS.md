# Status

Last update: 2026-10-23. Read this after a break, then [DECISIONS.md](DECISIONS.md).

## In one paragraph

A small library inside a Debug build of an iOS or Android app serves what the app draws, frame by frame (ADR 0001). The `ddp` CLI, one Rust binary on npm (ADR 0002, 0003), reads it live or from recordings and prints short text for coding agents (ADR 0006). The protocol between them is versioned (ADR 0004). A flight recorder keeps the last 15 minutes on both platforms (ADR 0005).

## Released

0.1.5 on 2026-10-23: npm `agent-ddp`, GitHub release with 5 binaries, JitPack `com.github.bandada-labs:agent-ddp:v0.1.5`, SwiftPM from the repo URL. What is in it: [CHANGELOG.md](../CHANGELOG.md).

## Next

1. The control endpoint in the library (ADR 0008, which replaced the CDP plan of ADR 0007). Order: transport, reading, acting, scripts in the app, frames and history over it, then the CLI on top, then v1 goes. Done: transport, `screen.*`, `net.*` with pausing, `log.watch`, `input.*`, `prefs.*`, on both platforms. Next in acting: the device settings an agent needs to test with (appearance, text size, locale), then scripts.
2. A code of conduct, once the maintainer gives a contact address.
3. iOS on physical devices (today the CLI uses `simctl`).
4. History on iOS for `UIView.animate` animations, if users need more than their start and end (ADR 0005).

## Known limits

Listed in [agents.md, "What it does not see yet"](agents.md#what-it-does-not-see-yet).
