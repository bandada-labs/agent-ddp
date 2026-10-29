#if DEBUG && canImport(UIKit)
import UIKit

/// `screen.highlight`: a box over the live app for a while, for a person watching the phone. It sits in a window of
/// its own that takes no touches, and ddp's own reads leave that window out.
final class HighlightWindow: UIWindow {
    private static var shown: HighlightWindow?

    static func show(_ params: [String: Any]) throws -> [String: Any] {
        let numbers = (params["rect"] as? [NSNumber])?.map(\.doubleValue) ?? []
        guard numbers.count == 4 else { throw Control.Failure(description: "rect is [x, y, width, height] in points") }
        let ms = min(max((params["ms"] as? NSNumber)?.doubleValue ?? 3000, 100), 60_000)
        onMain {
            guard let scene = FrameCapture.visibleWindows().first?.windowScene else { return }
            shown?.isHidden = true
            let window = HighlightWindow(windowScene: scene)
            window.windowLevel = .alert + 1
            window.isUserInteractionEnabled = false
            window.backgroundColor = .clear
            let box = UIView(frame: CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3]))
            box.layer.borderColor = UIColor.systemRed.cgColor
            box.layer.borderWidth = 3
            box.backgroundColor = UIColor.systemRed.withAlphaComponent(0.15)
            window.addSubview(box)
            window.isHidden = false
            shown = window
            DispatchQueue.main.asyncAfter(deadline: .now() + ms / 1000) { [weak window] in
                window?.isHidden = true
                if shown === window { shown = nil }
            }
        }
        return [:]
    }
}
#endif
