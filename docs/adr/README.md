# Architecture decisions (ADRs)

Read the ADR before changing what it decides. A superseded or amended ADR says so at the top. Each ADR names the patterns it uses, from the table in [ARCHITECTURE.md](../../ARCHITECTURE.md#patterns-in-use), or says why none fits.

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-in-app-server-like-devtools.md) | A server inside the Debug build and a CLI client, like Chrome DevTools; no daemon | accepted |
| [0002](0002-rust-cli-same-output.md) | The CLI is one Rust binary with the first CLI's output byte for byte | accepted |
| [0003](0003-distribution.md) | npm with Trusted Publishing, JitPack and SwiftPM, all from one tag | accepted |
| [0004](0004-protocol-version.md) | A protocol version in every header; clients refuse a newer one | accepted |
| [0005](0005-flight-recorder.md) | A flight recorder in the library for the last 15 minutes | accepted |
| [0006](0006-output-for-agents.md) | Output is a contract made for agents | accepted |

## Writing a new one

Copy the shape of an existing ADR: a title that states the decision, `Status: <proposed|accepted|superseded by NNNN>, <date>`, then **Decision**, **Why** (with numbers when there are any), **Patterns** and **Consequences**. Number it with the next free number and add it to the table above in the same change.
