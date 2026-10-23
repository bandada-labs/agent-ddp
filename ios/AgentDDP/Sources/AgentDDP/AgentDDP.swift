import Foundation
import QuartzCore
#if DEBUG && canImport(UIKit)
import UIKit
#endif

/// Records what the app drew, frame by frame, when the `ddp` CLI asks for it.
/// Everything below compiles to an empty `start()` outside Debug builds.
public enum AgentDDP {
    public static func start() {
        #if DEBUG && canImport(UIKit)
        FrameRecorder.shared.listenForRequests()
        LiveServer.shared.start(directory: FrameRecorder.shared.outputDirectory)
        #endif
    }

    /// Call from the app's Debug network decorator once a request finishes, so `ddp net` can list it.
    /// Headers, query items and JSON keys that look secret are redacted before anything is kept.
    public static func logRequest(_ request: URLRequest, response: URLResponse?, body: Data?, error: Error?,
                                  startedAt: Date, finishedAt: Date = Date()) {
        #if DEBUG && canImport(UIKit)
        NetworkLog.shared.add(request: request, response: response, body: body, error: error,
                              startedAt: startedAt, finishedAt: finishedAt)
        #endif
    }

    /// Seconds on ddp's controllable clock (`ddp time pause|step|seek|rate`). An app's Debug clock port can read
    /// this instead of the system clock so its own timelines pause, step and rewind with ddp.
    /// Outside Debug it is CACurrentMediaTime().
    public static func elapsedTime() -> TimeInterval {
        #if DEBUG && canImport(UIKit)
        return Thread.isMainThread ? TimeControl.now() : DispatchQueue.main.sync { TimeControl.now() }
        #else
        return CACurrentMediaTime()
        #endif
    }

    /// Literal values (the app's dev secret, a test token) to replace with "[redacted]" wherever they show up.
    public static func redact(values: [String]) {
        #if DEBUG && canImport(UIKit)
        NetworkLog.shared.redact(values: values)
        #endif
    }
}

#if DEBUG && canImport(UIKit)

/// Starts a recording when the CLI posts the Darwin notification (offline mode: the file in the app container).
final class FrameRecorder {
    static let shared = FrameRecorder()

    private static let requestNotificationName = "dev.agentddp.record"
    private static let defaultFrameCount = 120
    private static let maxFrameCount = 600

    let outputDirectory = FileManager.default
        .urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("agent-ddp")
    private var stream: FrameStream?
    private var output: FileHandle?

    private var partialFile: URL { outputDirectory.appendingPathComponent("recording.partial") }
    private var finishedFile: URL { outputDirectory.appendingPathComponent("recording.jsonl") }
    private var requestFile: URL { outputDirectory.appendingPathComponent("request.json") }

    func listenForRequests() {
        AccessibilityReader.enableAutomationTree()
        try? FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in DispatchQueue.main.async { FrameRecorder.shared.beginRecording() } },
            Self.requestNotificationName as CFString, nil, .deliverImmediately)
    }

    private func beginRecording() {
        guard stream == nil else { return }
        let request = (try? Data(contentsOf: requestFile))
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        let frames = min(max(request?["frames"] as? Int ?? Self.defaultFrameCount, 1), Self.maxFrameCount)
        FileManager.default.createFile(atPath: partialFile.path, contents: nil)
        output = try? FileHandle(forWritingTo: partialFile)
        stream = FrameStream(frames: frames, send: { [weak self] line in
            guard let self, let output = self.output, let data = jsonLine(line) else { return false }
            output.write(data)
            return true
        }, finished: { [weak self] in self?.finishRecording() })
        stream?.start()
    }

    private func finishRecording() {
        try? output?.close()
        output = nil
        stream = nil
        try? FileManager.default.removeItem(at: finishedFile)
        try? FileManager.default.moveItem(at: partialFile, to: finishedFile)
    }
}

/// Captures display frames and hands each line (header, frames, end) to `send`, for a file or a live client.
/// `frames` 0 runs until `send` returns false or `stop()` is called.
final class FrameStream {
    private let frameLimit: Int
    private let send: ([String: Any]) -> Bool
    private let finished: () -> Void
    private var displayLink: CADisplayLink?
    private var frameIndex = 0
    private var firstTimestamp: CFTimeInterval = 0
    private var slowestCaptureMs = 0.0
    private var totalCaptureMs = 0.0

    init(frames: Int, send: @escaping ([String: Any]) -> Bool, finished: @escaping () -> Void = {}) {
        frameLimit = frames
        self.send = send
        self.finished = finished
    }

    /// Main thread only.
    func start() {
        guard send(FrameCapture.header(frames: frameLimit)) else { return finished() }
        let link = CADisplayLink(target: self, selector: #selector(captureFrame(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    func stop() {
        guard let link = displayLink else { return }
        link.invalidate()
        displayLink = nil
        _ = send(["type": "end", "frames": frameIndex, "slowestCaptureMs": round2(slowestCaptureMs),
                  "averageCaptureMs": round2(totalCaptureMs / Double(max(frameIndex, 1)))])
        finished()
    }

    @objc private func captureFrame(_ link: CADisplayLink) {
        if frameIndex == 0 { firstTimestamp = link.timestamp }
        let captureStart = CACurrentMediaTime()
        let line = FrameCapture.frame(index: frameIndex, milliseconds: (link.timestamp - firstTimestamp) * 1000)
        let captureMs = (CACurrentMediaTime() - captureStart) * 1000
        slowestCaptureMs = max(slowestCaptureMs, captureMs)
        totalCaptureMs += captureMs
        frameIndex += 1
        if !send(line) || (frameLimit > 0 && frameIndex >= frameLimit) { stop() }
    }
}

enum FrameCapture {
    static func header(frames: Int) -> [String: Any] {
        ["type": "header", "platform": "ios", "unit": "pt", "scale": UIScreen.main.scale, "frames": frames,
         "startedAt": (Date().timeIntervalSince1970 * 1000).rounded()]
    }

    /// Main thread only.
    static func frame(index: Int, milliseconds: Double) -> [String: Any] {
        let windows = visibleWindows()
        return ["type": "frame", "i": index, "t": round2(milliseconds),
                "elements": windows.flatMap(AccessibilityReader.elements(in:)),
                "nodes": LayerReader.paintedNodes(in: windows, textColors: index == 0)]
    }

    static func visibleWindows() -> [UIWindow] {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
            .filter { !$0.isHidden }
            .sorted { $0.windowLevel < $1.windowLevel }
    }
}

func jsonLine(_ line: [String: Any]) -> Data? {
    guard var data = try? JSONSerialization.data(withJSONObject: line) else { return nil }
    data.append(0x0A)
    return data
}

enum AccessibilityReader {
    private static let accessibilityIdentifierSelector = NSSelectorFromString("accessibilityIdentifier")

    /// SwiftUI only builds its accessibility tree when an assistive client or XCTest is
    /// attached. This flips the same private switch XCTest uses. Debug builds only.
    static func enableAutomationTree() {
        typealias SetEnabled = @convention(c) (Bool) -> Void
        guard let library = dlopen("/usr/lib/libAccessibility.dylib", RTLD_NOW),
              let symbol = dlsym(library, "_AXSSetAutomationEnabled") else { return }
        unsafeBitCast(symbol, to: SetEnabled.self)(true)
    }

    static func elements(in window: UIWindow) -> [[String: Any]] {
        var found: [[String: Any]] = []
        visitElements(window) { object in
            var element: [String: Any] = ["frame": rectArray(object.accessibilityFrame)]
            if let id = identifier(of: object) { element["id"] = id }
            if let label = object.accessibilityLabel, !label.isEmpty { element["label"] = label }
            if !object.accessibilityTraits.intersection([.button, .link, .adjustable]).isEmpty { element["tap"] = true }
            found.append(element)
        }
        return found
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

    private static func identifier(of object: NSObject) -> String? {
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
    /// `textColors`: also read the color of each text bitmap from its pixels (SwiftUI `Text` has no other trace of it).
    static func paintedNodes(in windows: [UIWindow], textColors: Bool = false) -> [[String: Any]] {
        var nodes: [[String: Any]] = []
        for window in windows {
            let root = window.layer.presentation() ?? window.layer
            let windowClip = CGRect(origin: .zero, size: window.bounds.size)
            walk(window.layer, root: root, clip: windowClip, opacity: 1, offset: window.frame.origin,
                 textColors: textColors, into: &nodes)
        }
        return nodes
    }

    // ponytail: paint order is sublayer order, re-sorted by zPosition; 3D transforms are ignored.
    private static func walk(_ layer: CALayer, root: CALayer, clip: CGRect, opacity: Float,
                             offset: CGPoint, textColors: Bool, into nodes: inout [[String: Any]]) {
        let shown = layer.presentation() ?? layer
        let effectiveOpacity = opacity * shown.opacity
        guard !shown.isHidden, effectiveOpacity > 0.001 else { return }
        let frame = shown.convert(shown.bounds, to: root).offsetBy(dx: offset.x, dy: offset.y)
        let visible = frame.intersection(clip)

        if !visible.isEmpty, var node = paintDescription(of: layer, shown: shown) {
            node["z"] = nodes.count
            node["frame"] = rectArray(frame)
            if visible != frame { node["visible"] = rectArray(visible) }
            if effectiveOpacity < 0.999 { node["opacity"] = round2(Double(effectiveOpacity)) }
            if shown.masksToBounds { node["clips"] = true }
            // UIKit view classes help name a coverer; SwiftUI's mangled internal classes do not.
            if let view = layer.delegate as? UIView, !NSStringFromClass(type(of: view)).hasPrefix("_Tt") {
                node["source"] = NSStringFromClass(type(of: view))
            }
            if let text = textStyle(of: layer, sampling: textColors) { node["text"] = text }
            nodes.append(node)
        }

        let childClip = shown.masksToBounds ? clip.intersection(frame) : clip
        guard !childClip.isEmpty else { return }
        // A stable sort keeps sublayer order among equal zPosition, which is how Core Animation draws them.
        let ordered = (layer.sublayers ?? []).enumerated()
            .sorted { ($0.element.zPosition, $0.offset) < ($1.element.zPosition, $1.offset) }.map(\.element)
        ordered.forEach {
            walk($0, root: root, clip: childClip, opacity: effectiveOpacity, offset: offset, textColors: textColors,
                 into: &nodes)
        }
    }

    /// Size and color of the text a layer draws: from UILabel / UITextField / UITextView when there is one,
    /// otherwise (SwiftUI `Text`) the main color of its pixels, when asked to sample.
    private static func textStyle(of layer: CALayer, sampling: Bool) -> [String: Any]? {
        if let label = layer.delegate as? UILabel, let font = label.font {
            return ["size": round2(font.pointSize), "color": hex(label.textColor.cgColor) ?? "transparent",
                    "weight": weight(of: font)]
        }
        if let field = layer.delegate as? UITextField, let font = field.font {
            return ["size": round2(font.pointSize), "color": hex(field.textColor?.cgColor) ?? "transparent",
                    "weight": weight(of: font)]
        }
        guard sampling, NSStringFromClass(type(of: layer)).contains("CGDrawingLayer"),
              let pixels = PixelSampler.sample(layer),
              let main = (pixels["colors"] as? [[String: Any]])?.first?["color"] as? String else { return nil }
        // The bitmap's line height is about 1.2 times the font size.
        return ["color": main, "size": round2(layer.bounds.height / 1.2), "estimated": true]
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
        if shown.cornerRadius > 0 { node["corner"] = round2(shown.cornerRadius) }

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
    private static let visibleAlpha = 13  // about 5 %

    /// The colors a drawing shows (top 3 with their share of its painted pixels), its strongest alpha,
    /// and how much of its box is painted.
    static func sample(_ layer: CALayer) -> [String: Any]? {
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
