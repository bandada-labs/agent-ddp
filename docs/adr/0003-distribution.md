# 0003 npm with Trusted Publishing, JitPack and SwiftPM, all from one tag

Status: accepted, 2026-10-23

## Decision

- One version for every part. Pushing a tag `vX.Y.Z` runs `.github/workflows/release.yml`: it builds 5 CLI binaries, creates the GitHub release and publishes the unscoped npm package `agent-ddp` with provenance through npm Trusted Publishing (OIDC), with no stored token.
- The npm package is a small launcher (`npm/bin/ddp.js`) plus the binaries, and picks `ddp-<platform>-<arch>`.
- Android: JitPack builds `com.github.bandada-labs:agent-ddp:<tag>` from `jitpack.yml`, which must not `cd` (JitPack then finds no artifacts).
- iOS: SwiftPM from the repo URL, through the root `Package.swift`.
- Releases happen only when the maintainer says so.

## Why

- `npm i -g agent-ddp` is the install path agents already know from similar tools.
- The maintainer's npm account requires 2FA and the package disallows tokens that bypass it. Trusted Publishing keeps that and still publishes from CI.
- JitPack and SwiftPM need no account or upload step: they build from the tag.

## Patterns

None needed: configuration only.

## Consequences

- The workflow checks that `cli/Cargo.toml` and `npm/package.json` match the tag. The install lines in README.md and docs/ must be bumped by hand.
- `npm/publish.sh` is the manual fallback from release binaries.
