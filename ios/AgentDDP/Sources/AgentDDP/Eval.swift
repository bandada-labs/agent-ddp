#if DEBUG && canImport(UIKit)
import Foundation
import UIKit

/// Property reads for `screen.value` (`ddp eval <element> <path>`):
/// `element.<key path>` on the accessibility element, `layer.<key path>` on the topmost layer that
/// matches the element's frame (presentation values, so mid-animation).
enum LiveEval {
    static func evaluate(name: String, path: String) -> [String: Any] {
        switch target(name: name, path: path) {
        case .failure(let message): return error(message.text)
        case .success(let (object, keyPath)):
            return value(of: (object as? CALayer)?.presentation() ?? object, keyPath: keyPath)
        }
    }

    /// `screen.setValue`: a number, Bool or "#RRGGBB(AA)" color set with KVC on what `evaluate` reads, then read back,
    /// so the answer is UIKit's. A layer gets its model value: the presentation layer is a copy. SwiftUI owns the
    /// layers it draws and can put its own value back on its next update.
    static func set(name: String, path: String, to given: Any) -> [String: Any] {
        guard case .success(let (object, keyPath)) = target(name: name, path: path) else {
            return evaluate(name: name, path: path)
        }
        var keys = keyPath.split(separator: ".").map(String.init)
        let last = keys.removeLast()
        var owner: NSObject = object
        for key in keys {
            guard owner is CALayer || owner.responds(to: NSSelectorFromString(key)),
                  let next = owner.value(forKey: key) as? NSObject else {
                return error("'\(key)' is not a property of \(type(of: owner))")
            }
            owner = next
        }
        // Without a setter, KVC throws an Objective-C exception Swift cannot catch.
        let setter = "set" + last.prefix(1).uppercased() + last.dropFirst() + ":"
        guard owner.responds(to: NSSelectorFromString(setter)) else {
            return error("'\(last)' of \(type(of: owner)) cannot be set")
        }
        let value: Any
        if let text = given as? String {
            guard let color = color(text) else { return error("a value is a number, true or false, or #RRGGBB(AA), not '\(text)'") }
            value = owner is CALayer ? color.cgColor : color
        } else if let number = given as? NSNumber {
            value = number
        } else {
            return error("a value is a number, true or false, or #RRGGBB(AA)")
        }
        owner.setValue(value, forKey: last)
        return LiveEval.value(of: object, keyPath: keyPath)
    }

    private struct Message: Error { let text: String }

    /// The accessibility element or the topmost layer with its frame, and the key path left to read on it.
    private static func target(name: String, path: String) -> Result<(NSObject, String), Message> {
        let parts = path.split(separator: ".", maxSplits: 1).map(String.init)
        guard parts.count == 2, ["element", "layer"].contains(parts[0]) else {
            return .failure(Message(text: "path is element.<key path> or layer.<key path>, e.g. layer.shadowRadius"))
        }
        let windows = FrameCapture.visibleWindows()
        guard let element = AccessibilityReader.object(named: name, in: windows) else {
            return .failure(Message(text: "no element '\(name)'"))
        }
        if parts[0] == "element" { return .success((element, parts[1])) }
        guard let layer = LayerReader.topLayer(matching: element.accessibilityFrame, in: windows) else {
            return .failure(Message(text: "no layer matches the frame of '\(name)'"))
        }
        return .success((layer, parts[1]))
    }

    private static func color(_ text: String) -> UIColor? {
        let digits = text.hasPrefix("#") ? String(text.dropFirst()) : ""
        guard [6, 8].contains(digits.count), let packed = UInt64(digits, radix: 16) else { return nil }
        let rgba = digits.count == 6 ? packed << 8 | 0xFF : packed
        let channel = { (shift: UInt64) in CGFloat((rgba >> shift) & 0xFF) / 255 }
        return UIColor(red: channel(24), green: channel(16), blue: channel(8), alpha: channel(0))
    }

    private static func value(of object: NSObject, keyPath: String) -> [String: Any] {
        var current: Any? = object
        for key in keyPath.split(separator: ".").map(String.init) {
            // KVC on an unknown key throws an Objective-C exception Swift can't catch, so ask first.
            // CALayer answers any key (nil when unset).
            guard let target = current as? NSObject,
                  target is CALayer || target.responds(to: NSSelectorFromString(key)) else {
                return error("'\(key)' is not a property of \(type(of: current ?? "nil"))")
            }
            current = target.value(forKey: key)
        }
        return ["type": "value", "path": keyPath, "value": jsonValue(current)]
    }

    private static func jsonValue(_ value: Any?) -> Any {
        guard let value else { return NSNull() }
        if let number = value as? NSNumber { return number }
        if let string = value as? String { return string }
        if CFGetTypeID(value as CFTypeRef) == CGColor.typeID { return hex((value as! CGColor)) ?? "transparent" }
        return String(describing: value)
    }

    private static func error(_ message: String) -> [String: Any] { ["type": "error", "error": message] }
}
#endif
