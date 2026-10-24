#if DEBUG && canImport(UIKit)
import UIKit

/// Where a touch on an element would land, asked of UIKit and SwiftUI the way a real touch asks them, without touching.
/// UIKit answers with `hitTest`. Inside SwiftUI content the hosting view's `_hitTestWithContext:` (private, the
/// call UIKit makes for a touch) answers with the gesture that would get it. A gesture belongs to the element when it
/// does not also win just outside the element's frame; a clear view on top that takes touches wins there too.
// ponytail: a coverer exactly the element's size and with the same kind of gesture reads as the element's own.
enum Hit {
    /// Every tappable element, or the one named by id or label.
    static func check(name: String?) throws -> [[String: Any]] {
        let found = AccessibilityReader.objects(in: FrameCapture.visibleWindows())
        let chosen = name.map { name in found.filter { $0.element["id"] as? String == name || $0.element["label"] as? String == name } }
            ?? found.filter { $0.element["tap"] as? Bool == true }
        if let name, chosen.isEmpty { throw Control.Failure(description: "No element '\(name)'") }
        return chosen.map { check($0.object, $0.element) }
    }

    private enum Taker: Equatable {
        case nothing
        case view(UIView)
        case gesture(NSObject, String)
        // A List row: UIKit's cell selection runs the row's action.
        case cell(UIView)

        static func == (left: Taker, right: Taker) -> Bool {
            switch (left, right) {
            case (.nothing, .nothing): return true
            case let (.view(a), .view(b)), let (.cell(a), .cell(b)): return a === b
            case let (.gesture(a, _), .gesture(b, _)): return a === b
            default: return false
            }
        }
    }

    private static func check(_ object: NSObject, _ element: [String: Any]) -> [String: Any] {
        let frame = object.accessibilityFrame
        var result = element
        // The center first, then a 3 by 3 grid, so a partly covered element still gets a point that reaches it.
        let fractions: [CGFloat] = [0.5, 1.0 / 6, 5.0 / 6]
        let points: [CGPoint] = fractions.flatMap { y in
            fractions.map { x in CGPoint(x: frame.minX + frame.width * x, y: frame.minY + frame.height * y) }
        }
        let reaching = points.filter { reaches(object, frame, at: $0) }
        result["points"] = reaching.count
        if let point = reaching.first { result["point"] = [round2(point.x), round2(point.y)] }
        if object.accessibilityTraits.contains(.notEnabled) {
            result["reason"] = "disabled"
        } else if reaching.first != points[0] {
            let (reason, by) = blocker(object, at: points[0])
            result["reason"] = reason
            if let by { result["by"] = by }
        }
        return result
    }

    private static func reaches(_ object: NSObject, _ frame: CGRect, at point: CGPoint) -> Bool {
        let taker = taker(at: point)
        switch taker {
        case .nothing: return false
        case let .view(view):
            guard let own = object as? UIView else { return false }
            return view === own || view.isDescendant(of: own)
        case let .cell(cell): return cell.convert(cell.bounds, to: nil).contains(CGPoint(x: frame.midX, y: frame.midY))
        case .gesture:
            let margin: CGFloat = 2
            let outside = [CGPoint(x: frame.minX - margin, y: frame.midY), CGPoint(x: frame.maxX + margin, y: frame.midY),
                           CGPoint(x: frame.midX, y: frame.minY - margin), CGPoint(x: frame.midX, y: frame.maxY + margin)]
            return !outside.contains { Self.taker(at: $0) == taker }
        }
    }

    private static func taker(at point: CGPoint) -> Taker {
        let windows = FrameCapture.visibleWindows().sorted { $0.windowLevel > $1.windowLevel }
        guard let window = windows.first(where: { $0.hitTest($0.convert(point, from: nil), with: nil) != nil }),
              let view = window.hitTest(window.convert(point, from: nil), with: nil) else { return .nothing }
        guard let host = sequence(first: view, next: \.superview).first(where: isHostingView) else { return .view(view) }
        if let gesture = swiftUIHit(host, at: host.convert(point, from: nil)) {
            return .gesture(gesture, gestureName(gesture))
        }
        if let cell = sequence(first: view, next: \.superview).first(where: { $0 is UICollectionViewCell || $0 is UITableViewCell }) {
            return .cell(cell)
        }
        // A UIKit view inside SwiftUI content is its own taker; the hosting view itself takes nothing.
        return view === host ? .nothing : .view(view)
    }

    private static func isHostingView(_ view: UIView) -> Bool {
        NSStringFromClass(type(of: view)).contains("HostingView")
    }

    private static let contextSelector = NSSelectorFromString("contextWithPoint:radius:")
    private static let hitTestSelector = NSSelectorFromString("_hitTestWithContext:")

    private static func swiftUIHit(_ host: UIView, at point: CGPoint) -> NSObject? {
        guard let contextClass = NSClassFromString("_UIHitTestContext"), host.responds(to: hitTestSelector),
              let make = class_getMethodImplementation(object_getClass(contextClass), contextSelector) else { return nil }
        typealias Make = @convention(c) (AnyClass, Selector, CGPoint, CGFloat) -> Unmanaged<NSObject>?
        guard let context = unsafeBitCast(make, to: Make.self)(contextClass, contextSelector, point, 0)?.takeUnretainedValue(),
              let hit = host.perform(hitTestSelector, with: context)?.takeUnretainedValue() as? NSObject,
              // Without a gesture, SwiftUI answers with a view (a platform view, or the host); that is UIKit's case.
              !(hit is UIView) else { return nil }
        return hit
    }

    /// The gesture's SwiftUI name from the container's description, such as `TapGesture`; a Button's is `Button`.
    private static func gestureName(_ container: NSObject) -> String {
        let description = container.description
        if description.contains("_ButtonGesture") { return "Button" }
        let names = ["SpatialTapGesture", "TapGesture", "LongPressGesture", "DragGesture", "MagnifyGesture", "RotateGesture"]
        return names.first { description.contains($0) } ?? "a SwiftUI gesture"
    }

    /// Why the touch at the element's center misses it, and what takes it instead.
    private static func blocker(_ object: NSObject, at point: CGPoint) -> (String, String?) {
        if let view = object as? UIView {
            for ancestor in sequence(first: view, next: \.superview) {
                let name = className(ancestor)
                if !ancestor.isUserInteractionEnabled { return ("untouchable", "\(name) isUserInteractionEnabled false") }
                if ancestor.isHidden || ancestor.alpha < 0.01 { return ("untouchable", "\(name) hidden") }
                if !ancestor.point(inside: ancestor.convert(point, from: nil), with: nil) {
                    return ("outside", "\(name) bounds")
                }
            }
        }
        switch taker(at: point) {
        case .nothing: return ("untouchable", nil)
        case let .gesture(_, name): return ("covered", name)
        case let .view(view), let .cell(view): return ("covered", described(view))
        }
    }

    private static func described(_ view: UIView) -> String {
        let named = sequence(first: view, next: \.superview).lazy.compactMap(AccessibilityReader.identifier(of:)).first
        return named ?? className(view)
    }

    /// UIKit class names; SwiftUI's mangled generic ones are cut to their base name.
    private static func className(_ view: UIView) -> String {
        let name = NSStringFromClass(type(of: view))
        guard name.hasPrefix("_Tt") else { return name }
        return isHostingView(view) ? "SwiftUI content" : "SwiftUI view"
    }
}
#endif
