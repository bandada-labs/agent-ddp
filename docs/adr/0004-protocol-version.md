# 0004 A protocol version in every header; clients refuse a newer one

Status: accepted, 2026-10-23

## Decision

Every `header` (live streams, recordings, history files) carries `"protocol": N`, now 1. A header without the field reads as 0. A client reads every version up to its own and stops on a higher one: the CLI fails with code `protocol_too_new` and the command to update. Adding an optional field keeps the version; changing a field's meaning or removing a field or command raises it.

## Why

The library and the CLI ship on different paths: the library is pinned in each app's build, the CLI updates with npm. Without a version, a CLI older than the app's library would misread new frames silently.

## Patterns

None beyond one check in one place (`model::check_protocol`), called by the live client, the recording loader and the history loader.

## Consequences

- A protocol change is listed in CHANGELOG.md and in docs/protocol.md.
- The CLI never refuses an older library, so apps can update the library at their own pace.
