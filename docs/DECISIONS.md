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
- The library speaks CDP itself (same messages, domains, methods and events), not new ddp options. `Runtime.evaluate` runs JavaScript: JavaScriptCore on iOS, QuickJS on Android in a debug-only artifact. No Rust core for now. ADR 0007. (2026-10-23)

## Distribution

- Repo in the GitHub org bandada-labs. npm package unscoped `agent-ddp`, installed with `npm i agent-ddp`, with sebospc as the developer and the package listed under the bandada-labs org. (2026-10-23)
- Releases run on GitHub Actions from a tag, only when the maintainer says. npm publishes with Trusted Publishing, no token. (2026-10-23, "I think it's better to use GitHub Actions")

## How agents work with the maintainer

- Reply to the maintainer in English, even when they write in Spanish. (2026-10-23, "please keep talking to me in English")
- Human-facing text (docs, commits, pull requests) is plain English, short, and never mentions AI.
