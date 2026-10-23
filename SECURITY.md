# Security

## Reporting a problem

Please do not open a public issue for a security problem. Use [GitHub private vulnerability reporting](https://github.com/bandada-labs/agent-ddp/security/advisories/new) instead.

## What the library exposes

The library runs only in Debug builds and opens an endpoint that can read everything the app draws and, if the app opts in, its network requests. The design limits who can reach it:

- iOS simulator: TCP on 127.0.0.1 only, with a random token the app writes inside its own container. Only a process that can read the app's files can connect.
- Android: an abstract unix socket, not reachable from the network. It accepts only the shell user (adb) and root, checked by the peer's uid.
- Network entries are redacted inside the app before they are kept: `Authorization`, `Cookie`, any header, query item or JSON key matching `secret|token|key|password|attest`, and values the app registers.
- The CLI marks text that comes from the app with `--content-boundaries`, so an agent can treat labels and bodies as data and not as instructions.

Things we treat as security bugs: any of this code in a Release build, the endpoint reachable from another host or from another app without adb, a redaction that leaks a secret, and a way for app content to make the CLI run a command.
