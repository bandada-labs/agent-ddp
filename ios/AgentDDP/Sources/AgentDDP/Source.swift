#if DEBUG && canImport(UIKit)
import SwiftUI
import UIKit

/// Where a SwiftUI element comes from (`screen.source`, `ddp source`): the app's view whose `body` wrote it, the app's
/// views around that one, and the modifiers SwiftUI wrapped around it. The data is SwiftUI's view debug tree, recorded
/// only when the app starts with `SWIFTUI_VIEW_DEBUG` set, with the accessibility elements SwiftUI made from each node,
/// and the static type of each app view's body; `ViewOwners` matches the two. The lines of each body come from the
/// app's debug info, which the CLI reads: the app keeps no source positions.
@MainActor
enum SourceLocations {
    /// The type bit of `_ViewDebug.Property`: the tree's types are all ddp reads.
    static let debugMask = 1

    /// The hosting view's tree as dictionaries (`kind`, `type` as a metatype, `children`, and `element` on an
    /// accessibility element's node). Unlike `_viewDebugData()`, which traps on iOS 27 outside SwiftUI's update (`UpdateGroup.current` is nil
    /// in `AGAttribute.syncMainIfReferences`), this one reads the tree inside it.
    private static let debugData = NSSelectorFromString("_accessibilitySwiftUIDebugData")

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
        let hosts = hostingViews(in: windows)
        var trees: [[DebugNode]] = []
        var metatypes: [Any.Type] = []
        for host in hosts {
            guard let data = host.view.perform(debugData)?.takeUnretainedValue() as? [[String: Any]],
                  let roots = read(data, metatypes: &metatypes) else {
                Hooks.failed("swiftui.viewDebugData", "no view debug data from \(type(of: host.view))")
                return none("SwiftUI's view tree could not be read, so which view holds it is not known")
            }
            trees.append(roots)
        }
        if !hosts.isEmpty { Hooks.ok("swiftui.viewDebugData") }
        let bodies = AppViews.bodies(metatypes: metatypes)
        if bodies.isEmpty {
            Hooks.failed("swiftui.viewTypes", "no view type of the app's own in the view debug tree")
            return none("no view type of the app's own was found in SwiftUI's view tree, so which view holds it is not known")
        }
        Hooks.ok("swiftui.viewTypes")
        let owners = ViewOwners(bodies: bodies)
        zip(hosts.indices, trees).forEach { owners.match(roots: $1, host: $0, cellOf: hosts[$0].cellOf) }
        switch owners.element(element, hosts: trees) {
        case .noNode: return none("no node of SwiftUI's view debug data made this element")
        case .notKnown: return none("SwiftUI's view tree does not prove which of the app's views wrote it, so it is not known")
        case .found(let found): return answer(found)
        }
    }

    private static func answer(_ found: ViewOwners.Found) -> [String: Any] {
        let readable = found.path.map(\.readableType)
        // The modifiers around the element: each parent that is `ModifiedContent<what is below, the modifier>`.
        var modifiers: [String] = []
        var inner = readable.last ?? ""
        for parent in readable.dropLast().reversed() {
            let (base, args) = ViewOwners.split(parent)
            guard base == "ModifiedContent", args.count == 2, args[0] == inner else { break }
            modifiers.append(args[1])
            inner = parent
        }
        let views = [["type": found.owner, "role": "writes"]] + found.inside.map { ["type": $0, "role": "holds"] }
        // The path's types, innermost first, so an answer can be checked against what SwiftUI recorded.
        return ["status": "ok", "element": readable.last ?? "unknown", "views": views, "modifiers": modifiers,
                "path": Array(found.path.map(\.type).reversed())]
    }

    /// The tree with each node's type names, keeping the metatypes: they give the app's views, generic ones included.
    private static func read(_ data: [[String: Any]], metatypes: inout [Any.Type]) -> [DebugNode]? {
        DebugNode.tree(from: data) { value in
            guard let type = value as? Any.Type else { return nil }
            metatypes.append(type)
            return (_typeName(type, qualified: true), _typeName(type, qualified: false))
        }
    }

    /// Every hosting view, each after the one that holds it. `cellOf` is the host whose List cell holds this one.
    private static func hostingViews(in windows: [UIWindow]) -> [(view: UIView, cellOf: Int?)] {
        var found: [(view: UIView, cellOf: Int?)] = []
        func visit(_ view: UIView, host: Int?, inCell: Bool) {
            var host = host
            var inCell = inCell || view is UICollectionViewCell || view is UITableViewCell
            if view.responds(to: debugData) {
                found.append((view, inCell ? host : nil))
                host = found.count - 1
                inCell = false
            }
            view.subviews.forEach { visit($0, host: host, inCell: inCell) }
        }
        windows.forEach { visit($0, host: nil, inCell: false) }
        return found
    }

    private static func none(_ why: String) -> [String: Any] { ["status": "none", "why": why] }
}

/// The app's own SwiftUI view types and the static type of each one's body.
enum AppViews {
    /// Views from the tree's node types, then by mangled name (`_typeByName`) the ones their bodies name: a view only
    /// built in a closure (a ForEach row) has no node of its own.
    static func bodies(metatypes: [Any.Type]) -> [String: String] {
        var views: [String: any View.Type] = [:]
        var checked: Set<String> = []
        func add(_ type: Any.Type) {
            guard let view = type as? any View.Type else { return }
            let name = _typeName(type, qualified: true)
            guard checked.insert(name).inserted, isAppType(type) else { return }
            views[name] = view
        }
        metatypes.forEach(add)
        var bodies: [String: String] = [:]
        var pending: Set<String> = []
        // Each body found can name more views; a few rounds reach every view the screen's bodies name.
        for _ in 0..<8 {
            for (name, view) in views where bodies[name] == nil {
                let body = bodyName(view)
                bodies[name] = body
                pending.insert(body)
            }
            let before = views.count
            Set(pending.flatMap(plainNames)).subtracting(checked).compactMap(lookUp).forEach(add)
            pending = []
            if views.count == before { break }
        }
        return bodies
    }

    private static func bodyName<V: View>(_: V.Type) -> String { _typeName(V.Body.self, qualified: true) }

    private static let plainName = try! NSRegularExpression(
        pattern: #"(?<![A-Za-z0-9_.)$])([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+)(?![A-Za-z0-9_<.])"#)

    /// Names of non-generic types outside Apple's modules in a type string.
    private static func plainNames(in type: String) -> [String] {
        plainName.matches(in: type, range: NSRange(type.startIndex..., in: type)).compactMap { match in
            Range(match.range(at: 1), in: type).map { String(type[$0]) }
        }.filter { name in !ViewOwners.appleModules.contains(String(name.prefix { $0 != "." })) }
    }

    /// `_typeByName` takes mangled names: `App.Row` is `3App3RowV`. Views are structs; the types they nest in may be
    /// structs, enums or classes.
    private static func lookUp(_ name: String) -> Any.Type? {
        let parts = name.split(separator: ".").map(String.init)
        guard parts.count >= 2, parts.count <= 4, parts.allSatisfy({ $0.allSatisfy(\.isASCII) }) else { return nil }
        var outer: [[String]] = [[]]
        for _ in parts.dropFirst().dropLast() { outer = outer.flatMap { kinds in ["V", "O", "C"].map { kinds + [$0] } } }
        for kinds in outer {
            let mangled = parts.enumerated().map { index, part in
                "\(part.utf8.count)\(part)" + (index == 0 ? "" : index == parts.count - 1 ? "V" : kinds[index - 1])
            }.joined()
            if let type = _typeByName(mangled), _typeName(type, qualified: true) == name { return type }
        }
        return nil
    }

    private static let bundlePath = Bundle.main.bundleURL.resolvingSymlinksInPath().path

    /// A type whose nominal type descriptor lives in an image inside the app's bundle: the app's code or a framework it
    /// ships, never Apple's. Struct and enum metadata start with their kind and then the descriptor (Swift's stable ABI).
    private static func isAppType(_ type: Any.Type) -> Bool {
        let metadata = unsafeBitCast(type, to: UnsafeRawPointer.self)
        let kind = metadata.load(as: Int.self)
        guard kind == 0x200 || kind == 0x201 else { return false }
        let descriptor = metadata.load(fromByteOffset: MemoryLayout<Int>.size, as: UnsafeRawPointer.self)
        var info = Dl_info()
        guard dladdr(descriptor, &info) != 0, let file = info.dli_fname else { return false }
        return URL(fileURLWithPath: String(cString: file)).resolvingSymlinksInPath().path.hasPrefix(bundlePath + "/")
    }
}
#endif
