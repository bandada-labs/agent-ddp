# Engine hooks

The platform pieces the libraries reach outside public API, what depends on each one, and what happens when it is missing. The third hard rule (ARCHITECTURE.md): every hook is registered in `Hooks`, counted in every run, and shown by `ddp doctor`. A hook that fails makes its answer unknown, never a problem. Every check, `check tappable` and `explore` print a line per hook that failed in the app run, and the take keeps them (`inspect.hooks`). Each release checks them all live on the sample apps with `samples/hook_matrix.py` and updates the last column. A version not in that column has not been checked.

Kinds: `private` (an Apple symbol, class or selector without a public header), `hidden` (an Android SDK member marked @hide, reached by reflection; Android blocks some per target SDK), `internal` (a framework's internals, found by name or read with Mirror; they can change with any release of the framework).

## iOS

| Hook | Kind | What depends on it | When it fails | Checked on |
| --- | --- | --- | --- | --- |
| `accessibility.automation` (`_AXSSetAutomationEnabled`) | private | SwiftUI elements at all | no SwiftUI elements; doctor fails on 0 elements | iOS 26.5 and 27.0 simulators |
| `swiftui.hitTest` (`_UIHitTestContext`, `_hitTestWithContext:`) | private | tap, check tappable, touch size in SwiftUI | points in SwiftUI content are unknown | iOS 26.5 and 27.0 simulators |
| `swiftui.gestureWinner` (gesture container `responder`, iOS 27 `base.responder`) | internal | whose gesture takes a touch | a SwiftUI gesture never counts as covering | iOS 26.5 and 27.0 simulators |
| `swiftui.responders` (accessibility node geometry, `AGGraphGetWeakValue`) | internal | whose gesture takes a touch | as above; it also fails, instead of reading memory, when the attribute is no longer a one-argument `WeakAttribute` struct whose metadata names its `Value` | iOS 26.5 and 27.0 simulators |
| `uikit.touchRadius` (`contextWithPoint:radius:event:`) | private | the radius SwiftUI hit-tests a touch with | 20 pt, what iOS 27 gives every touch | iOS 26.5 and 27.0 simulators |
| `input.hid` (IOHIDEvent functions, `_enqueueHIDEvent:`, `_contextId`) | private | tap, type, press, input.touch | those commands fail with the missing name | iOS 26.5 and 27.0 simulators |
| `renderbox.displayList` (`RBDisplayList`, `drawInDisplayList:`, `RBXMLRecorderStart`) | private | `draw` nodes inside a SwiftUI `Canvas` or `drawingGroup` | no `draw` nodes; the `opaque-drawing` node and its pixels stay | iOS 26.5 simulator |
| `swiftui.viewDebugData` (`_UIHostingView._accessibilitySwiftUIDebugData`: the view tree with each node's type and the accessibility elements made from it, with `SWIFTUI_VIEW_DEBUG` at launch; `_viewDebugData()` traps on iOS 27 when called outside SwiftUI's update) | private | `ddp source` on SwiftUI | the element gets no source information | iOS 27.0 simulator (probe app and sample, 2026-11-03) |
| `swiftui.viewTypes` (the tree's node types as metatypes; `_typeByName` for views only named in a body; `_typeName` of each body type; the nominal type descriptor in Swift's type metadata, to tell the app's types from Apple's) | internal | `ddp source` on SwiftUI: which app view wrote the element | the element's view is not known | iOS 27.0 simulator (probe app and sample, 2026-11-03) |

## Mac host (iOS apps without the library, `--device`)

ddp itself reaches these, from the Mac, for a booted simulator: nothing runs in the simulator and there is no XCTest. Each request waits for the simulator's answer; no answer fails the command with the hook's name, never a partial answer. `ddp doctor <sim UDID>` asks both once.

| Hook | Kind | What depends on it | When it fails | Checked on |
| --- | --- | --- | --- | --- |
| `simulator.hid` (CoreSimulator `SimServiceContext`, `-[SimDevice lookup:error:]`, libxpc `xpc_endpoint_create_mach_port_4sim` and `xpc_connection_enable_sim2host_4sim`, `dtuhidd`'s `IndigoDigitizerEvent` and `IndigoKeyboardButtonEvent` messages) | private | `tap`, `double-tap`, `long-press`, `swipe`, `drag`, `type`, `press` with `--device` | the command fails; a touch is only reported once `dtuhidd` answered after it | iOS 27.0 simulator, Xcode 27.2 |
| `simulator.accessibility` (`AccessibilityPlatformTranslation`'s `AXPTranslatorRequest`, `AXPTranslatorResponse`, `AXPTranslationObject`, archived to `com.apple.CoreSimulator.accessibility`) | private | `tree --device`, places given by label or id | the command fails; a tree is never shown with elements missing | iOS 27.0 simulator, Xcode 27.2 |

## Android

| Hook | Kind | What depends on it | When it fails | Checked on |
| --- | --- | --- | --- | --- |
| `windowManager.roots` (`WindowManagerGlobal`) | hidden | popups, menus and dialogs | only the activity window is read | API 29, 33 and 35 emulators |
| `gradientDrawable.stroke` (`GradientDrawable.mStrokePaint`) | hidden | View borders | no border read | API 29, 33 and 35 emulators |
| `view.touchListener` (`View.mListenerInfo`) | hidden | whether app code decides a touch | every View is unknown | API 29, 33 and 35 emulators |
| `decorView.window` (`DecorView.mWindow`) | hidden | whether the Activity decides a touch; pixels before API 34 | the window's touches are unknown | API 29, 33 and 35 emulators |
| `viewGroup.touchPoint` (`isTransformedTouchPointInView`) | hidden | tap, check tappable on Views | every View is unknown | API 29, 33 and 35 emulators |
| `viewGroup.touchOrder` (`buildTouchDispatchChildList`) | hidden | the order Views are asked | blocked from API 29: drawing order then Z, its documented rule (fallback) | API 29, 33 and 35 emulators (fallback) |
| `view.canReceivePointerEvents` | hidden | which Views can get a touch | blocked: visible or animating, its own definition (fallback) | API 29, 33 and 35 emulators (fallback) |
| `compose.hitTest` (`LayoutNode.hitTest`) | internal | tap, check tappable, touch size on Compose | Compose points are unknown | Compose UI 1.7.6 on API 29, 33 and 35 |
| `compose.zSortedChildren` (`LayoutNode.getZSortedChildren`) | internal | the Compose node walk | no Compose nodes below the root | Compose UI 1.7.6 on API 29, 33 and 35 |
| `compose.interopLayout` (`AndroidViewHolder.getLayoutNode`) | internal | an AndroidView inside Compose | its View is not reached | Compose UI 1.7.6 on API 29, 33 and 35 |
| `compose.textColor` (BasicText's color producer) | internal | whether a text's color is known | the text is left out of pixel checks | Compose UI 1.7.6 on API 29, 33 and 35 |
| `compose.drawCache` (`CacheDrawScope()`, `setCacheParams`, `DrawResult.getBlock`) | internal | what a `drawWithCache` lambda draws | its draws are not read; a cache that needs a graphics layer fails it too | Compose UI 1.7.6 on API 35 |
| `compose.inspectionTables` (the `inspection_slot_table_set` view tag) | internal | `ddp source` | the element gets no source information | Compose UI 1.7.6 on API 35 |
| `compose.sourceInformation` (ui-tooling-data `CompositionData.asTree`, the app's own copy) | internal | `ddp source` | the element gets no source information; a missing library says to add `ui-tooling-data` | Compose UI 1.7.6 on API 35 |
| `compose.tracer` (`ComposerKt.compositionTracer`) | internal | `ddp recompositions`: the app's own tracer (Perfetto's runtime-tracing) is kept, gets every call, and is put back | an app tracer is replaced for the window and the field is reset to none | Compose runtime 1.7.6 on API 35 |
| `compose.wrappedComposition` (`WrappedComposition.getOriginal`) | internal | `ddp recompositions`: which state invalidated a composable | runs are still counted; causes are not given | Compose UI 1.7.6 on API 35 |

Not hooks: `drawHierarchy`, `PixelCopy`, `CADisplayLink`, Choreographer, `UIView.hitTest`, `onInterceptTouchEvent` and Compose `PointerInputModifierNode` are public API.
