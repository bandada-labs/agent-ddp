# agent-ddp docs: start here

This folder is the project's memory. Anything a person or an agent needs to know across sessions lives here, not in someone's head. If you learn something the next contributor needs, write it in the file below that owns it.

## Read order

1. [STATUS.md](STATUS.md): where the project is, what is in progress, what is next.
2. [../ARCHITECTURE.md](../ARCHITECTURE.md): how the parts fit and the rules every change keeps.
3. [DECISIONS.md](DECISIONS.md): the maintainer's standing decisions. Follow them without asking again.
4. [adr/README.md](adr/README.md): technical decisions and why. Read the ADR before changing what it decides.
5. [../CONTRIBUTING.md](../CONTRIBUTING.md) to build and test, or [../AGENTS.md](../AGENTS.md) if you are a coding agent.

## Where things are

| Need | File |
| --- | --- |
| The wire format between the library and a client | [protocol.md](protocol.md) |
| Every command, its output and the recording format | [agents.md](agents.md) |
| The private, hidden and internal platform pieces the libraries use, and the versions they were checked on | [hooks.md](hooks.md) |
| The guides `ddp skills` prints to agents | [skills/](skills) |
| What changed in each release | [../CHANGELOG.md](../CHANGELOG.md) |
| The dated history of every maintainer decision | [decisions-log.md](decisions-log.md) |
| What existing tools could see before DDP | [../SURVEY.md](../SURVEY.md) |
| Security model and reporting | [../SECURITY.md](../SECURITY.md) |

## Keeping it current

- Update STATUS.md when a change lands that moves the project, in the same pull request.
- When the maintainer decides something, add it to decisions-log.md with the date and their words, and a one-line rule to DECISIONS.md while it is in force.
- A technical decision that is hard to undo (a format, a dependency, a design that other code builds on) gets an ADR, and a line in adr/README.md in the same change.
- A doc that no longer matches the code is a bug. Fix the doc in the change that makes it wrong.
