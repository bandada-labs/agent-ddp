---
name: ddp
description: See what a Debug iOS or Android app draws, frame by frame, and check its animations (Device DevTools Protocol). Use to inspect a native app screen, debug or review an animation, find elements covered by others, compare iOS and Android, or watch values while an animation runs. Also for a running app's profile, memory and leaks, last crash, runtime warnings, a breakpoint at a line, its storage, preferences and network (mock or fail requests), and device actions (back, background, location, biometrics, push).
allowed-tools: Bash(ddp:*)
---

# ddp

The guides ship with the CLI, so they always match the installed version. Load one before running other commands:

```
ddp skills get core          # the loop, which command for which question, refs, waits, checks, troubleshooting
ddp skills get reviewer      # review an animation step by step
ddp skills get core --full   # plus every command and output shape
ddp skills list
```

Not installed? `npm i -g agent-ddp`.
