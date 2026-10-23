# 0006 Output is a contract made for agents

Status: accepted, 2026-10-23

## Decision

Every command prints short, deterministic text sized for a model's context: elements with `@eN` refs, sparklines instead of tables of values, one line per problem. Errors carry a stable `code` and a `next` command. Checks exit 1 when they find something. `--json`, `--max-output` and `--content-boundaries` work on every command. The guides agents read (`ddp skills`) ship inside the binary.

## Why

The users are coding agents. Their cost is tokens and wrong guesses: a screenshot costs a lot and does not say a border is 5.25 pt; a vague error leads to retries. Text that is stable between versions lets guides, tests and agents' memories stay valid.

## Patterns

Errors as values (`Failure`), an output sink stack (limit and capture) so `batch` and `mcp` reuse every command.

## Consequences

- A change in what a command prints is a contract change: it goes in CHANGELOG.md and in `cli/test_ddp.py`.
- App text is data, not instructions: `--content-boundaries` marks it, and redaction happens inside the app.
