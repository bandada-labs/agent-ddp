#if DEBUG && canImport(UIKit)
import UIKit

/// Where a touch on an element would land, asked of UIKit and SwiftUI the way a real touch asks them, without touching.
/// UIKit answers with `hitTest`. Inside SwiftUI content the hosting view's `_hitTestWithContext:` (private, the
/// call UIKit makes for a touch) answers with the gesture that would get it. Whether that gesture is the element's own
/// is SwiftUI's answer too: an accessibility node keeps the responders of its view (the ones SwiftUI hit-tests), and a
/// gesture is the element's when it is one of them, inside them or around them in SwiftUI's responder tree.
/// A point is reported as blocked only when the engine says so. When it cannot tell (the responders cannot be read, or
/// a UIKit view sits inside the element's own SwiftUI content), the point counts as reaching: `ddp tap` still sees a
/// miss after the touch, as nothing changed. Which gesture wins once a touch is under way (a parent's tap recognizer,
/// a `highPriorityGesture`) is decided by UIKit only during a real touch, so it is not reported either.
enum Hit {
    /// Every tappable element, or the one named by id or label. With `minimumTouch` (pt), a control the touch reaches
    /// over less than that, across or down, also gets `reach`.
    static func check(name: String?, minimumTouch: CGFloat = 0) throws -> [[String: Any]] {
        let found = AccessibilityReader.objects(in: FrameCapture.visibleWindows())
        let chosen = name.map { name in found.filter { $0.element["id"] as? String == name || $0.element["label"] as? String == name } }
            ?? found.filter { $0.element["tap"] as? Bool == true }
        if let name, chosen.isEmpty { throw Control.Failure(description: "No element '\(name)'") }
        return chosen.map { check($0.object, $0.element, minimumTouch: minimumTouch) }
    }

    private enum Taker {
        case nothing
        /// A private hook the answer needs did not work: nothing is reported from it.
        case unknown
        case view(UIView)
        case gesture(NSObject, String)
        // A List row: UIKit's cell selection runs the row's action.
        case cell(UIView)

    }

    private static func check(_ object: NSObject, _ element: [String: Any], minimumTouch: CGFloat) -> [String: Any] {
        let frame = object.accessibilityFrame
        var result = element
        // The center first, then a 3 by 3 grid, so a partly covered element still gets a point that reaches it.
        let fractions: [CGFloat] = [0.5, 1.0 / 6, 5.0 / 6]
        let points: [CGPoint] = fractions.flatMap { y in
            fractions.map { x in CGPoint(x: frame.minX + frame.width * x, y: frame.minY + frame.height * y) }
        }
        // Read once, and only if a SwiftUI gesture takes one of the points.
        var ownResponders: [AnyObject]?
        let responders = {
            let read = ownResponders ?? SwiftUIResponders.of(object)
            ownResponders = read
            return read
        }
        let reaching = points.filter { !blocked(object, at: $0, responders: responders) }
        result["points"] = reaching.count
        if let point = reaching.first { result["point"] = [round2(point.x), round2(point.y)] }
        if minimumTouch > 0, let (width, height) = touchSize(object, frame, minimum: minimumTouch, responders: responders) {
            result["reach"] = [round2(width), round2(height)]
        }
        if object.accessibilityTraits.contains(.notEnabled) {
            result["reason"] = "disabled"
        } else if reaching.first != points[0] {
            let (reason, by) = blocker(object, at: points[0])
            result["reason"] = reason
            if let by { result["by"] = by }
        }
        return result
    }

    /// Whether the engine says a touch at `point` does not reach the element. `responders` gives the element's own
    /// SwiftUI responders, read only when a SwiftUI gesture takes the point.
    private static func blocked(_ object: NSObject, at point: CGPoint, responders: () -> [AnyObject]) -> Bool {
        switch taker(at: point) {
        case .nothing: return true
        case .unknown: return false
        case let .view(view):
            if let own = object as? UIView { return !(view === own || view.isDescendant(of: own)) }
            // A UIKit view inside the element's own SwiftUI content can be part of it (a representable in its label).
            guard let host = ownView(of: object) else { return false }
            return !view.isDescendant(of: host)
        case let .cell(cell):
            guard let own = ownView(of: object) else { return false }
            return !own.isDescendant(of: cell)
        case let .gesture(container, _):
            // A SwiftUI gesture over a UIKit view: SwiftUI does not say how the two relate.
            guard !(object is UIView), let winner = SwiftUIResponders.winner(of: container) else { return false }
            let own = responders()
            guard !own.isEmpty else { return false }
            let winnerAndParents = SwiftUIResponders.selfAndParents(winner)
            let inOrInside = winnerAndParents.contains { step in own.contains { $0 === step } }
            let around = own.contains { SwiftUIResponders.selfAndParents($0).contains { $0 === winner } }
            return !inOrInside && !around
        }
    }

    /// How far the touch reaches the control across and down through its center, when that is under `minimum`: from
    /// the center out to the first point the engine says it does not reach. Nil when it reaches `minimum` both ways.
    private static func touchSize(_ object: NSObject, _ frame: CGRect, minimum: CGFloat,
                                  responders: () -> [AnyObject]) -> (CGFloat, CGFloat)? {
        let center = CGPoint(x: frame.midX, y: frame.midY)
        func reach(_ dx: CGFloat, _ dy: CGFloat) -> CGFloat {
            let far = minimum / 2 - 0.5
            func isBlocked(_ distance: CGFloat) -> Bool {
                blocked(object, at: CGPoint(x: center.x + dx * distance, y: center.y + dy * distance), responders: responders)
            }
            guard isBlocked(far) else { return far }
            // Half point steps back toward the center: the first point that is not blocked is the edge.
            var distance = far - 0.5
            while distance > 0, isBlocked(distance) { distance -= 0.5 }
            return max(distance, 0)
        }
        let width = reach(-1, 0) + reach(1, 0), height = reach(0, -1) + reach(0, 1)
        return width < minimum - 1 || height < minimum - 1 ? (width, height) : nil
    }

    /// The view UIKit hands a touch at `point` (screen coordinates): the topmost window's `hitTest`.
    private static func hitView(at point: CGPoint) -> UIView? {
        let windows = FrameCapture.visibleWindows().sorted { $0.windowLevel > $1.windowLevel }
        return windows.lazy.compactMap { $0.hitTest($0.convert(point, from: nil), with: nil) }.first
    }

    private static func taker(at point: CGPoint) -> Taker {
        guard let view = hitView(at: point), let window = view.window else { return .nothing }
        guard let host = sequence(first: view, next: \.superview).first(where: isHostingView) else { return .view(view) }
        // The hit test context takes window coordinates, also for a hosting view nested in UIKit.
        switch swiftUIHit(host, at: window.convert(point, from: nil)) {
        case .unavailable: return .unknown
        case let .gesture(gesture): return .gesture(gesture, gestureName(gesture))
        case .noGesture: break
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

    private enum SwiftUIHit {
        case gesture(NSObject)
        case noGesture
        case unavailable
    }

    private static func swiftUIHit(_ host: UIView, at point: CGPoint) -> SwiftUIHit {
        guard let contextClass = NSClassFromString("_UIHitTestContext"), host.responds(to: hitTestSelector),
              let make = class_getMethodImplementation(object_getClass(contextClass), contextSelector) else {
            Hooks.failed("swiftui.hitTest", "no _UIHitTestContext or _hitTestWithContext:")
            return .unavailable
        }
        typealias Make = @convention(c) (AnyClass, Selector, CGPoint, CGFloat) -> Unmanaged<NSObject>?
        guard let context = unsafeBitCast(make, to: Make.self)(contextClass, contextSelector, point, TouchRadius.points)?
            .takeUnretainedValue() else {
            Hooks.failed("swiftui.hitTest", "contextWithPoint:radius: gave nothing")
            return .unavailable
        }
        Hooks.ok("swiftui.hitTest")
        // Without a gesture, SwiftUI answers with a view (a platform view, or the host); that is UIKit's case.
        guard let hit = host.perform(hitTestSelector, with: context)?.takeUnretainedValue() as? NSObject,
              !(hit is UIView) else { return .noGesture }
        return .gesture(hit)
    }

    /// The gesture's SwiftUI name from the container's description, such as `TapGesture`; a Button's is `Button`.
    private static func gestureName(_ container: NSObject) -> String {
        let description = container.description
        if description.contains("_ButtonGesture") { return "Button" }
        let names = ["SpatialTapGesture", "TapGesture", "LongPressGesture", "DragGesture", "MagnifyGesture", "RotateGesture"]
        return names.first { description.contains($0) } ?? "a SwiftUI gesture"
    }

    private static let containerSelector = NSSelectorFromString("accessibilityContainer")

    /// The view an element is or lives in: a view element itself, or the view holding an accessibility element.
    private static func ownView(of object: NSObject) -> UIView? {
        sequence(first: object) { element in
            // SwiftUI's nodes answer the selector without declaring it, like UIAccessibilityElement does.
            element.responds(to: containerSelector) ? element.value(forKey: "accessibilityContainer") as? NSObject : nil
        }.lazy.compactMap { $0 as? UIView }.first
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
        // Unknown never gets here: a center the engine cannot decide counts as reaching.
        case .nothing, .unknown: return ("untouchable", nil)
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
/// SwiftUI's responders (private): the nodes its hit test walks. A gesture container keeps the responder that won; an
/// accessibility node keeps, in its geometry, a weak AttributeGraph attribute holding the responders of its view.
/// Read with `Mirror` and `AGGraphGetWeakValue`, the call SwiftUI's `WeakAttribute.value` makes. Empty or nil when a
/// SwiftUI version stores them differently; the caller then does not know.
enum SwiftUIResponders {
    static func winner(of container: NSObject) -> AnyObject? {
        guard let ivar = class_getInstanceVariable(object_getClass(container), "base"),
              let pointer = UnsafeRawPointer(Unmanaged.passUnretained(container).toOpaque())
                .advanced(by: ivar_getOffset(ivar)).load(as: UnsafeRawPointer?.self),
              let responder = unwrap(child(Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue(), "responder"))
        else {
            Hooks.failed("swiftui.gestureWinner", "no base.responder on the gesture container")
            return nil
        }
        Hooks.ok("swiftui.gestureWinner")
        return responder as AnyObject
    }

    static func selfAndParents(_ responder: AnyObject) -> [AnyObject] {
        Array(sequence(first: responder) { unwrap(child($0, "parent")).map { $0 as AnyObject } }.prefix(200))
    }

    private typealias GetWeakValue = @convention(c) (UInt64, UInt32, UnsafeRawPointer) -> UnsafeRawPointer?
    private static let getWeakValue: GetWeakValue? = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "AGGraphGetWeakValue")
        .map { unsafeBitCast($0, to: GetWeakValue.self) }

    /// The responders an accessibility node's geometry points at.
    static func of(_ node: NSObject) -> [AnyObject] {
        guard let getWeakValue, let storage = child(node, "attachmentsStorage") else {
            Hooks.failed("swiftui.responders", "no AGGraphGetWeakValue or attachmentsStorage")
            return []
        }
        var layoutChanged: String?
        let found = Mirror(reflecting: storage).children.flatMap { entry -> [AnyObject] in
            guard let geometry = unwrap(child(entry.value, "geometry")), let weak = unwrap(child(geometry, "responders")),
                  let details = child(weak, "base").flatMap({ child($0, "_details") }),
                  let seed = child(details, "seed") as? UInt32,
                  let identifier = child(details, "identifier").flatMap({ Mirror(reflecting: $0).children.first?.value as? UInt32 }),
                  let valueType = valueType(of: weak, else: &layoutChanged)
            else { return [] }
            guard let pointer = getWeakValue(UInt64(identifier) | UInt64(seed) << 32, 0, valueType) else { return [] }
            func load<Value>(_: Value.Type) -> Any { pointer.load(as: Value.self) }
            let value = _openExistential(unsafeBitCast(valueType, to: Any.Type.self), do: load)
            return Mirror(reflecting: value).children.compactMap { $0.value as? [AnyObject] }.flatMap { $0 }
        }
        if let layoutChanged {
            Hooks.failed("swiftui.responders", layoutChanged)
        } else if found.isEmpty {
            Hooks.failed("swiftui.responders", "a control's node held no responders")
        } else {
            Hooks.ok("swiftui.responders")
        }
        return found
    }

    /// `Value` of a `WeakAttribute<Value>`: word 2 of a generic struct's metadata is its first generic argument (Swift's
    /// stable ABI). Taken only while the attribute is still a struct with one generic argument and word 2 names that
    /// argument; on any other layout reading it could crash the app, so the hook fails with `why` instead.
    private static func valueType(of weak: Any, else why: inout String?) -> UnsafeRawPointer? {
        let attribute = type(of: weak) as Any.Type
        let name = String(reflecting: attribute)
        let words = unsafeBitCast(attribute, to: UnsafePointer<UnsafeRawPointer>.self)
        let structKind: UInt = 0x200
        guard UInt(bitPattern: words[0]) == structKind, name.hasSuffix(">"), let open = name.firstIndex(of: "<"),
              name[..<open].hasSuffix(".WeakAttribute") else {
            why = "the responders are no longer a WeakAttribute<Value> struct: \(name)"
            return nil
        }
        let argument = name[name.index(after: open)..<name.index(before: name.endIndex)]
        guard !argument.contains(","), String(reflecting: unsafeBitCast(words[2], to: Any.Type.self)) == argument else {
            why = "WeakAttribute's metadata does not hold its Value where expected: \(name)"
            return nil
        }
        return words[2]
    }

    private static func child(_ value: Any, _ label: String) -> Any? {
        sequence(first: Mirror(reflecting: value), next: \.superclassMirror)
            .lazy.compactMap { $0.children.first { $0.label == label }?.value }.first
    }

    private static func unwrap(_ value: Any?) -> Any? {
        guard let value else { return nil }
        let mirror = Mirror(reflecting: value)
        guard mirror.displayStyle == .optional else { return value }
        return mirror.children.first.flatMap { unwrap($0.value) }
    }
}
/// The radius UIKit gives SwiftUI's hit test for a touch (`_UIHitTestContext contextWithPoint:radius:event:`, private).
/// It is UIKit's own value, read from its call the first time the app gets a touch; before that, 20 pt, what iOS 27
/// passes for every touch whatever the finger's size. The method is wrapped once and calls through unchanged.
enum TouchRadius {
    private(set) static var points: CGFloat = 20
    private(set) static var seen = false

    /// Main thread only.
    static func watch() {
        guard !installed else { return }
        guard let type = NSClassFromString("_UIHitTestContext"), let method = class_getClassMethod(type, selector) else {
            return Hooks.failed("uikit.touchRadius", "no contextWithPoint:radius:event:")
        }
        installed = true
        Hooks.ok("uikit.touchRadius")
        typealias Make = @convention(c) (AnyClass, Selector, CGPoint, CGFloat, AnyObject?) -> AnyObject?
        let original = unsafeBitCast(method_getImplementation(method), to: Make.self)
        let wrapped: @convention(block) (AnyClass, CGPoint, CGFloat, AnyObject?) -> AnyObject? = { type, point, radius, event in
            if event != nil { points = radius; seen = true }
            return original(type, selector, point, radius, event)
        }
        method_setImplementation(method, imp_implementationWithBlock(wrapped))
    }

    private static var installed = false
    private static let selector = NSSelectorFromString("contextWithPoint:radius:event:")
}
#endif
