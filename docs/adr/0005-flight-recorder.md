# 0005 A flight recorder in the library for the last 15 minutes

Status: accepted, 2026-10-23. Android first, iOS added the same day.

## Decision

The library records, without being asked, every frame that differs from the one before, plus taps, swipes, screen changes and launches, for the last 15 minutes. Files are one minute each, in the recording format with `"history": true`, gzipped when closed. `ddp history` prints a timeline, and `ddp history at <time>` turns a moment into a take.

When to capture:

- Android: after the activity window draws (`OnDrawListener`).
- iOS: when the main run loop is about to sleep after a turn, after Core Animation's commit.
- A still app captures nothing, because neither signal fires.
- An element that keeps changing for 2 s, never still for longer than the quiet gap, is busy. While only busy elements change, capture at most every 250 ms; anything else changing goes back to every frame. The quiet gap must be longer than that interval (it is twice), or a throttled clock looks still between captures and the throttle flips on and off.
- A capture that saw nothing new keeps the busy state.

## Why

- Agents are asked about things that already happened ("a minute ago it flickered"). Reproducing is slow and often fails.
- Measured on the sample apps, with a clock that never stops: Android 55% to 59% CPU on the emulator (rendering dominates), iOS 2.5% to 6.2% on the simulator. Before the throttle fixes, iOS was at 50%.
- Files on the device survive a crash.

## Patterns

Observer (draw listener, run loop observer, a passive gesture recognizer and a `Window.Callback` decorator for touches), a pure `ChangeTracker` with unit tests on both platforms, a single writer queue so the main thread never writes files.

## Consequences

- iOS keeps only the start and end of animations Core Animation runs by itself (`UIView.animate`), because they do not wake the app.
- Android wakes only on the activity window, so a dialog animating over a still screen shows at the next screen draw.
- Live streams feed their frames to the recorder, so recording does not capture twice.
