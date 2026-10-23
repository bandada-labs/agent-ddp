# Agent-DDP survey

Date: 2026-10-22. Xcode 27.1, iOS 27.0 simulator "Agent-DDP iPhone", Android emulator AVD `agent_ddp_api35` (API 35, port 5580).

Sample apps: `samples/ios` (SwiftUI) and `samples/android` (Compose). Both have a button whose fill, border color and border width animate over 2 s on tap, and a list row with a context menu (long press). The iOS app also has a capsule with an animated gradient, width and shadow.

"Mid-animation" below means values read at about t+0.4 s, 1.1 s, 1.8 s and 3 s after the tap.

| Tool | Borders / fills | Text boxes | Z-order / clipping | Live values mid-animation | Cost |
|---|---|---|---|---|---|
| agent-device 0.21.30 `snapshot` (iOS) | No | Label + rect | Tree order only, no clip info | Only label changes ("Tap me" to "Selected" at once) | No app code, no debugger. ~0.4 s per snapshot |
| agent-device `perf frames` (iOS sim) | - | - | - | No: "available only on connected iOS devices" | - |
| agent-device `snapshot` (Android) | No | Label + rect | No. testTag not exposed unless app sets `testTagsAsResourceId` | Label only | No app code |
| agent-device `perf frames` (Android) | - | - | - | Frame timing only (14 frames, 2 dropped, worst 19.4 ms), from `dumpsys gfxinfo` | Free |
| xc-mcp (toba/xc-mcp) | Not tested | | | | Repo returns 404 (GitHub and gh API) on 2026-10-22. Its `debug_view_hierarchy` per old README was LLDB + recursiveDescription, covered by the rows below |
| LLDB + `recursiveDescription` (iOS) | Only UIKit views. SwiftUI content shows as anonymous `CALayer` / `CGDrawingLayer` | Text is a bitmap layer, frame only | View order yes; `clipsToBounds` shown | No | Debugger attach, ~10 s first attach in batch mode |
| LLDB + CALayer walk (`probes/ios_layer_sampler.py`) | Yes. SwiftUI turns simple fill/stroke/gradient/shadow into plain CALayer props: `backgroundColor`, `borderWidth`, `borderColor`, `cornerRadius`, gradient `colors`, `shadowRadius` | Frame of text bitmap, no string | Sublayer order, `masksToBounds`, opacity | Yes. Border 2.00, 2.45, 4.56, 5.97, 6.00; fill white to blue; capsule width and shadow too. SwiftUI sets model values per frame, no CAAnimation, so no presentation layer needed | Attach ~3 s once, then ~0.2 s per sample. Each sample pauses the app. Layers have no ids, map by frame. Context menu is in another window |
| SwiftUI view debug data (`makeViewDebugData` via LLDB) | Yes, as declared: `_ShapeView<RoundedRectangle, Color>`, stroke `lineWidth`, resolved fill color, corner size | Yes: Text string + frame | View tree order | No, it holds target values, not interpolated ones | App must launch with env `SWIFTUI_VIEW_DEBUG=287`, then LLDB. ~1 MB JSON for this tiny screen. Private API |
| XcodeBuildMCP 2.7.0 `debugging attach` + `lldb-command` | Same as LLDB above (it is LLDB) | Same | Same | Yes, same values | Needs `XCODEBUILDMCP_DEBUGGER_BACKEND=lldb-cli`; default DAP backend refuses raw commands. ~1.8 s per command. `snapshot-ui` is the AX tree again |
| adb `uiautomator dump` | No | Text + bounds | No | No | Free |
| adb `dumpsys activity top` | No | No | View tree stops at `AndroidComposeView`, nothing inside Compose | No | Free |
| Compose Layout Inspector (`ui-inspection`) | Yes, in Studio | Yes | Yes | Partly (recomposition counts) | Not testable: needs Android Studio's transport, no CLI. Studio is not installed here |
| Compose Buddy (mikepenz) | Yes (colors, shapes, modifiers) | Yes | Yes | Not documented | Renders `@Preview` functions, not the running app screen. Experimental, repo archived 2026-07-13. Not installed |
| In-app Compose probe (`DebugProbe.kt`, ~60 lines, debug only) | Yes: `BorderModifierNodeElement width/color/shape`, `BackgroundElement color/shape` | Text + bounds + testTag | `GraphicsLayerElement clip/shape/alpha`, semantics order | Yes. Border 2.23, 5.14, 5.96, 6.00 dp; fill white to blue. Needs `isDebugInspectorInfoEnabled = true` | Code in the app. No debugger, triggered by `adb shell am broadcast`. Misses popup windows (DropdownMenu) |
| Flipper native layout plugin, Reveal | Not tested | | | | Both need an SDK/framework inside the app and a desktop GUI. Flipper's RN integration is deprecated; no CLI export found for Reveal |

## What the tests showed beyond the table

- On Android the first build drew the Compose button under the action bar. uiautomator, agent-device and the semantics tree all reported the button at its bounds as normal. `adb input tap` on it did nothing and no tool said why. Only a screenshot showed it. Nothing on the list sees occlusion.
- On iOS the context menu shows up fine in the AX tree (Rename, Delete). On Android, uiautomator sees the DropdownMenu items too.

## Verdict

There is a real gap, but it is smaller than "build a DevTools protocol". The raw data is there on both platforms:

- iOS: SwiftUI already flattens most shapes into CALayers with readable fill, border, gradient and shadow values, updated every frame. LLDB can read them mid-animation today. What is missing is the packaging: no tool samples over time, none joins layers to the SwiftUI view or accessibility id, and every read pauses the app.
- Android: nothing outside Studio reads inside Compose. A small debug-only probe gets the full modifier values mid-animation without a debugger.

No tool on the list can answer "how did this move between t0 and t1" or "is this element covered by something else".

Smallest useful Agent-DDP:

1. A debug-only in-app library per platform (Swift package, Android debug artifact). On a command it records N frames: per element its frame, fill, border, corner, opacity, clip, and its accessibility id or test tag. iOS reads the CALayer tree on a CADisplayLink tick, Android reads semantics + modifier info on each frame. No debugger, no pausing.
2. Output as compact JSON lines per frame, plus an occlusion check (which hit-testable element is covered by another layer at its center).
3. Transport: a local socket or plain logcat / file in the app container, so agent-device can fetch it. No new device control.

Skip for v1: text glyph rendering, UIKit/Views legacy paths (the AX tree plus recursiveDescription already cover them), physical devices, and any GUI.

## Follow-up: custom Path, Canvas, drawingGroup (iOS)

Added to the sample, all animated by the same 2 s tap: a custom `Triangle: Shape` with fill, stroke and x offset; a `Canvas` drawing a circle whose position, size, color and stroke width follow an `Animatable` progress; a `RoundedRectangle` with fill and stroke under `.drawingGroup()` whose width animates.

| Case | What SwiftUI creates | Fill / border mid-animation | Position / size mid-animation |
|---|---|---|---|
| Custom `Path` shape | Two `ColorShapeLayer` (a `CAShapeLayer` subclass), one for fill, one for stroke | Yes. `fillColor`, `strokeColor`, `lineWidth` interpolate: stroke 6.00, 5.62, 3.60, 2.07, 2.00 | Yes. Layer frame and `CGPath` bounds |
| `Canvas` | One `RBDrawingLayer` with an `RBSurfaceContentsLayer` bitmap | No. Colors and line width are pixels only | Partly. The layer frame shrinks to the drawn content (48x60, 48x48, 32x32), so you get the bounding box of everything drawn, not of each shape |
| `.drawingGroup()` | Same: one `RBDrawingLayer` + bitmap | No. Fill and stroke of the inner shapes are gone | Yes for the group as a whole: width 80, 77.1, 62.0, 50.5, 50 |

So the open risk is real but narrow. Plain shapes, including custom `Path` shapes, stay readable as layers. `Canvas` and `drawingGroup` (and likely anything else Metal-backed) become one bitmap per group: the recorder can report its frame per frame, but colors and borders inside it would need pixel sampling from the bitmap or the SwiftUI view debug data (declared values only, and a Canvas closure is opaque there). Neither was tested. For v1 this means: report these groups as "opaque drawing, bounds only" and flag them, instead of guessing.

Note on the sampler: `CAShapeLayer` values need their own fields (`fillColor`, `strokeColor`, `lineWidth`, path bounds). The first version of `probes/ios_layer_sampler.py` read only `backgroundColor` and `border*`, which are empty on shape layers. Fixed in the probe.

## Files

- `samples/ios`, `samples/android`: the two sample apps.
- `probes/ios_layer_sampler.py`: LLDB script that attaches, taps through agent-device and samples the layer tree.
- `tools/`: project-local npm installs of agent-device and xcodebuildmcp (`npm i --ignore-scripts`).
