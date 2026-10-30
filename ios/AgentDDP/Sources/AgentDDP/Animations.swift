#if DEBUG && canImport(UIKit)
import UIKit

/// `screen.animations`: the Core Animation animations on the app's layers, as `animationKeys` lists them. SwiftUI
/// animates most changes by drawing each frame itself, so those are not here; `ddp record` shows them.
enum Animations {
    static func list() -> [String: Any] {
        onMain {
            var found: [[String: Any]] = []
            for window in FrameCapture.visibleWindows() {
                collect(window.layer, in: window, into: &found)
            }
            return ["animations": found]
        }
    }

    private static func collect(_ layer: CALayer, in window: UIWindow, into found: inout [[String: Any]]) {
        for key in layer.animationKeys() ?? [] {
            guard let animation = layer.animation(forKey: key) else { continue }
            let frame = layer.convert(layer.bounds, to: window.layer)
            var entry: [String: Any] = ["key": key, "type": String(describing: type(of: animation)),
                                        "duration": round2(animation.duration), "frame": rectArray(frame)]
            if let keyPath = (animation as? CAPropertyAnimation)?.keyPath { entry["keyPath"] = keyPath }
            // UIKit's own spinners repeat 2^31 times, not .infinity.
            if animation.repeatCount > 0 { entry["repeat"] = animation.repeatCount >= Float(Int32.max) ? "forever" : animation.repeatCount }
            if layer.speed != 1 { entry["speed"] = layer.speed }
            if let id = (layer.delegate as? UIView)?.accessibilityIdentifier, !id.isEmpty { entry["id"] = id }
            found.append(entry)
        }
        layer.sublayers?.forEach { collect($0, in: window, into: &found) }
    }
}
#endif
