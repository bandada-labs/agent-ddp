#if DEBUG && canImport(UIKit)
import QuartzCore
import UIKit

/// The shapes inside a SwiftUI `Canvas` or `drawingGroup`, one `draw` node per call, like the Android draw replay.
/// SwiftUI hands those to RenderBox as a display list. RenderBox writes a display list out as XML only when its XML
/// recorder is on, so `start` turns that on for the app's run; a Canvas drawn before then reads at its next redraw.
enum CanvasDraws {
    private typealias Call = @convention(c) () -> Void
    private static let displayListClass = NSClassFromString("RBDisplayList") as? NSObject.Type
    private static let drawSelector = NSSelectorFromString("drawInDisplayList:")
    private static let recordSelector = NSSelectorFromString("beginRecordingXML")

    static func start() {
        let handle = UnsafeMutableRawPointer(bitPattern: -2)  // RTLD_DEFAULT
        guard displayListClass != nil, let install = dlsym(handle, "RBXMLRecorderInstall"),
              let begin = dlsym(handle, "RBXMLRecorderStart") else {
            return Hooks.failed("renderbox.displayList", "no RBDisplayList or RBXMLRecorderStart")
        }
        unsafeBitCast(install, to: Call.self)()
        unsafeBitCast(begin, to: Call.self)()
    }

    /// The draws of a drawing layer, in its own coordinates, and how many items in it were not read (text, images,
    /// nested layers). Nil when the layer gave no display list or held something that moves or clips its draws.
    static func read(_ layer: CALayer) -> (draws: [Draw], unread: Int)? {
        guard let displayListClass, layer.responds(to: drawSelector) else { return nil }
        let list = displayListClass.init()
        _ = list.perform(recordSelector)
        _ = layer.perform(drawSelector, with: list)
        guard let xml = list.value(forKey: "xmlDescription") as? String else {
            Hooks.failed("renderbox.displayList", "no xmlDescription")
            return nil
        }
        guard let read = DisplayListReader.read(xml) else {
            Hooks.failed("renderbox.displayList", "a transform or clip it does not know")
            return nil
        }
        // A Canvas recorded before the recorder was on draws, but its part of the XML is empty.
        // ponytail: only the all-empty case is caught; a list with some draws and an unrecorded part reads short.
        if read.draws.isEmpty, read.unread == 0, list.value(forKey: "empty") as? Bool == false { return nil }
        Hooks.ok("renderbox.displayList")
        return read
    }

    struct Draw: Equatable {
        let op: String
        let bounds: CGRect
        var fill: String?
        var border: (width: Double, color: String)?
        var shader = false

        static func == (left: Draw, right: Draw) -> Bool {
            left.op == right.op && left.bounds == right.bounds && left.fill == right.fill
                && left.border?.width == right.border?.width && left.border?.color == right.border?.color
                && left.shader == right.shader
        }
    }
}

/// Reads RenderBox's display list XML. Only what it knows is read: a transform or clip it does not know makes the
/// whole list unknown, because every draw after it could be somewhere else.
final class DisplayListReader: NSObject, XMLParserDelegate {
    private var draws: [CanvasDraws.Draw] = []
    private var unread = 0
    private var unknown = false
    // One frame per display list: its base transform and its states ("" is the current one, "#1" copies).
    private var lists: [(base: CGAffineTransform, states: [String: CGAffineTransform])] = []
    private var skippedDepth = 0  // inside an element whose content is not read

    static func read(_ xml: String) -> (draws: [CanvasDraws.Draw], unread: Int)? {
        let reader = DisplayListReader()
        let parser = XMLParser(data: Data(xml.utf8))
        parser.delegate = reader
        guard parser.parse(), !reader.unknown else { return nil }
        return (reader.draws, reader.unread)
    }

    func parser(_ parser: XMLParser, didStartElement name: String, namespaceURI: String?, qualifiedName: String?,
                attributes: [String: String]) {
        if skippedDepth > 0 { skippedDepth += 1; return }
        let state = attributes["state"] ?? ""
        switch name {
        case "display-list":
            guard let parent = lists.last.map({ list in list.states[state].map { $0.concatenating(list.base) } })
                ?? .identity else { return stop(parser) }
            lists.append((parent, ["": .identity]))
        case "device", "identity":
            break
        case "copy-state":
            guard let to = attributes["to"], let from = lists.last?.states[state] else { return stop(parser) }
            lists[lists.count - 1].states[to] = from
        case "transform":
            guard let current = lists.last?.states[state], let change = Self.transform(attributes) else {
                return stop(parser)
            }
            lists[lists.count - 1].states[state] = change.concatenating(current)
        case "draw":
            guard let list = lists.last, let ctm = list.states[state] else { return stop(parser) }
            switch Self.draw(attributes) {
            case .some(.some(let draw)):
                draws.append(CanvasDraws.Draw(op: draw.op, bounds: draw.bounds.applying(ctm.concatenating(list.base)),
                                              fill: draw.fill, border: draw.border, shader: draw.shader))
            case .some(.none): break  // fully transparent: draws nothing
            case .none: unread += 1
            }
        case "clip", "clip-shape", "clip-layer":
            // A clip would show only part of the draws after it; reading them whole could report what is not there.
            stop(parser)
        default:
            // Text, images and nested layers: drawn, but not read here.
            unread += 1
            skippedDepth = 1
        }
    }

    func parser(_ parser: XMLParser, didEndElement name: String, namespaceURI: String?, qualifiedName: String?) {
        if skippedDepth > 0 { skippedDepth -= 1; return }
        if name == "display-list" { lists.removeLast() }
    }

    private func stop(_ parser: XMLParser) {
        unknown = true
        parser.abortParsing()
    }

    private static func transform(_ attributes: [String: String]) -> CGAffineTransform? {
        let known: Set<String> = ["state", "translation", "scale", "matrix"]
        guard Set(attributes.keys).isSubset(of: known) else { return nil }
        var transform = CGAffineTransform.identity
        if let matrix = attributes["matrix"].flatMap(numbers), matrix.count == 6 {
            transform = CGAffineTransform(a: matrix[0], b: matrix[1], c: matrix[2], d: matrix[3], tx: matrix[4], ty: matrix[5])
        } else if attributes["matrix"] != nil {
            return nil
        }
        if let scale = attributes["scale"].flatMap(numbers) {
            guard scale.count == 2 else { return nil }
            transform = CGAffineTransform(scaleX: scale[0], y: scale[1]).concatenating(transform)
        }
        if let translation = attributes["translation"].flatMap(numbers) {
            guard translation.count == 2 else { return nil }
            transform = CGAffineTransform(translationX: translation[0], y: translation[1]).concatenating(transform)
        }
        return transform
    }

    /// One draw call; `.some(nil)` when it is fully transparent; nil for a shape or paint it does not know.
    private static func draw(_ attributes: [String: String]) -> CanvasDraws.Draw?? {
        let lineWidth = attributes["line-width"].flatMap(Double.init)
        let geometry: (op: String, bounds: CGRect, stroked: Bool)?
        if let center = attributes["circle"] ?? attributes["stroked-circle"], let point = numbers(center),
           point.count == 2, let radius = attributes["radius"].flatMap(Double.init) {
            geometry = ("circle", CGRect(x: point[0] - radius, y: point[1] - radius, width: 2 * radius, height: 2 * radius),
                        attributes["stroked-circle"] != nil)
        } else if let rect = (attributes["rect"] ?? attributes["stroked-rect"]).flatMap(numbers), rect.count == 4 {
            let rounded = (attributes["corner-radius"].flatMap(Double.init) ?? 0) > 0
            geometry = (rounded ? "roundRect" : "rect", CGRect(x: rect[0], y: rect[1], width: rect[2], height: rect[3]),
                        attributes["stroked-rect"] != nil)
        } else if let oval = (attributes["ellipse"] ?? attributes["stroked-ellipse"]).flatMap(numbers), oval.count == 4 {
            geometry = ("oval", CGRect(x: oval[0], y: oval[1], width: oval[2], height: oval[3]),
                        attributes["stroked-ellipse"] != nil)
        } else if let text = attributes["path"] ?? attributes["stroked-path"], let path = path(text) {
            geometry = (path.isLine ? "line" : "path", path.bounds, attributes["stroked-path"] != nil)
        } else {
            geometry = nil
        }
        guard let geometry else { return nil }

        var draw = CanvasDraws.Draw(op: geometry.op, bounds: geometry.bounds)
        let alpha = attributes["alpha"].flatMap(Double.init) ?? 1
        if attributes.keys.contains(where: { $0.hasSuffix("-gradient") }) {
            draw.shader = true
        } else if let color = attributes["color"].flatMap({ hex($0, alpha: alpha) }) {
            guard let color else { return .some(nil) }
            if geometry.stroked {
                guard let lineWidth else { return nil }
                draw.border = (round2(max(lineWidth, 1)), color)
            } else {
                draw.fill = color
            }
        } else {
            return nil
        }
        return draw
    }

    /// RenderBox's path text is postfix: `x y m`, `x y l`, `cx cy x y q`, `c1x c1y c2x c2y x y c`, `h` closes.
    /// A stroke path inside a fill (`{s butt miter} 2 [ ... ]`) is not read.
    private static func path(_ text: String) -> (bounds: CGRect, isLine: Bool)? {
        let path = CGMutablePath()
        var operands: [Double] = []
        var segments = 0
        for token in text.split(separator: " ") {
            if let number = Double(token) { operands.append(number); continue }
            let points = stride(from: 0, to: operands.count - 1, by: 2).map { CGPoint(x: operands[$0], y: operands[$0 + 1]) }
            switch (token, points.count, operands.count % 2) {
            case ("m", 1, 0): path.move(to: points[0])
            case ("l", 1, 0): path.addLine(to: points[0]); segments += 1
            case ("q", 2, 0): path.addQuadCurve(to: points[1], control: points[0]); segments += 2
            case ("c", 3, 0): path.addCurve(to: points[2], control1: points[0], control2: points[1]); segments += 2
            case ("h", 0, 0): path.closeSubpath(); segments += 2
            default: return nil
            }
            operands = []
        }
        guard operands.isEmpty, !path.isEmpty else { return nil }
        // The box of the curve itself, not of its control points.
        return (path.boundingBoxOfPath, segments == 1)
    }

    /// `rgb(r, g, b)` or `rgba(r, g, b, a)`, extended sRGB (Display P3 colors go past 0...1), as "#RRGGBBAA".
    /// `.some(nil)` when the color is fully transparent, nil when it cannot be read.
    private static func hex(_ text: String, alpha: Double) -> String?? {
        guard let open = text.firstIndex(of: "("), text.hasSuffix(")"),
              ["rgb", "rgba"].contains(String(text[..<open])) else { return nil }
        let values = text[text.index(after: open)..<text.index(before: text.endIndex)]
            .split(separator: ",").compactMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        guard values.count == 3 || values.count == 4 else { return nil }
        let rgba = [values[0], values[1], values[2], (values.count == 4 ? values[3] : 1) * alpha]
        guard rgba[3] > 0.001 else { return .some(nil) }
        return "#" + rgba.map { String(format: "%02X", Int((min(max($0, 0), 1) * 255).rounded())) }.joined()
    }

    private static func numbers(_ text: String) -> [Double]? {
        let values = text.split(separator: " ").map { Double($0) }
        return values.contains(nil) ? nil : values.compactMap { $0 }
    }
}
#endif
