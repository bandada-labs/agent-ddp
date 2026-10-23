---
name: explore
description: QA a screen without a script: tap every control, record each, run every check, report what is wrong.
---

# Exploring a screen

Use this to find problems nobody wrote a test for. The app runs a Debug build; you are connected.

## 1. See what is there

```
ddp tree -i
ddp check all --for 1s           # problems before touching anything
```

## 2. Tap every control

```
ddp explore --per 2s --skip logoutButton deleteButton
```

It taps each element marked as a control, records 2 s after each tap, and runs `check all` on that take. One line per control:

```
chipPicked: changed Open, clockBar; overlap 2, a11y 2 (ddp check all --file explore-chipPicked)
```

Skip anything destructive or anything that leaves the app. It stops when a tap leaves the screen; go back (agent-device) and run it again with `--skip`, or explore the new screen.

## 3. Look closer at what failed

```
ddp check all --file explore-chipPicked
ddp read --file explore-chipPicked
ddp get chipPicked --curve --file explore-chipPicked
ddp annotate shot.png            # if you can read images
```

## 4. Vary the device

```
ddp set appearance dark ; ddp explore
ddp set text-size accessibility-large ; ddp explore      # iOS; Android: largest
ddp set reduce-motion on                                 # animations should shorten or stop
```

Set things back when you are done.

## 5. Report

One finding per line: element, what is wrong, the take to reproduce it. For example: "`animatedButton`: text is white on a white fill for the first 300 ms of the tap animation (LOW CONTRAST 1.0:1), take explore-animatedButton".
