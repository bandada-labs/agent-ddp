# Status

Last update: 2026-10-23. Read this after a break, then [DECISIONS.md](DECISIONS.md).

## In one paragraph

A small library inside a Debug build of an iOS or Android app serves what the app draws, frame by frame (ADR 0001). The `ddp` CLI, one Rust binary on npm (ADR 0002, 0003), reads it live or from recordings and prints short text for coding agents (ADR 0006). The protocol between them is versioned (ADR 0004). A flight recorder keeps the last 15 minutes on both platforms (ADR 0005).

## Released

0.1.4 on 2026-10-23: npm `agent-ddp`, GitHub release with 5 binaries, JitPack `com.github.bandada-labs:agent-ddp:v0.1.4`, SwiftPM from the repo URL.

## Done since 0.1.4, not released

- Protocol version 1 in every header; the CLI refuses a newer library.
- `ddp history` on iOS.
- History busy throttling fixed and cheaper (4 captures a second while only a clock or spinner changes; about 4% CPU on the samples).
- Unit tests in both libraries; CI runs fmt, clippy, the CLI suites, both library builds and their unit tests.
- README with banner and badges, architecture, protocol, contributor and security docs, this docs folder.
- Code layout: `cli/src/checks/`, `Capture.kt`, `Capture.swift`.

## Next

1. Release 0.1.5 so npm shows the new README (needs the maintainer's yes).
2. Enable GitHub private vulnerability reporting (maintainer, repo settings).
3. A code of conduct, once the maintainer gives a contact address.
4. iOS on physical devices (today the CLI uses `simctl`).
5. History on iOS for `UIView.animate` animations, if users need more than their start and end (ADR 0005).

## Known limits

Listed in [agents.md, "What it does not see yet"](agents.md#what-it-does-not-see-yet).
