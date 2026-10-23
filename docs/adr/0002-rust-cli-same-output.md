# 0002 The CLI is one Rust binary with the first CLI's output byte for byte

Status: accepted, 2026-10-23. Replaces the Python CLI.

## Decision

The CLI is written in Rust and ships as one native binary per platform. It prints exactly what the first CLI (Python) printed: `py.rs` reproduces Python's number formatting and `json.dumps` separators, and `cli/test_ddp.py` checks the output from the outside.

## Why

- One binary installs with `npm i -g agent-ddp` and needs no runtime on the machine.
- Measured against the Python CLI: 4 to 14 times faster (`skills list` 42 ms to 3 ms, `read` 97 ms to 22 ms). Agents call ddp many times in a row, so start-up time adds up.
- Keeping the output identical kept every guide, example and test valid. A parity run over about 140 commands on 7 real takes found no difference.

## Patterns

A dispatch table from command name to handler (`main.rs`), one module per area, errors as values with a stable code and a next step.

## Consequences

- Python-style printing stays in one place, `py.rs`. Code that prints a value goes through it.
- Builds for macOS, Linux (musl) and Windows run in the release workflow.
