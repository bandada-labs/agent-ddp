# Maintainer decisions

The maintainer's decisions in force, one line each with their words. The maintainer often writes in Spanish; the words here are a faithful English translation, and [decisions-log.md](decisions-log.md) keeps the original. Follow these without asking again. When one conflicts with something new, say so once and follow the maintainer's call. Technical decisions live in [adr/](adr/README.md).

A line leaves this file when the maintainer replaces it or when its work has landed and the code or an ADR carries it. The log keeps the history.

## What needs the maintainer's yes

- Anything that leaves the machine: pushing, tagging, releasing, publishing to npm, JitPack or anywhere else, opening issues or pull requests outside the repo. (2026-10-23, "we publish whenever I say, it doesn't have to be all the time")
- New dependencies in the libraries, since they ship inside other people's apps.

## Product

- The README says what ddp does today, then a block to hand to your agent: nobody runs this by hand. (2026-10-23)
- Installing must be as easy as agent-browser: `npm i -g agent-ddp`, one native binary. (2026-10-23)
- The npm and GitHub pages should look as good as agent-browser and agent-device, with a styled header and badges in the project's warm palette. (2026-10-23)
- The project has to be at the level of a serious open source project in formality, structure and architecture, for contributors. (2026-10-23)
- The public repo talks only about agent-ddp, never about other projects. (2026-10-23)
- In prose the project is called Agent-DDP. `agent-ddp` stays for the npm package, the repo and the libraries; the command is `ddp`. (2026-10-23)

## Engineering

- The CLI is Rust. (2026-10-23, "Rust first")
- The flight recorder ("what happened 15 minutes ago") exists on both platforms. (2026-10-23, "add iOS, I think it is missing for the black box")
- The core is Android and iOS: names, types and units come from the platforms, never from a browser. No 1:1 compatibility with Chrome or agent-browser. (2026-10-23, "in the core it must be Android and iOS")
- agent-browser is a reference and a guide, not something to copy literally. Take its good ideas (short commands, refs, small output); do not copy browser concepts the app does not have. (2026-10-23, "it's a reference, not literal, it's there to help us as a guide")
- The library serves its own protocol with native names over a WebSocket, not CDP. Translating to browser concepts only pays off for 1:1 compatibility, which is not wanted. ADR 0008, which replaced ADR 0007. (2026-10-23, "yes go ahead")
- No CSS domain and no `getComputedStyle`. Styles are the native values the library captures (layer fill, border, corner, Compose modifiers), under their own names. (2026-10-23, replaces "CSS through the JavaScript DOM")
- The phases go in order (ADR 0008, STATUS.md). Every answer stays small for agents: only the fields we have, no padding to look like Chrome. Low memory in the app and the CLI. Each phase should give agents something agent-device cannot. (2026-10-23)
- `Input` on iOS synthesizes real touches with private API, Debug only: an IOHIDEvent handed to UIKit as backboardd hands it a finger (the approach of Lyft's Hammer). On Android it dispatches `MotionEvent`s to the window. (2026-10-23)
- Device settings (appearance, text size, reduce motion, locale) are not in the library. `ddp set` changes the real setting on the simulator or emulator, so the app reacts as it would for a user. Phase 3 is closed with that. (2026-10-23)
- Scripts are app commands: named Swift or Kotlin closures the app adds in Debug, run with `app.run`. No JavaScript. `net.send` sends a request with the app's own client. (2026-10-23, "buenísimo hagamos eso entonces")
- The CLI speaks the control endpoint with its own small WebSocket client (`cli/src/control.rs`), no new crate. Existing commands keep their output; `tap` is a real touch on both platforms. Then new commands, then v1 goes. (2026-10-23, "continue with phase 6")
- The control endpoint carries over v1's snapshot, stream, eval and time as `screen.*` and `time.*`, and serves the history files. v1's iOS `tap` (the accessibility action) is not carried over, since `input.touch` is a real finger. History stays on disk too, so the CLI can still read it after a crash. (2026-10-23)

## Distribution

- Repo in the GitHub org bandada-labs. npm package unscoped `agent-ddp`, installed with `npm i agent-ddp`, with sebospc as the developer and the package listed under the bandada-labs org. (2026-10-23)
- Releases run on GitHub Actions from a tag, only when the maintainer says. npm publishes with Trusted Publishing, no token. (2026-10-23, "I think it's better to use GitHub Actions")

## How agents work with the maintainer

- Reply to the maintainer in English, even when they write in Spanish. (2026-10-23, "please keep talking to me in English")
- Human-facing text (docs, commits, pull requests) is plain English, short, and never mentions AI.
