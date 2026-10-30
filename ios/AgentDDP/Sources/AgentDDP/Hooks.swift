#if DEBUG && canImport(UIKit)
import UIKit

/// Every engine piece the library reaches outside public API, how fragile that path is, and how it behaved in this
/// run, for `app.hooks` and `ddp doctor`. A hook that stops working on a new iOS shows here; the checks that depend on
/// it go quiet rather than guess.
enum Hooks {
    /// `private`: an Apple symbol, class or selector without a public header. `internal`: a framework's Swift
    /// internals, read through Mirror or AttributeGraph; they can change with any SwiftUI release.
    struct Hook {
        let name: String
        let kind: String
        let feeds: String
    }

    static let all = [
        Hook(name: "accessibility.automation", kind: "private", feeds: "SwiftUI elements (_AXSSetAutomationEnabled)"),
        Hook(name: "swiftui.hitTest", kind: "private", feeds: "tap, check tappable, touch size (_hitTestWithContext:)"),
        Hook(name: "swiftui.gestureWinner", kind: "internal", feeds: "whose gesture takes a touch (container base)"),
        Hook(name: "swiftui.responders", kind: "internal", feeds: "whose gesture takes a touch (AGGraphGetWeakValue)"),
        Hook(name: "uikit.touchRadius", kind: "private", feeds: "the touch radius SwiftUI hit-tests with"),
        Hook(name: "input.hid", kind: "private", feeds: "tap, type, press (IOHIDEvent, _enqueueHIDEvent:)"),
        Hook(name: "renderbox.displayList", kind: "private",
             feeds: "draw nodes inside a SwiftUI Canvas or drawingGroup (RBDisplayList XML recorder)"),
    ]

    private typealias Count = (ok: Int, failed: Int, why: String?)
    private static var counts: [String: Count] = [:]
    private static let lock = NSLock()

    static func ok(_ name: String) { update(name) { $0.ok += 1 } }

    static func failed(_ name: String, _ why: String) { update(name) { $0.failed += 1; $0.why = why } }

    /// One line per hook: `ok` (it answered every time), `failing` (never), `mixed`, or `unused` in this run.
    static func report() -> [[String: Any]] {
        lock.lock()
        defer { lock.unlock() }
        return all.map { hook in
            let count = counts[hook.name] ?? (0, 0, nil)
            var line: [String: Any] = ["name": hook.name, "kind": hook.kind, "feeds": hook.feeds,
                                       "ok": count.ok, "failed": count.failed,
                                       "status": count.ok + count.failed == 0 ? "unused"
                                           : count.failed == 0 ? "ok" : count.ok == 0 ? "failing" : "mixed"]
            if let why = count.why { line["error"] = why }
            if hook.name == "uikit.touchRadius" {
                line["value"] = Double(TouchRadius.points)
                line["source"] = TouchRadius.seen ? "UIKit" : "default until the first touch"
            }
            return line
        }
    }

    private static func update(_ name: String, _ change: (inout Count) -> Void) {
        lock.lock()
        defer { lock.unlock() }
        var count = counts[name] ?? (0, 0, nil)
        change(&count)
        counts[name] = count
    }
}
#endif
