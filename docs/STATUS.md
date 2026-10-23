# Status

Last update: 2026-10-23. Read this after a break, then [DECISIONS.md](DECISIONS.md).

## In one paragraph

A small library inside a Debug build of an iOS or Android app serves what the app draws, frame by frame (ADR 0001). The `ddp` CLI, one Rust binary on npm (ADR 0002, 0003), reads it live or from recordings and prints short text for coding agents (ADR 0006). The protocol between them is versioned (ADR 0004). A flight recorder keeps the last 15 minutes on both platforms (ADR 0005).

## Released

0.1.5 on 2026-10-23: npm `agent-ddp`, GitHub release with 5 binaries, JitPack `com.github.bandada-labs:agent-ddp:v0.1.5`, SwiftPM from the repo URL. What is in it: [CHANGELOG.md](../CHANGELOG.md).

## Next

1. The CDP server in the library (ADR 0007). Order: transport and discovery, read domains, act domains, `Runtime.evaluate`, the `DDP` domain, then the CLI on top. Transport, read domains, Input and Fetch are done; agent-browser `snapshot`, `screenshot` and `click` work. Storage, Overlay and Emulation come next. Until `Runtime` exists (phase 4), agent-browser drops the app when it sits idle for about 3 seconds: it checks the page with `Runtime.evaluate("location.href")` and launches its own Chrome when that fails.
2. A code of conduct, once the maintainer gives a contact address.
3. iOS on physical devices (today the CLI uses `simctl`).
4. History on iOS for `UIView.animate` animations, if users need more than their start and end (ADR 0005).

## Known limits

Listed in [agents.md, "What it does not see yet"](agents.md#what-it-does-not-see-yet).
