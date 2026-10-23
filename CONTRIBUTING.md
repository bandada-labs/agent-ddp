# Contributing

Thanks for helping. Read [ARCHITECTURE.md](ARCHITECTURE.md) first. It has the rules every change keeps and where each piece lives.

## Before you start

For a bug, open an issue with the `ddp doctor` output and, if you can, the recording (`.ddp/last.jsonl`). For a new command, a new check or a protocol change, open an issue first so we agree on the shape before you write it. Small fixes and docs can go straight to a pull request.

## Set up

You only need the toolchain of the part you change.

| Part | Needs | Build |
| --- | --- | --- |
| CLI (`cli/`) | Rust stable, Python 3 for the black-box tests | `cargo build --release --manifest-path cli/Cargo.toml` |
| iOS library (`ios/AgentDDP`) | Xcode 16 or newer | `xcodebuild -scheme AgentDDP -destination 'generic/platform=iOS Simulator' build` |
| Android library (`android/agent-ddp`) | JDK 17, Android SDK | `./samples/android/gradlew -p samples/android :agent-ddp:assembleRelease` |
| Sample apps | the platform's toolchain, [XcodeGen](https://github.com/yonaskolb/XcodeGen) for iOS | `xcodegen` in `samples/ios`, `:app:installDebug` in `samples/android` |

The sample apps already include the library from this repo, so a change in `ios/` or `android/` shows up in them on the next build.

## Test

The CLI has two layers of tests. CI runs both.

```bash
cargo fmt --check --manifest-path cli/Cargo.toml
cargo clippy --release --manifest-path cli/Cargo.toml -- -D warnings
cargo test --release --manifest-path cli/Cargo.toml
cargo build --release --manifest-path cli/Cargo.toml && python3 cli/test_ddp.py
```

`cli/test_ddp.py` runs the built binary from the outside against small recordings and a fake app. Most CLI changes need a case there. Set `DDP_BIN` to test another binary.

The libraries have unit tests for their pure logic (what changed between frames, busy elements, gzip):

```bash
xcodebuild test -scheme AgentDDP -destination 'platform=iOS Simulator,name=<a simulator you have>'
./samples/android/gradlew -p samples/android :agent-ddp:testDebugUnitTest
```

Most of a library change still needs the real thing. Check it on a simulator or emulator with the sample app:

```bash
ddp doctor <udid or serial> --app dev.agentddp.sample
ddp connect <udid or serial> --app dev.agentddp.sample
ddp read --for 3s --then "ddp tap animatedButton"
ddp check all
```

Put the output, before and after, in the pull request. For changes to capture code, add the `averageCaptureMs` and `slowestCaptureMs` from the `end` line of a take.

## Pull requests

- One topic per pull request. A refactor and a feature go in separate ones.
- Keep both platforms in step. If a change only lands on one, say why and add it to "What it does not see yet" in `docs/agents.md`.
- Update the docs in the same pull request: `docs/agents.md` for commands and output, `docs/protocol.md` for the wire format, `docs/skills/` when agents should use it differently.
- Add a line under "Unreleased" in [CHANGELOG.md](CHANGELOG.md) when users or agents will notice the change.
- Output is part of the contract. If a command prints something different, say so in the description.

## Style

Code should read on its own. Names say what a thing is, comments say why and not what. Match the code around you: Rust is formatted with `rustfmt` (`cli/rustfmt.toml`), Swift and Kotlin follow the files next to them.

Docs and guides are read by agents and by people whose first language is not English. Write short, plain sentences.

## Releases

Maintainers release by pushing a `vX.Y.Z` tag. The release workflow builds the CLI for macOS, Linux and Windows, creates the GitHub release and publishes `agent-ddp` to npm with provenance. JitPack and SwiftPM pick up the same tag. Before tagging, bump the version in `cli/Cargo.toml`, `npm/package.json` and the install lines in `README.md` and `docs/`, and move "Unreleased" in the changelog under the new version.

## License

By contributing you agree that your work is released under the [MIT license](LICENSE).
