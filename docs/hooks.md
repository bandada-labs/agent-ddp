# Engine hooks

The platform pieces the libraries reach outside public API, what depends on each one, and what happens when it is missing. The third hard rule (ARCHITECTURE.md): every hook is registered in `Hooks`, counted in every run, and shown by `ddp doctor`. A hook that fails makes its answer unknown, never a problem. Every check, `check tappable` and `explore` print a line per hook that failed in the app run, and the take keeps them (`inspect.hooks`). Each release checks them all live on the sample apps and updates the last column.

Kinds: `private` (an Apple symbol, class or selector without a public header), `hidden` (an Android SDK member marked @hide, reached by reflection; Android blocks some per target SDK), `internal` (a framework's internals, found by name or read with Mirror; they can change with any release of the framework).

## iOS

| Hook | Kind | What depends on it | When it fails | Checked on |
| --- | --- | --- | --- | --- |
| `accessibility.automation` (`_AXSSetAutomationEnabled`) | private | SwiftUI elements at all | no SwiftUI elements; doctor fails on 0 elements | iOS 27.0 simulator |
| `swiftui.hitTest` (`_UIHitTestContext`, `_hitTestWithContext:`) | private | tap, check tappable, touch size in SwiftUI | points in SwiftUI content are unknown | iOS 27.0 simulator |
| `swiftui.gestureWinner` (gesture container `base.responder`) | internal | whose gesture takes a touch | a SwiftUI gesture never counts as covering | iOS 27.0 simulator |
| `swiftui.responders` (accessibility node geometry, `AGGraphGetWeakValue`) | internal | whose gesture takes a touch | as above; it also fails, instead of reading memory, when the attribute is no longer a one-argument `WeakAttribute` struct whose metadata names its `Value` | iOS 27.0 simulator |
| `uikit.touchRadius` (`contextWithPoint:radius:event:`) | private | the radius SwiftUI hit-tests a touch with | 20 pt, what iOS 27 gives every touch | iOS 27.0 simulator |
| `input.hid` (IOHIDEvent functions, `_enqueueHIDEvent:`, `_contextId`) | private | tap, type, press, input.touch | those commands fail with the missing name | iOS 27.0 simulator |

## Android

| Hook | Kind | What depends on it | When it fails | Checked on |
| --- | --- | --- | --- | --- |
| `windowManager.roots` (`WindowManagerGlobal`) | hidden | popups, menus and dialogs | only the activity window is read | API 35 emulator |
| `gradientDrawable.stroke` (`GradientDrawable.mStrokePaint`) | hidden | View borders | no border read | API 35 emulator |
| `view.touchListener` (`View.mListenerInfo`) | hidden | whether app code decides a touch | every View is unknown | API 35 emulator |
| `decorView.window` (`DecorView.mWindow`) | hidden | whether the Activity decides a touch; pixels before API 34 | the window's touches are unknown | API 35 emulator |
| `viewGroup.touchPoint` (`isTransformedTouchPointInView`) | hidden | tap, check tappable on Views | every View is unknown | API 35 emulator |
| `viewGroup.touchOrder` (`buildTouchDispatchChildList`) | hidden | the order Views are asked | blocked from API 29: drawing order then Z, its documented rule (fallback) | API 35 emulator (fallback) |
| `view.canReceivePointerEvents` | hidden | which Views can get a touch | blocked: visible or animating, its own definition (fallback) | API 35 emulator (fallback) |
| `compose.hitTest` (`LayoutNode.hitTest`) | internal | tap, check tappable, touch size on Compose | Compose points are unknown | Compose UI 1.7.6 |
| `compose.zSortedChildren` (`LayoutNode.getZSortedChildren`) | internal | the Compose node walk | no Compose nodes below the root | Compose UI 1.7.6 |
| `compose.interopLayout` (`AndroidViewHolder.getLayoutNode`) | internal | an AndroidView inside Compose | its View is not reached | Compose UI 1.7.6 |
| `compose.textColor` (BasicText's color producer) | internal | whether a text's color is known | the text is left out of pixel checks | Compose UI 1.7.6 |

Not hooks: `drawHierarchy`, `PixelCopy`, `CADisplayLink`, Choreographer, `UIView.hitTest`, `onInterceptTouchEvent` and Compose `PointerInputModifierNode` are public API.
