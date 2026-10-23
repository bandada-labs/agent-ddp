#if DEBUG
import CoreGraphics
import Foundation

/// Which elements changed since the last capture, and whether only busy ones did. An element that keeps changing
/// for `busyAfterMs`, never still for longer than `quietMs`, is busy: a clock, a spinner, a loop. Captures can come
/// faster than an animation's own frames, so "changed in every capture" would miss them.
struct ChangeTracker<Signature: Equatable> {
    let busyAfterMs: Double
    let quietMs: Double
    private var last: [String: Signature] = [:]
    private var changingSince: [String: Double] = [:]
    private var lastChangeAt: [String: Double] = [:]
    private(set) var onlyBusyChanging = false

    init(busyAfterMs: Double, quietMs: Double) {
        self.busyAfterMs = busyAfterMs
        self.quietMs = quietMs
    }

    mutating func changed(_ signatures: [String: Signature], at now: Double) -> [String] {
        let changed = Set(signatures.keys).union(last.keys).filter { signatures[$0] != last[$0] }.sorted()
        last = signatures
        for key in changed {
            if now - lastChangeAt[key, default: -.infinity] > quietMs { changingSince[key] = now }
            lastChangeAt[key] = now
        }
        // A capture that saw nothing new does not end a busy stretch: it only came between two of its frames.
        if !changed.isEmpty {
            onlyBusyChanging = changed.allSatisfy { now - changingSince[$0, default: now] >= busyAfterMs }
        }
        return changed
    }
}

/// What each element drew, compared with Foundation equality: cheaper than JSON on every capture.
enum FrameSignatures {
    /// Per element (named like the CLI names them), the element and the nodes it drew; nodes no element holds under "".
    /// iOS nodes carry no owner, so a node belongs to the smallest element whose frame holds it, as in the CLI.
    static func of(_ frame: [String: Any]) -> [String: [NSDictionary]] {
        let elements = frame["elements"] as? [[String: Any]] ?? []
        let nodes = frame["nodes"] as? [[String: Any]] ?? []
        var seen: [String: Int] = [:]
        let names = elements.map { element -> String in
            let id = element["id"] as? String ?? ""
            let name = id.isEmpty ? element["label"] as? String ?? "" : id
            seen[name, default: 0] += 1
            return seen[name] == 1 ? name : "\(name) [\(seen[name]!)]"
        }
        let boxes = elements.map { rect($0["frame"]) }
        var parts = elements.map { [$0 as NSDictionary] }
        var unowned: [NSDictionary] = []
        for node in nodes {
            let box = rect(node["frame"])
            let owner = boxes.indices.filter { holds(boxes[$0], box) }.min { area(boxes[$0]) < area(boxes[$1]) }
            if let owner { parts[owner].append(node as NSDictionary) } else { unowned.append(node as NSDictionary) }
        }
        var signatures = Dictionary(zip(names, parts), uniquingKeysWith: { first, _ in first })
        signatures[""] = unowned
        return signatures
    }

    private static let tolerance: CGFloat = 4

    private static func holds(_ outer: CGRect, _ inner: CGRect) -> Bool {
        outer.insetBy(dx: -tolerance, dy: -tolerance).contains(inner)
    }

    private static func area(_ box: CGRect) -> CGFloat { box.width * box.height }

    private static func rect(_ value: Any?) -> CGRect {
        let numbers = (value as? [Any] ?? []).compactMap { ($0 as? NSNumber)?.doubleValue }
        guard numbers.count == 4 else { return .null }
        return CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3])
    }
}
#endif
