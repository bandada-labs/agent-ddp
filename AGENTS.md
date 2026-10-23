# Notes for coding agents working on this repo

This file is for agents that change agent-ddp itself. To use ddp on an app, run `ddp skills get core` instead.

- Read [ARCHITECTURE.md](ARCHITECTURE.md) before a change. Its rules are not optional: Debug only, read-only library, cheap main thread, local endpoint, one wire format, no daemon, stable output.
- The protocol in [docs/protocol.md](docs/protocol.md) is shared by the iOS library, the Android library and the CLI. A change to it touches all three, or explains why not.
- CLI output is a contract that tests compare as text. `py.rs` keeps numbers and JSON printed the same way everywhere. Do not format values by hand.
- Errors go through `fail!` with a stable code and a `next` command the agent can run.
- Before you finish a CLI change, run:

  ```bash
  cargo fmt --manifest-path cli/Cargo.toml
  cargo clippy --release --manifest-path cli/Cargo.toml -- -D warnings
  cargo test --release --manifest-path cli/Cargo.toml
  cargo build --release --manifest-path cli/Cargo.toml && python3 cli/test_ddp.py
  ```

- A library change is checked on a simulator or emulator with the sample app in `samples/`. Report the numbers, not only "it works".
- Update `docs/agents.md`, `docs/protocol.md` and `docs/skills/` in the same change. The guides are compiled into the binary.
- Do not add dependencies to the libraries. They ship inside other people's apps.
