#if DEBUG && canImport(UIKit)
import UIKit

/// What the app's own windows show, from the render server (`drawHierarchy`), for the checks that judge what is on
/// screen. Only the app's windows: a system alert or notification over the app is not the app's to answer for.
///
/// Texts: a text layer's own bitmap gives its shape (alpha) but not always its color on screen (SwiftUI keeps an
/// alpha-only bitmap and the render server colors it). So per text, the screen's colors are sampled where the text's
/// own alpha says ink (glyph cores) and where it says clear: what the screen shows there, whatever made it so.
enum Pixels {
    /// Main thread only. The pixels of each rectangle (screen points), RGBA in sRGB, at the screen's scale, and per
    /// text layer the screen's colors at its ink and clear points.
    static func answer(_ rects: [CGRect]) -> [String: Any] {
        let scale = UIScreen.main.scale
        guard let screen = composite(scale: scale) else {
            return ["scale": Double(scale), "regions": rects.map { ["rect": rectArray($0), "missing": "drawHierarchy drew nothing"] }]
        }
        return ["scale": Double(scale), "regions": rects.map { region($0, of: screen, scale: scale) },
                "texts": LayerReader.textLayers(in: FrameCapture.visibleWindows()).compactMap { ink(of: $0, on: screen, scale: scale) }]
    }

    private struct Screen {
        let bytes: [UInt8]
        let width: Int
        let height: Int

        func color(_ x: Int, _ y: Int) -> String? {
            guard x >= 0, y >= 0, x < width, y < height else { return nil }
            let at = (y * width + x) * 4
            guard bytes[at + 3] == 255 else { return nil }
            return String(format: "#%02X%02X%02XFF", bytes[at], bytes[at + 1], bytes[at + 2])
        }
    }

    private static func composite(scale: CGFloat) -> Screen? {
        let bounds = UIScreen.main.bounds
        let width = Int(bounds.width * scale), height = Int(bounds.height * scale)
        var screen = [UInt8](repeating: 0, count: width * height * 4)
        let drawn: Bool = screen.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: width * 4, space: sRGB,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.translateBy(x: 0, y: CGFloat(height))
            context.scaleBy(x: scale, y: -scale)
            UIGraphicsPushContext(context)
            defer { UIGraphicsPopContext() }
            // Bottom window first; each one composites over what is under it, as on screen.
            return FrameCapture.visibleWindows().allSatisfy { window in
                window.drawHierarchy(in: window.frame, afterScreenUpdates: false)
            }
        }
        return drawn ? Screen(bytes: screen, width: width, height: height) : nil
    }

    private static func region(_ rect: CGRect, of screen: Screen, scale: CGFloat) -> [String: Any] {
        let left = max(0, Int((rect.minX * scale).rounded(.down))), top = max(0, Int((rect.minY * scale).rounded(.down)))
        let right = min(screen.width, Int((rect.maxX * scale).rounded(.up)))
        let bottom = min(screen.height, Int((rect.maxY * scale).rounded(.up)))
        guard right > left, bottom > top else { return ["rect": rectArray(rect), "missing": "off the screen"] }
        var bytes = Data(capacity: (right - left) * (bottom - top) * 4)
        for row in top..<bottom {
            let start = (row * screen.width + left) * 4
            bytes.append(contentsOf: screen.bytes[start..<start + (right - left) * 4])
        }
        return ["rect": rectArray(rect), "x": left, "y": top, "width": right - left, "height": bottom - top,
                "rgba": bytes.base64EncodedString()]
    }

    /// Up to this many ink and clear points per text.
    private static let samples = 64

    /// The screen's colors where the layer's own alpha is ink (a pixel and its eight neighbors near the layer's
    /// strongest alpha, so a dimmed text counts too) and clear (nothing within two pixels), with the layer's frame to
    /// match it to its node.
    private static func ink(of layer: (layer: CALayer, frame: CGRect), on screen: Screen, scale: CGFloat) -> [String: Any]? {
        let origin = CGPoint(x: layer.frame.minX * scale, y: layer.frame.minY * scale)
        let (left, top) = (Int(origin.x.rounded(.down)), Int(origin.y.rounded(.down)))
        let width = Int((layer.frame.maxX * scale).rounded(.up)) - left, height = Int((layer.frame.maxY * scale).rounded(.up)) - top
        guard width > 4, height > 4, width * height < 4_000_000 else { return nil }
        var alpha = [UInt8](repeating: 0, count: width * height)
        let rendered: Bool = alpha.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: width, space: CGColorSpaceCreateDeviceGray(),
                                          bitmapInfo: CGImageAlphaInfo.alphaOnly.rawValue) else { return false }
            // The layer drawn where it sits on screen, its fraction of a pixel included, so its pixels line up.
            context.translateBy(x: origin.x - CGFloat(left), y: CGFloat(height) - (origin.y - CGFloat(top)))
            context.scaleBy(x: scale, y: -scale)
            layer.layer.render(in: context)
            return true
        }
        guard rendered, let strongest = alpha.max(), strongest >= 64 else { return nil }
        let inked = Int(strongest) - 13
        func value(_ x: Int, _ y: Int) -> Int { x < 0 || y < 0 || x >= width || y >= height ? 0 : Int(alpha[y * width + x]) }
        var cores: [(Int, Int)] = [], clear: [(Int, Int)] = []
        for y in 0..<height {
            for x in 0..<width {
                if (-1...1).allSatisfy({ dy in (-1...1).allSatisfy { dx in value(x + dx, y + dy) >= inked } }) {
                    cores.append((x, y))
                } else if (-2...2).allSatisfy({ dy in (-2...2).allSatisfy { dx in value(x + dx, y + dy) == 0 } }),
                          x >= 2, y >= 2, x < width - 2, y < height - 2 {
                    clear.append((x, y))
                }
            }
        }
        func colors(_ points: [(Int, Int)]) -> [String] {
            let step = max(1, points.count / samples)
            return stride(from: 0, to: points.count, by: step).compactMap { screen.color(left + points[$0].0, top + points[$0].1) }
        }
        guard !cores.isEmpty else { return nil }
        return ["frame": rectArray(layer.frame), "ink": colors(cores), "clear": colors(clear)]
    }
}
#endif
