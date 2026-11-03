#if DEBUG
import Foundation

/// A node of SwiftUI's view debug tree: the type SwiftUI recorded for it, and the accessibility elements SwiftUI made
/// from it.
final class DebugNode {
    let type: String
    let readableType: String
    let children: [DebugNode]
    let elements: [AnyObject]

    init(type: String, readableType: String? = nil, children: [DebugNode], elements: [AnyObject] = []) {
        self.type = type
        self.readableType = readableType ?? type
        self.children = children
        self.elements = elements
    }
}

extension DebugNode {
    /// SwiftUI's view debug tree from `_accessibilitySwiftUIDebugData`: dictionaries with `kind`, `type`, `children`,
    /// `attribute`, and `element` on an accessibility element's node. An accessibility element's node is not a view:
    /// its children take its place. Its element was made from what is below it, so it goes to its one child (a Text's
    /// insides, a Toggle's), or with several children to the node above. Nil when a node lacks the fields, so a
    /// changed format answers nothing.
    static func tree(from data: [[String: Any]], names: (Any) -> (qualified: String, readable: String)?) -> [DebugNode]? {
        // SwiftUI lists some views twice: childless where the view sits, and with its children among the views of a
        // container above it. Both copies carry the same `attribute`, which puts the children back where the view sits.
        var copies: [UInt32: [[String: Any]]] = [:]
        func collect(_ dict: [String: Any]) {
            if let attribute = dict["attribute"] as? UInt32 { copies[attribute, default: []].append(dict) }
            (dict["children"] as? [[String: Any]])?.forEach(collect)
        }
        data.forEach(collect)
        let hoisted = copies.compactMapValues { list -> [[String: Any]]? in
            let full = list.compactMap { $0["children"] as? [[String: Any]] }.filter { !$0.isEmpty }
            return list.count == 2 && full.count == 1 ? full[0] : nil
        }
        func movedAttribute(_ dict: [String: Any]) -> UInt32? {
            guard let attribute = dict["attribute"] as? UInt32, hoisted[attribute] != nil,
                  let children = dict["children"] as? [[String: Any]], !children.isEmpty else { return nil }
            return attribute
        }
        // An element made from a full copy moves with its children.
        var carried: [UInt32: [AnyObject]] = [:]
        func carry(_ dict: [String: Any]) {
            let children = dict["children"] as? [[String: Any]] ?? []
            if dict["kind"] as? String == "accessibility_element", children.count == 1, let moved = movedAttribute(children[0]),
               let element = dict["element"] {
                carried[moved, default: []].append(element as AnyObject)
            }
            children.forEach(carry)
        }
        data.forEach(carry)
        var placing: Set<UInt32> = []
        func nodes(_ dict: [String: Any]) -> (nodes: [DebugNode], elements: [AnyObject])? {
            guard let kind = dict["kind"] as? String, var children = dict["children"] as? [[String: Any]] else { return nil }
            var placed: UInt32?
            if let attribute = dict["attribute"] as? UInt32, let moved = hoisted[attribute] {
                if !children.isEmpty { return ([], []) }
                // Children placed back inside their own copy would loop: the format changed, so nothing is known.
                guard placing.insert(attribute).inserted else { return nil }
                placed = attribute
                children = moved
            }
            defer { if let placed { placing.remove(placed) } }
            var below: [DebugNode] = []
            var attached: [AnyObject] = placed.flatMap { carried[$0] } ?? []
            for child in children {
                guard let read = nodes(child) else { return nil }
                below += read.nodes
                attached += read.elements
            }
            if kind == "accessibility_element" {
                if children.count == 1, movedAttribute(children[0]) != nil { return (below, attached) }
                let made = dict["element"].map { [$0 as AnyObject] } ?? []
                guard below.count == 1, let only = below.first else { return (below, attached + made) }
                let holder = DebugNode(type: only.type, readableType: only.readableType, children: only.children,
                                       elements: only.elements + made)
                return ([holder], attached)
            }
            guard let type = dict["type"].flatMap(names) else { return nil }
            return ([DebugNode(type: type.qualified, readableType: type.readable, children: below, elements: attached)], [])
        }
        var roots: [DebugNode] = []
        for root in data {
            guard let read = nodes(root) else { return nil }
            roots += read.nodes
        }
        return roots
    }
}

/// Which of the app's views wrote each node of SwiftUI's view debug tree.
///
/// Each of the app's views records a node of its own type, and each view's `body` has a static type, so the types a
/// node may record follow from the type above it: walking down from a node whose type is known, each child is matched
/// against what that type says it can be, and the view whose body wrote the matching type owns it. A node gets an
/// owner only when exactly one view could have written it. Anything else is not known, so a SwiftUI release that
/// records its internals differently makes ddp answer less, never wrong.
final class ViewOwners {
    /// Modules whose types are never the app's own views. A type from any other module counts as an app view: when its
    /// body is not known, nothing at its place in the tree is claimed.
    static let appleModules: Set<String> = ["SwiftUI", "SwiftUICore", "Swift", "__C", "Foundation", "UIKit",
                                            "CoreGraphics", "CoreFoundation", "QuartzCore", "_Concurrency", "Combine",
                                            "Observation"]

    private struct Option: Hashable {
        /// `view`: an app view's own node, written by the view that placed it; its children are its body.
        enum Kind { case plain, content, modifier, view, unresolved, tooDeep }
        /// The type a node records when it is this option; nil when that is not known.
        let recorded: String?
        let owner: String?
        let staticType: String
        let kind: Kind
    }

    private enum Context {
        case expected([Option])
        case inherited(String)
        case unknown
    }

    private struct ListLink {
        let staticType: String
        let owner: String
    }

    private let bodies: [String: String]
    /// The writer of a type that comes from a generic view's arguments is whoever placed that view.
    private var placers: [String: Set<String?>] = [:]
    private var owners: [ObjectIdentifier: String] = [:]
    private var listsByHost: [Int: [ListLink]] = [:]
    private var currentHost = 0
    /// Nodes whose type is written as such in their owner's body, so the element's name.
    private var named: Set<ObjectIdentifier> = []
    private var listOwnerOfCell: [ObjectIdentifier: String] = [:]
    private var cell: ListLink?
    private var cellMatches: [(node: DebugNode, option: Option)] = []
    /// A view whose body is not known could write any type, so "written once across all bodies" proves nothing then.
    private var anchorsAllowed = false
    private var splitCache: [String: (base: String, args: [String])] = [:]
    private var foreignCache: [String: Bool] = [:]

    /// `bodies`: each app view type's name and the static type of its body, both as `_typeName(qualified: true)` writes them.
    init(bodies: [String: String]) {
        self.bodies = bodies
        anchorsAllowed = unresolvedViews().isEmpty
    }

    /// Matches one hosting view's tree. A List's rows sit in their own hosts, in the List's cells: `cellOf` is the host
    /// that holds that List, matched before. Its rows are claimed only when that host has a single List.
    func match(roots: [DebugNode], host: Int, cellOf listHost: Int?) {
        currentHost = host
        let lists = listHost.flatMap { listsByHost[$0] } ?? []
        cell = lists.count == 1 ? lists.first : nil
        cellMatches = []
        roots.forEach(start)
        // A row is claimed only when exactly one node of the cell can be its content.
        if let cell, cellMatches.count == 1, let match = cellMatches.first {
            clear(match.node)
            adopt(match.node, match.option)
            roots.forEach { listOwnerOfCell[ObjectIdentifier($0)] = cell.owner }
        }
        cell = nil
    }

    func owner(of node: DebugNode) -> String? { owners[ObjectIdentifier(node)] }

    /// The view that holds the List a cell host's tree sits in.
    func listOwner(ofCellRoot root: DebugNode) -> String? { listOwnerOfCell[ObjectIdentifier(root)] }

    // MARK: - Walking the tree

    private func start(_ root: DebugNode) {
        guard let body = bodies[root.type] else { return visit(root, .expected([])) }
        set(root, root.type)
        root.children.forEach { visit($0, .expected(options(body, owner: root.type))) }
    }

    private func visit(_ node: DebugNode, _ context: Context) {
        switch context {
        case .unknown: clear(node)
        case .inherited(let owner): inherit(node, owner)
        case .expected(let options) where options.isEmpty: free(node)
        case .expected(let options): expect(node, options)
        }
    }

    private func inherit(_ node: DebugNode, _ owner: String) {
        guard owner.contains("<") else {
            set(node, owner)
            return node.children.forEach { visit($0, .inherited(owner)) }
        }
        switch callerOfGeneric(owner, writing: node.type) {
        case .notTheCallers:
            set(node, owner)
            node.children.forEach { visit($0, .inherited(owner)) }
        case .caller(let caller):
            set(node, caller)
            named.insert(ObjectIdentifier(node))
            node.children.forEach { visit($0, .inherited(caller)) }
        case .unclear:
            clear(node)
        }
    }

    /// A node with nothing above it to match against: only a type that proves where it was written gives it an owner.
    private func free(_ node: DebugNode) {
        if bodies[node.type] != nil { return start(node) }
        if let owner = anchor(node.type) {
            set(node, owner)
            let next = childOptions(Option(recorded: node.type, owner: owner, staticType: node.type, kind: .plain))
            return node.children.forEach { visit($0, .expected(next)) }
        }
        if let cell {
            let matching = looseOptions(cell.staticType, owner: cell.owner)
                .filter { $0.recorded == node.type && viable($0, node) }
            if let first = matching.first, Set(matching.map(\.owner)).count == 1,
               Set(matching.map { [$0.staticType, "\($0.kind)"] }).count == 1 {
                cellMatches.append((node, first))
            }
        }
        // Nothing above says who wrote this node, but its children realize its arguments, and the app views among them
        // own what their bodies write (the App's scene writes `_ConditionalContent<Onboarding, ContentView>`).
        let next = mentionsForeign(node.type)
            ? childOptions(Option(recorded: node.type, owner: nil, staticType: node.type, kind: .plain)) : []
        node.children.forEach { visit($0, .expected(next)) }
    }

    private func expect(_ node: DebugNode, _ options: [Option]) {
        let exact = options.filter { $0.recorded == node.type && viable($0, node) }
        let exactOwners = Set(exact.map(\.owner))
        if options.contains(where: { $0.recorded == nil }) && !(exactOwners.count == 1 && !mentionsForeign(node.type)) {
            // A view whose body is not known could have recorded any type at this place.
            return node.children.forEach { visit($0, .expected([])) }
        }
        if exactOwners.count == 1, let first = exact.first {
            if Set(exact.map { [$0.staticType, "\($0.kind)"] }).count == 1 { return adopt(node, first) }
            return node.children.forEach { visit($0, .expected([])) }
        }
        if exactOwners.count > 1 { return node.children.forEach { visit($0, .expected([])) } }
        // The containers `options` flattens record a node too (TupleContent, ForEach): their children are the options.
        // Matched as a control instead, a tuple would hand every view inside it to the one owner of a Button in it.
        if isTransparent(node.type) || node.type.contains("SwiftUI._VariadicView.Tree<")
            || Self.flattened.contains(split(node.type).base) {
            return node.children.forEach { visit($0, .expected(options)) }
        }
        guard let realized = realized(node, options) else { return node.children.forEach { visit($0, .expected([])) } }
        set(node, realized.owner)
        if let listType = realized.listType {
            listsByHost[currentHost, default: []].append(ListLink(staticType: listType, owner: realized.owner))
        }
        node.children.forEach { visit($0, .inherited(realized.owner)) }
    }

    private func adopt(_ node: DebugNode, _ option: Option) {
        if option.kind == .view, option.owner == nil, bodies[node.type] != nil { return start(node) }
        guard let owner = option.owner else { return clear(node) }
        set(node, owner)
        if option.kind == .plain { named.insert(ObjectIdentifier(node)) }
        if option.kind == .plain, split(option.staticType).base == "SwiftUI.List" {
            listsByHost[currentHost, default: []].append(ListLink(staticType: option.staticType, owner: owner))
        }
        let wrapsNoAppView = option.kind == .modifier && !mentionsForeign(split(option.staticType).args.first ?? "")
        let next = childOptions(option)
        // Matching on below a body's last app view still names the deepest type it writes (the Text, not its HStack).
        // A generic view's insides go to `inherit`, which knows that its arguments are its caller's.
        if (!mentionsForeign(option.staticType) || wrapsNoAppView) && (next.isEmpty || owner.contains("<")) {
            return node.children.forEach { visit($0, .inherited(owner)) }
        }
        node.children.forEach { visit($0, .expected(next)) }
    }

    /// A node that matches no option exactly: SwiftUI's own internals for a control or a List, owned by the option it
    /// must realize.
    private func realized(_ node: DebugNode, _ options: [Option]) -> (owner: String, listType: String?)? {
        let known = options.filter { $0.recorded != nil }
        if let owner = soleOwner(known), !known.contains(where: { mentionsForeign($0.staticType) }) { return (owner, nil) }
        let controls = markers(node)
        let chosen = known.filter {
            $0.kind == .plain && controls.contains(String(split($0.staticType).base.split(separator: ".").last ?? ""))
        }
        if let owner = soleOwner(chosen), !chosen.contains(where: { mentionsForeign($0.staticType) }) { return (owner, nil) }
        if node.type.hasSuffix(".LazyContainerModifier"), known.count == 1, let list = known.first, list.kind == .plain,
           split(list.staticType).base == "SwiftUI.List", let owner = list.owner {
            return (owner, list.staticType)
        }
        return nil
    }

    private func soleOwner(_ options: [Option]) -> String? {
        let owners = Set(options.map(\.owner))
        guard !options.isEmpty, owners.count == 1 else { return nil }
        return owners.first ?? nil
    }

    private func set(_ node: DebugNode, _ owner: String) { owners[ObjectIdentifier(node)] = owner }

    private func clear(_ node: DebugNode) {
        owners[ObjectIdentifier(node)] = nil
        node.children.forEach(clear)
    }

    // MARK: - What a place in a body can be

    /// The nodes a place written as `type` in `owner`'s body may record.
    private func options(_ type: String, owner: String?, depth: Int = 0) -> [Option] {
        guard depth <= 40 else { return [Option(recorded: nil, owner: nil, staticType: type, kind: .tooDeep)] }
        let (base, args) = split(type)
        if isForeign(base) {
            placers[type, default: []].insert(owner)
            return [Option(recorded: type, owner: owner, staticType: type, kind: .view)]
        }
        switch base {
        case "SwiftUI.ModifiedContent" where args.count == 2:
            return [Option(recorded: type, owner: owner, staticType: type, kind: .content),
                    Option(recorded: args[1], owner: owner, staticType: type, kind: .modifier)]
        case "SwiftUI.TupleContent", "SwiftUI.TupleView":
            return elements(args.first ?? "").flatMap { options($0, owner: owner, depth: depth + 1) }
        case "SwiftUI.ForEach" where args.count == 3:
            return options(args[2], owner: owner, depth: depth + 1)
        case "SwiftUI._ConditionalContent":
            return args.flatMap { options($0, owner: owner, depth: depth + 1) }
        case "Swift.Optional", "SwiftUI.Group":
            return options(args.first ?? "", owner: owner, depth: depth + 1)
        default:
            return [Option(recorded: type, owner: owner, staticType: type, kind: .plain)]
        }
    }

    private func childOptions(_ option: Option) -> [Option] {
        let args = split(option.staticType).args
        switch option.kind {
        case .view:
            // A view whose body is not known could have recorded any type below its node.
            guard let body = bodies[option.staticType] else {
                return [Option(recorded: nil, owner: option.staticType, staticType: option.staticType, kind: .unresolved)]
            }
            return options(body, owner: option.staticType)
        case .content where args.count == 2:
            // SwiftUI records the content below the `ModifiedContent` node, or for some modifiers the modifier first.
            return [Option(recorded: args[1], owner: option.owner, staticType: option.staticType, kind: .modifier)]
                + options(args[0], owner: option.owner)
        case .modifier where args.count == 2:
            return options(args[0], owner: option.owner)
        default:
            // A container's children realize its generic arguments.
            return args.flatMap { options($0, owner: option.owner) }
        }
    }

    /// What a List's rows may be, a few levels deep (a Section of rows, a row view's body, what its modifiers wrap): a
    /// cell holds only part of the row.
    private func looseOptions(_ type: String, owner: String) -> [Option] {
        loose(childOptions(Option(recorded: type, owner: owner, staticType: type, kind: .plain)), depth: 0)
    }

    private func loose(_ options: [Option], depth: Int) -> [Option] {
        guard depth <= 8 else { return [] }
        return options.flatMap { option -> [Option] in
            let opens = option.kind == .view || option.kind == .content || option.kind == .modifier
                || (option.kind == .plain && mentionsForeign(option.staticType))
            guard opens, option.owner != nil else { return [option] }
            return [option] + loose(childOptions(option), depth: depth + 1)
        }
    }

    /// Every modifier SwiftUI keeps records a node, so a modifier option holds only when a child is what it wraps.
    private func viable(_ option: Option, _ node: DebugNode, depth: Int = 0) -> Bool {
        guard option.kind == .modifier || option.kind == .content, depth <= 6 else { return true }
        let inner = childOptions(option)
        return node.children.contains { child in
            if inner.contains(where: { $0.recorded == child.type && viable($0, child, depth: depth + 1) }) { return true }
            if isTransparent(child.type) && viable(option, child, depth: depth + 1) { return true }
            // A List records SwiftUI's lazy container in its place.
            return child.type.hasSuffix(".LazyContainerModifier")
                && inner.contains { $0.kind == .plain && split($0.staticType).base == "SwiftUI.List" }
        }
    }

    private enum Caller { case notTheCallers, caller(String), unclear }

    /// A generic view's body places the content it was given: a type that is one of its arguments was written by the
    /// view that placed it, unless its own body writes that type too.
    private func callerOfGeneric(_ owner: String, writing type: String) -> Caller {
        guard let open = owner.firstIndex(of: "<"), owner.hasSuffix(">") else { return .notTheCallers }
        let args = String(owner[owner.index(after: open)..<owner.index(before: owner.endIndex)])
        guard args.contains(type), let body = bodies[owner] else { return .notTheCallers }
        let ownUses = occurrences(of: type, in: body) - occurrences(of: args, in: body) * occurrences(of: type, in: args)
        let placed = placers[owner] ?? []
        if ownUses <= 0, placed.count == 1, let caller = placed.first ?? nil { return .caller(caller) }
        return .unclear
    }

    /// A type that names an app view and is written exactly once across all bodies was written in that body.
    private func anchor(_ type: String) -> String? {
        guard anchorsAllowed, mentionsForeign(type), !bodies.values.contains(type) else { return nil }
        let hits = bodies.filter { $0.value.contains(type) }
        guard hits.count == 1, let hit = hits.first, occurrences(of: type, in: hit.value) == 1 else { return nil }
        return hit.key
    }

    private static let control = try! NSRegularExpression(
        pattern: #"SwiftUI\.(?:Resolved)?([A-Z][A-Za-z]*?)Style(?:Configuration|Body|Base|Modifier)?\b"#)

    /// The controls whose style SwiftUI resolved below a node (`ButtonStyleConfiguration`, `ResolvedButtonStyleBody`).
    private func markers(_ node: DebugNode, depth: Int = 0) -> Set<String> {
        guard depth <= 12 else { return [] }
        let range = NSRange(node.type.startIndex..., in: node.type)
        let here = Self.control.matches(in: node.type, range: range).compactMap { match in
            Range(match.range(at: 1), in: node.type).map { String(node.type[$0]) }
        }
        return node.children.reduce(into: Set(here)) { $0.formUnion(markers($1, depth: depth + 1)) }
    }

    private static let flattened: Set<String> = ["SwiftUI.TupleContent", "SwiftUI.TupleView", "SwiftUI.ForEach",
                                                 "SwiftUI._ConditionalContent", "Swift.Optional", "SwiftUI.Group"]

    private func isTransparent(_ type: String) -> Bool {
        type.hasPrefix("SwiftUI.PlaceholderContentView<") || type.hasPrefix("SwiftUI.OpacityRendererEffect")
            || type.hasPrefix("SwiftUI.ModifiedContent<SwiftUI.PlaceholderContentView<")
    }

    // MARK: - Type strings

    /// `Name<a, b>` into `Name` and its top-level arguments `[a, b]`.
    private func split(_ type: String) -> (base: String, args: [String]) {
        if let known = splitCache[type] { return known }
        let parsed = Self.split(type)
        splitCache[type] = parsed
        return parsed
    }

    static func split(_ type: String) -> (base: String, args: [String]) {
        guard let open = type.firstIndex(of: "<"), type.hasSuffix(">") else { return (type, []) }
        let inner = Array(type[type.index(after: open)..<type.index(before: type.endIndex)])
        var args: [String] = []
        var current = ""
        var depth = 0
        var index = 0
        // A plain loop: `->` in a function type must not close a bracket, which needs one character of lookahead.
        while index < inner.count {
            let character = inner[index]
            if character == "-", index + 1 < inner.count, inner[index + 1] == ">" {
                current += "->"
                index += 2
                continue
            }
            if "<({[".contains(character) { depth += 1 }
            if ">)}]".contains(character) { depth -= 1 }
            if character == ",", depth == 0 {
                args.append(current.trimmingCharacters(in: .whitespaces))
                current = ""
            } else {
                current.append(character)
            }
            index += 1
        }
        args.append(current.trimmingCharacters(in: .whitespaces))
        return (String(type[..<open]), args)
    }

    /// `Pack{a, b}` and `(a, b)`, the element lists of TupleContent and TupleView.
    private func elements(_ list: String) -> [String] {
        let inner: Substring
        if list.hasPrefix("Pack{"), list.hasSuffix("}") {
            inner = list.dropFirst(5).dropLast()
        } else if list.hasPrefix("("), list.hasSuffix(")") {
            inner = list.dropFirst().dropLast()
        } else {
            return [list]
        }
        return split("X<\(inner)>").args
    }

    private func isForeign(_ base: String) -> Bool {
        guard !base.hasPrefix("("), let module = base.split(separator: ".").first, base.contains(".") else { return false }
        return !Self.appleModules.contains(String(module))
    }

    private static let moduleName = try! NSRegularExpression(pattern: #"(?<![A-Za-z0-9_.)$])([A-Za-z_][A-Za-z0-9_]*)\."#)

    /// Whether a type names a type outside Apple's modules anywhere in it.
    private func mentionsForeign(_ type: String) -> Bool {
        if let known = foreignCache[type] { return known }
        let found = Self.moduleNames(in: type).contains { !Self.appleModules.contains($0) }
        foreignCache[type] = found
        return found
    }

    static func moduleNames(in type: String) -> [String] {
        moduleName.matches(in: type, range: NSRange(type.startIndex..., in: type)).compactMap { match in
            Range(match.range(at: 1), in: type).map { String(type[$0]) }
        }
    }

    /// App views the bodies name whose own body is not known.
    private func unresolvedViews() -> Set<String> {
        var unresolved: Set<String> = []
        var seen: Set<String> = []
        func collect(_ type: String) {
            guard seen.insert(type).inserted else { return }
            let (base, args) = split(type)
            if isForeign(base), bodies[type] == nil { unresolved.insert(type) }
            if base.hasPrefix("(") || base.hasPrefix("Pack{") { elements(base).forEach(collect) }
            args.flatMap(elements).forEach(collect)
        }
        bodies.values.forEach(collect)
        return unresolved
    }

    private func occurrences(of needle: String, in text: String) -> Int {
        guard !needle.isEmpty else { return 0 }
        return text.components(separatedBy: needle).count - 1
    }
}

// MARK: - From an accessibility element to the view that wrote it

extension ViewOwners {
    struct Found {
        let owner: String
        /// The nodes from the host's root down to the element.
        let path: [DebugNode]
        /// The app's views around the one that wrote it, innermost first.
        let inside: [String]
    }

    enum Answer {
        case found(Found)
        case noNode
        case notKnown
    }

    /// The node SwiftUI made the element from, and who wrote it: the owner of the node the element hangs below.
    func element(_ target: AnyObject, hosts: [[DebugNode]]) -> Answer {
        // A plain loop: `lazy.compactMap(...).first` runs the closure twice for the match, at every level of the tree.
        func path(_ trail: [DebugNode]) -> [DebugNode]? {
            guard let node = trail.last else { return nil }
            if node.elements.contains(where: { $0 === target }) { return trail }
            for child in node.children {
                if let found = path(trail + [child]) { return found }
            }
            return nil
        }
        guard let found = hosts.joined().reduce(nil, { $0 ?? path([$1]) }) else { return .noNode }
        guard let writer = found.last.flatMap(owner(of:)) else { return .notKnown }
        // The deepest node its writer's body names as it is (a Text, a Button), not SwiftUI's insides below it.
        let shown = found.lastIndex { named.contains(ObjectIdentifier($0)) && owner(of: $0) == writer }
            .map { Array(found[...$0]) } ?? found
        var inside: [String] = []
        for node in found.reversed() {
            if let around = owner(of: node), around != writer, !inside.contains(around) { inside.append(around) }
        }
        if let root = found.first, let list = listOwner(ofCellRoot: root), list != writer, !inside.contains(list) {
            inside.append(list)
        }
        return .found(Found(owner: writer, path: shown, inside: inside))
    }
}
#endif
