#if DEBUG && canImport(UIKit)
import SwiftUI
import UIKit

/// `_UIHostingView` is generic over its root view; this protocol reaches its view debug data without naming that type.
@MainActor
protocol ViewDebugReading {
    func _viewDebugData() -> [_ViewDebug.Data]
}

extension _UIHostingView: ViewDebugReading {}

/// Where a SwiftUI element comes from (`screen.source`, `ddp source`): the app's own view types above it, innermost
/// first, and the modifiers SwiftUI wrapped around it, from SwiftUI's view debug data (the tree Xcode's view
/// debugger reads). SwiftUI records that tree only when the app starts with `SWIFTUI_VIEW_DEBUG` set. The lines of
/// each view's `body` come from the app's debug info, which the CLI reads: the app keeps no source positions.
@MainActor
enum SourceLocations {
    /// type (0), position (3) and size (4): the property bits `_ViewDebug.Properties` names, all ddp needs.
    static let debugMask = 25

    static func of(name: String) -> [String: Any] {
        let windows = FrameCapture.visibleWindows()
        guard let element = AccessibilityReader.object(named: name, in: windows) else {
            return ["status": "error", "error": "No element '\(name)'"]
        }
        if let view = element as? UIView {
            // UIKit keeps no source position; where the view was allocated does, in the malloc stack log the CLI
            // reads with malloc_history. Only kept when the app started with stack logging on.
            guard ProcessInfo.processInfo.environment["MallocStackLogging"] != nil else {
                return none("a UIKit view: its creation stack is kept only when the app starts with stack logging: " +
                            "ddp launch <device> --app <id> --env MallocStackLogging=1")
            }
            return ["status": "uikit", "class": NSStringFromClass(type(of: view)),
                    "address": String(format: "0x%lx", UInt(bitPattern: Unmanaged.passUnretained(view).toOpaque()))]
        }
        guard ProcessInfo.processInfo.environment["SWIFTUI_VIEW_DEBUG"] != nil else {
            return none("SwiftUI records its view tree only when the app starts with it: " +
                        "ddp launch <device> --app <id> --env SWIFTUI_VIEW_DEBUG=\(debugMask)")
        }
        let frame = element.accessibilityFrame
        for host in hostingViews(in: windows) {
            guard let data = _ViewDebug.serializedData(host._viewDebugData()),
                  let roots = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
                Hooks.failed("swiftui.viewDebugData", "no view debug data from \(type(of: host))")
                continue
            }
            Hooks.ok("swiftui.viewDebugData")
            guard let view = host as? UIView, let path = path(to: frame, in: roots, host: view) else { continue }
            return answer(path)
        }
        return none("no SwiftUI view in the view debug data has this element's frame")
    }

    /// The outermost node with the element's frame, and every node above it, root first.
    private static func path(to frame: CGRect, in nodes: [[String: Any]], host: UIView) -> [[String: Any]]? {
        for node in nodes {
            if let rect = rect(of: node, in: host), close(rect, frame) { return [node] }
            if let below = path(to: frame, in: node["children"] as? [[String: Any]] ?? [], host: host) {
                return [node] + below
            }
        }
        return nil
    }

    private static func answer(_ path: [[String: Any]]) -> [String: Any] {
        let types = path.map(typeName)
        let readable = path.map(readableName)
        // App views: a type from a module that is not Apple's. The CLI keeps those whose body it finds in the app.
        let views = types.enumerated().reversed().compactMap { index, type -> [String: Any]? in
            guard let type, let module = type.split(separator: ".").first, !appleModules.contains(String(module)),
                  !type.hasPrefix("(") else { return nil }
            return ["type": type, "depth": index]
        }
        // The modifiers around the element are its parent nodes (_PaddingLayout, then _BackgroundStyleModifier<Color>).
        // A parent counts only when the static type SwiftUI recorded above says so: some ancestor's type holds
        // `ModifiedContent<what is below, that parent>`.
        let known = readable.compactMap { $0 }.joined(separator: "\n")
        var modifiers: [String] = []
        var inner = readable.last ?? nil
        for parent in readable.dropLast().reversed() {
            guard let parent, let inside = inner else { break }
            let wrapped = "ModifiedContent<\(inside), \(parent)>"
            guard known.contains(wrapped) else { break }
            modifiers.append(parent)
            inner = wrapped
        }
        return ["status": "ok", "element": (readable.last ?? nil) ?? "unknown", "views": views, "modifiers": modifiers]
    }

    private static let appleModules: Set<String> = ["SwiftUI", "SwiftUICore", "Swift", "__C", "Foundation", "UIKit",
                                                     "CoreGraphics", "_Concurrency", "Combine", "Observation"]

    private static func property(_ node: [String: Any], _ id: Int) -> [String: Any]? {
        if let byId = node["properties"] as? [String: Any] { return byId[String(id)] as? [String: Any] }
        return (node["properties"] as? [[String: Any]])?.first { ($0["id"] as? Int) == id }?["attribute"] as? [String: Any]
    }

    private static func typeName(_ node: [String: Any]) -> String? { property(node, 0)?["type"] as? String }

    private static func readableName(_ node: [String: Any]) -> String? { property(node, 0)?["readableType"] as? String }

    /// The node's frame in screen points, like an accessibility frame: position and size are in the hosting view's.
    private static func rect(of node: [String: Any], in host: UIView) -> CGRect? {
        guard let position = property(node, 3)?["value"] as? [Double], position.count == 2,
              let size = property(node, 4)?["value"] as? [Double], size.count == 2 else { return nil }
        let local = CGRect(x: position[0], y: position[1], width: size[0], height: size[1])
        guard let window = host.window else { return nil }
        return window.convert(host.convert(local, to: window), to: window.screen.coordinateSpace)
    }

    private static func close(_ a: CGRect, _ b: CGRect) -> Bool {
        abs(a.minX - b.minX) < 1 && abs(a.minY - b.minY) < 1 && abs(a.width - b.width) < 1 && abs(a.height - b.height) < 1
    }

    @MainActor
    private static func hostingViews(in windows: [UIWindow]) -> [ViewDebugReading] {
        var found: [ViewDebugReading] = []
        func visit(_ view: UIView) {
            if let host = view as? ViewDebugReading { found.append(host) }
            view.subviews.forEach(visit)
        }
        windows.forEach(visit)
        return found
    }

    private static func none(_ why: String) -> [String: Any] { ["status": "none", "why": why] }
}
#endif
