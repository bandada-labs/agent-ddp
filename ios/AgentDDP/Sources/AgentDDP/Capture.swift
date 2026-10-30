#if DEBUG && canImport(UIKit)
import QuartzCore
import UIKit

enum AccessibilityReader {
    private static let accessibilityIdentifierSelector = NSSelectorFromString("accessibilityIdentifier")

    /// SwiftUI only builds its accessibility tree when an assistive client or XCTest is
    /// attached. This flips the same private switch XCTest uses. Debug builds only.
    static func enableAutomationTree() {
        typealias SetEnabled = @convention(c) (Bool) -> Void
        guard let library = dlopen("/usr/lib/libAccessibility.dylib", RTLD_NOW),
              let symbol = dlsym(library, "_AXSSetAutomationEnabled") else {
            return Hooks.failed("accessibility.automation", "no _AXSSetAutomationEnabled")
        }
        unsafeBitCast(symbol, to: SetEnabled.self)(true)
        Hooks.ok("accessibility.automation")
    }

    static func elements(in window: UIWindow) -> [[String: Any]] {
        var found: [[String: Any]] = []
        visitElements(window) { found.append(element(for: $0)) }
        return found
    }

    /// Every element with its accessibility object, for checks that need the object (`screen.hit`).
    static func objects(in windows: [UIWindow]) -> [(object: NSObject, element: [String: Any])] {
        var found: [(NSObject, [String: Any])] = []
        windows.forEach { visitElements($0) { found.append(($0, element(for: $0))) } }
        return found
    }

    private static func element(for object: NSObject) -> [String: Any] {
        var element: [String: Any] = ["frame": rectArray(object.accessibilityFrame)]
        if let id = identifier(of: object) { element["id"] = id }
        if let label = object.accessibilityLabel, !label.isEmpty { element["label"] = label }
        if !object.accessibilityTraits.intersection([.button, .link, .adjustable]).isEmpty { element["tap"] = true }
        if object.accessibilityTraits.contains(.notEnabled) { element["disabled"] = true }
        element["role"] = role(of: object)
        return element
    }

    /// The accessibility element named by id or label, for `ddp eval`.
    static func object(named name: String, in windows: [UIWindow]) -> NSObject? {
        var match: NSObject?
        windows.forEach { window in
            visitElements(window) { object in
                if match == nil, identifier(of: object) == name || object.accessibilityLabel == name { match = object }
            }
        }
        return match
    }

    // Not public API: the trait XCTest reads to call an element a text field (SwiftUI TextField has no class to check).
    private static let textEntryTrait = UIAccessibilityTraits(rawValue: 1 << 18)

    /// The ARIA role a browser would give it, so agents read the same words on both platforms.
    static func role(of object: NSObject) -> String {
        let traits = object.accessibilityTraits
        if object is UITextField || object is UITextView || traits.contains(textEntryTrait) { return "textbox" }
        if traits.contains(.searchField) { return "searchbox" }
        if #available(iOS 17, *), traits.contains(.toggleButton) { return "switch" }
        if traits.contains(.adjustable) { return "slider" }
        if traits.contains(.link) { return "link" }
        if traits.contains(.button) { return "button" }
        if traits.contains(.header) { return "heading" }
        if traits.contains(.image) { return "image" }
        if traits.contains(.staticText) { return "StaticText" }
        return "generic"
    }

    static func identifier(of object: NSObject) -> String? {
        // SwiftUI's element objects answer the selector without declaring the protocol.
        guard object.responds(to: accessibilityIdentifierSelector),
              let id = object.value(forKey: "accessibilityIdentifier") as? String, !id.isEmpty else { return nil }
        return id
    }

    private static func visitElements(_ object: NSObject, _ visit: (NSObject) -> Void) {
        if object.isAccessibilityElement { return visit(object) }
        if let children = object.accessibilityElements as? [NSObject] {
            children.forEach { visitElements($0, visit) }
        } else if let view = object as? UIView {
            view.subviews.forEach { visitElements($0, visit) }
        }
    }
}

enum LayerReader {
    /// The topmost layer whose frame matches `rect` (window coordinates, 4 pt tolerance), for `ddp eval`.
    static func topLayer(matching rect: CGRect, in windows: [UIWindow]) -> CALayer? {
        var match: CALayer?
        func visit(_ layer: CALayer, root: CALayer, offset: CGPoint) {
            let shown = layer.presentation() ?? layer
            let frame = shown.convert(shown.bounds, to: root).offsetBy(dx: offset.x, dy: offset.y)
            if abs(frame.minX - rect.minX) <= 4, abs(frame.minY - rect.minY) <= 4,
               abs(frame.width - rect.width) <= 8, abs(frame.height - rect.height) <= 8 { match = layer }
            layer.sublayers?.forEach { visit($0, root: root, offset: offset) }
        }
        windows.forEach { visit($0.layer, root: $0.layer.presentation() ?? $0.layer, offset: $0.frame.origin) }
        return match
    }

    /// Layers in paint order (later covers earlier). Only layers that draw something are kept.
    static func paintedNodes(in windows: [UIWindow]) -> [[String: Any]] {
        var texts: [(layer: CALayer, frame: CGRect)] = []
        return walkWindows(windows, texts: &texts)
    }

    /// The layers that draw a text, with their frames on screen, as the nodes carry them.
    static func textLayers(in windows: [UIWindow]) -> [(layer: CALayer, frame: CGRect)] {
        var texts: [(layer: CALayer, frame: CGRect)] = []
        _ = walkWindows(windows, texts: &texts)
        return texts
    }

    private static func walkWindows(_ windows: [UIWindow], texts: inout [(layer: CALayer, frame: CGRect)]) -> [[String: Any]] {
        var nodes: [[String: Any]] = []
        for window in windows {
            let root = window.layer.presentation() ?? window.layer
            let windowClip = CGRect(origin: .zero, size: window.bounds.size)
            walk(window.layer, root: root, clip: windowClip, opacity: 1, offset: window.frame.origin,
                 into: &nodes, texts: &texts)
        }
        return nodes
    }

    // ponytail: paint order is sublayer order, re-sorted by zPosition; 3D transforms are ignored.
    private static func walk(_ layer: CALayer, root: CALayer, clip: CGRect, opacity: Float,
                             offset: CGPoint, effects: Bool = false, into nodes: inout [[String: Any]],
                             texts: inout [(layer: CALayer, frame: CGRect)]) {
        let shown = layer.presentation() ?? layer
        let effectiveOpacity = opacity * shown.opacity
        // A filter, a compositing filter (vibrancy, blend modes) or a mask changes what the layer's pixels look like on
        // screen: what it draws is then not what shows.
        let effects = effects || shown.compositingFilter != nil || !(shown.filters ?? []).isEmpty
            || !(shown.backgroundFilters ?? []).isEmpty || shown.mask != nil
        guard !shown.isHidden, effectiveOpacity > 0.001 else { return }
        let frame = shown.convert(shown.bounds, to: root).offsetBy(dx: offset.x, dy: offset.y)
        let visible = frame.intersection(clip)

        if !visible.isEmpty, var node = paintDescription(of: layer, shown: shown) {
            node["z"] = nodes.count
            node["frame"] = rectArray(frame)
            if visible != frame { node["visible"] = rectArray(visible) }
            if effectiveOpacity < 0.999 { node["opacity"] = round2(Double(effectiveOpacity)) }
            if shown.masksToBounds { node["clips"] = true }
            if effects { node["effects"] = true }
            // UIKit view classes help name a coverer; SwiftUI's mangled internal classes do not.
            if let view = layer.delegate as? UIView, !NSStringFromClass(type(of: view)).hasPrefix("_Tt") {
                node["source"] = NSStringFromClass(type(of: view))
            }
            if let text = textStyle(of: layer) {
                node["text"] = text
                texts.append((layer, frame))
            }
            nodes.append(node)
            if node["kind"] as? String == "opaque-drawing" {
                appendDraws(of: layer, shown: shown, root: root, offset: offset, clip: clip, base: node, into: &nodes)
            }
        }

        let childClip = shown.masksToBounds ? clip.intersection(frame) : clip
        guard !childClip.isEmpty else { return }
        // A stable sort keeps sublayer order among equal zPosition, which is how Core Animation draws them.
        let ordered = (layer.sublayers ?? []).enumerated()
            .sorted { ($0.element.zPosition, $0.offset) < ($1.element.zPosition, $1.offset) }.map(\.element)
        ordered.forEach {
            walk($0, root: root, clip: childClip, opacity: effectiveOpacity, offset: offset, effects: effects, into: &nodes,
                 texts: &texts)
        }
    }

    /// One `draw` node per shape a SwiftUI `Canvas` or `drawingGroup` drew, after its `opaque-drawing` node.
    private static func appendDraws(of layer: CALayer, shown: CALayer, root: CALayer, offset: CGPoint, clip: CGRect,
                                    base: [String: Any], into nodes: inout [[String: Any]]) {
        guard let read = CanvasDraws.read(layer) else { return }
        if read.unread > 0 { nodes[nodes.count - 1]["unread"] = read.unread }
        for draw in read.draws {
            let frame = shown.convert(draw.bounds, to: root).offsetBy(dx: offset.x, dy: offset.y)
            let visible = frame.intersection(clip)
            guard !visible.isEmpty else { continue }
            var node: [String: Any] = ["z": nodes.count, "kind": "draw", "op": draw.op, "frame": rectArray(frame)]
            if visible != frame { node["visible"] = rectArray(visible) }
            if let fill = draw.fill { node["fill"] = fill }
            if let border = draw.border { node["border"] = ["width": border.width, "color": border.color] }
            if draw.shader { node["shader"] = true }
            for key in ["opacity", "effects"] { node[key] = base[key] }
            nodes.append(node)
        }
    }

    /// Size and color of the text a layer draws: from UILabel / UITextField when there is one, otherwise
    /// (SwiftUI `Text`) the main color of its pixels, every frame. SwiftUI redraws a text's bitmap in place,
    /// so there is no cheap sign that its color changed.
    private static func textStyle(of layer: CALayer) -> [String: Any]? {
        if let label = layer.delegate as? UILabel, let font = label.font {
            var style: [String: Any] = ["size": round2(font.pointSize), "color": hex(label.textColor.cgColor) ?? "transparent",
                                        "weight": weight(of: font)]
            // An attributed text can draw parts in other colors.
            var colors: [String] = []
            if let text = label.attributedText {
                text.enumerateAttribute(.foregroundColor, in: NSRange(location: 0, length: text.length)) { value, _, _ in
                    if let color = (value as? UIColor).flatMap({ hex($0.cgColor) }), !colors.contains(color) { colors.append(color) }
                }
            }
            if colors.count > 1 || (colors.first.map { $0 != style["color"] as? String } ?? false) { style["colors"] = colors }
            return style
        }
        if let field = layer.delegate as? UITextField, let font = field.font {
            return ["size": round2(font.pointSize), "color": hex(field.textColor?.cgColor) ?? "transparent",
                    "weight": weight(of: font)]
        }
        guard NSStringFromClass(type(of: layer)).contains("CGDrawingLayer"), layer.contents != nil,
              let pixels = PixelSampler.sample(layer, maxSide: PixelSampler.textSide),
              let main = (pixels["colors"] as? [[String: Any]])?.first?["color"] as? String else { return nil }
        var style: [String: Any] = ["color": main, "size": round2(layer.bounds.height / 1.2), "estimated": true]
        // A text that draws in more than one color (a concatenated Text) shows each as a share of its pixels.
        let colors = (pixels["colors"] as? [[String: Any]] ?? [])
            .filter { ($0["share"] as? Double ?? 0) >= 0.15 }.compactMap { $0["color"] as? String }
        if colors.count > 1 { style["colors"] = colors }
        // The bitmap's line height is about 1.2 times the font size: an estimate, so checks treat the size as unknown.
        return style
    }

    private static func weight(of font: UIFont) -> Double {
        let traits = font.fontDescriptor.object(forKey: .traits) as? [UIFontDescriptor.TraitKey: Any]
        let value = traits?[.weight] as? CGFloat ?? 0
        // UIFont.Weight to the CSS scale (regular 400, bold 700) by the nearest named weight.
        let named: [(UIFont.Weight, Double)] = [(.ultraLight, 100), (.thin, 200), (.light, 300), (.regular, 400),
                                                (.medium, 500), (.semibold, 600), (.bold, 700), (.heavy, 800), (.black, 900)]
        return named.min { abs($0.0.rawValue - value) < abs($1.0.rawValue - value) }!.1
    }

    private static func paintDescription(of layer: CALayer, shown: CALayer) -> [String: Any]? {
        var node: [String: Any] = [:]
        if shown.cornerRadius > 0 {
            node["corner"] = round2(shown.cornerRadius)
            // maskedCorners leaves some corners square: top-left, top-right, bottom-right, bottom-left.
            let masks: [CACornerMask] = [.layerMinXMinYCorner, .layerMaxXMinYCorner, .layerMaxXMaxYCorner, .layerMinXMaxYCorner]
            if shown.maskedCorners != CACornerMask(masks) {
                node["corners"] = masks.map { shown.maskedCorners.contains($0) ? round2(shown.cornerRadius) : 0 }
            }
        }

        if let shape = shown as? CAShapeLayer {
            node["kind"] = "shape"
            node["fill"] = hex(shape.fillColor)
            if let stroke = hex(shape.strokeColor), shape.lineWidth > 0 {
                node["border"] = ["width": round2(shape.lineWidth), "color": stroke]
            }
            return node["fill"] == nil && node["border"] == nil ? nil : node
        }
        if let gradient = shown as? CAGradientLayer, let colors = gradient.colors as? [CGColor] {
            node["kind"] = "gradient"
            node["gradient"] = colors.compactMap(hex)
            return node
        }
        if NSStringFromClass(type(of: layer)).contains("RBDrawingLayer") {
            // Canvas, drawingGroup and other Metal-backed content: one bitmap. Its shapes are gone,
            // so read what it shows from its pixels.
            node["kind"] = "opaque-drawing"
            if let pixels = PixelSampler.sample(layer) { node["pixels"] = pixels }
            return node
        }

        node["kind"] = shown.contents == nil ? "layer" : "bitmap"
        if let fill = hex(shown.backgroundColor) { node["fill"] = fill }
        if shown.borderWidth > 0, let border = hex(shown.borderColor) {
            node["border"] = ["width": round2(shown.borderWidth), "color": border]
        }
        // SwiftUI's .shadow (a glow) is a layer shadow.
        if shown.shadowOpacity > 0, shown.shadowRadius > 0, let shadow = hex(shown.shadowColor) {
            node["shadow"] = ["radius": round2(shown.shadowRadius), "opacity": round2(Double(shown.shadowOpacity)),
                              "color": shadow]
        }
        let paintsSomething = shown.contents != nil || node["fill"] != nil || node["border"] != nil
            || node["shadow"] != nil
        return paintsSomething ? node : nil
    }
}

enum PixelSampler {
    // ponytail: at most 64 px a side keeps a sample near 1 ms; a hairline thinner than one sample pixel
    // reads fainter than it is. Raise it if an agent needs exact line colors.
    private static let maxSide: CGFloat = 64
    // ponytail: text only needs its main color; 24 px a side keeps it under 0.1 ms a label.
    static let textSide: CGFloat = 24
    private static let visibleAlpha = 13  // about 5 %

    /// The colors a drawing shows (top 3 with their share of its painted pixels), its strongest alpha,
    /// and how much of its box is painted.
    static func sample(_ layer: CALayer, maxSide: CGFloat = maxSide) -> [String: Any]? {
        let size = layer.bounds.size
        guard size.width >= 1, size.height >= 1 else { return nil }
        let scale = min(1, maxSide / max(size.width, size.height))
        let width = Int((size.width * scale).rounded(.up)), height = Int((size.height * scale).rounded(.up))
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let rendered: Bool = pixels.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                          bytesPerRow: width * 4, space: sRGB,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.translateBy(x: 0, y: CGFloat(height))
            context.scaleBy(x: scale, y: -scale)
            layer.render(in: context)
            return true
        }
        guard rendered else { return nil }

        var buckets: [Int: (count: Int, red: Int, green: Int, blue: Int)] = [:]
        var painted = 0
        var peakAlpha = 0
        for offset in stride(from: 0, to: pixels.count, by: 4) {
            let alpha = Int(pixels[offset + 3])
            guard alpha >= visibleAlpha else { continue }
            painted += 1
            peakAlpha = max(peakAlpha, alpha)
            // Un-premultiply, then group by 4 bits per channel so anti-aliased edges join their color.
            let channels = (0..<3).map { min(255, Int(pixels[offset + $0]) * 255 / alpha) }
            let bucket = (channels[0] >> 4) << 8 | (channels[1] >> 4) << 4 | channels[2] >> 4
            let sum = buckets[bucket] ?? (0, 0, 0, 0)
            buckets[bucket] = (sum.count + 1, sum.red + channels[0], sum.green + channels[1], sum.blue + channels[2])
        }
        let colors = buckets.values.sorted { $0.count > $1.count }.prefix(3).map { sum -> [String: Any] in
            let rgb = [sum.red, sum.green, sum.blue].map { String(format: "%02X", $0 / sum.count) }
            return ["color": "#" + rgb.joined() + "FF", "share": round2(Double(sum.count) / Double(painted))]
        }
        return ["colors": colors, "peakAlpha": round2(Double(peakAlpha) / 255),
                "coverage": round2(Double(painted) / Double(width * height))]
    }
}

let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

/// "#RRGGBBAA" in sRGB, or nil when the color is missing or fully transparent.
func hex(_ color: CGColor?) -> String? {
    guard let color,
          let converted = color.converted(to: sRGB, intent: .defaultIntent, options: nil),
          let components = converted.components, components.count >= 4,
          components[3] > 0.001 else { return nil }
    let bytes = components.prefix(4).map { Int((min(max($0, 0), 1) * 255).rounded()) }
    return "#" + bytes.map { String(format: "%02X", $0) }.joined()
}

func rectArray(_ rect: CGRect) -> [Double] {
    [rect.origin.x, rect.origin.y, rect.width, rect.height].map { round2($0) }
}

func round2(_ value: Double) -> Double { (value * 100).rounded() / 100 }
func round2(_ value: CGFloat) -> Double { round2(Double(value)) }

#endif
