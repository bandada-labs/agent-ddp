#if DEBUG && canImport(UIKit)
import Foundation
import UIKit

/// Read-only property reads for `screen.value` (`ddp eval <element> <path>`):
/// `element.<key path>` on the accessibility element, `layer.<key path>` on the topmost layer that
/// matches the element's frame (presentation values, so mid-animation).
enum LiveEval {
    static func evaluate(name: String, path: String) -> [String: Any] {
        let parts = path.split(separator: ".", maxSplits: 1).map(String.init)
        guard parts.count == 2, ["element", "layer"].contains(parts[0]) else {
            return error("path is element.<key path> or layer.<key path>, e.g. layer.shadowRadius")
        }
        let windows = FrameCapture.visibleWindows()
        guard let element = AccessibilityReader.object(named: name, in: windows) else {
            return error("no element '\(name)'")
        }
        if parts[0] == "element" { return value(of: element, keyPath: parts[1]) }
        guard let layer = LayerReader.topLayer(matching: element.accessibilityFrame, in: windows) else {
            return error("no layer matches the frame of '\(name)'")
        }
        return value(of: layer.presentation() ?? layer, keyPath: parts[1])
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
