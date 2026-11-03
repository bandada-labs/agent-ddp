import XCTest
@testable import AgentDDP

final class ViewOwnersTests: XCTestCase {
    /// `_accessibilitySwiftUIDebugData` of each hosting view of a probe app on the iOS 27 simulator: a NavigationStack
    /// with a header view, plain controls, ForEach rows, a generic card, a padded item and a List of row views.
    func testTheProbeScreenNamesTheViewThatWroteEachElement() throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "ios27-probe", withExtension: "json", subdirectory: "Fixtures"))
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        let types = try XCTUnwrap(fixture["types"] as? [String])
        // Each element is its own object, as SwiftUI's are; the fixture names them by label ("Circle#2" for the second).
        var elements: [String: NSObject] = [:]
        func element(_ label: String) -> NSObject {
            if let known = elements[label] { return known }
            let made = NSObject()
            elements[label] = made
            return made
        }
        // The fixture keeps each node as [type index, 1 for an accessibility element's node, its label, children,
        // attribute].
        func dictionary(_ value: Any) -> [String: Any] {
            let node = value as! [Any]
            var out: [String: Any] = ["type": types[node[0] as! Int], "children": (node[3] as! [Any]).map(dictionary),
                                      "attribute": UInt32(node[4] as! Int)]
            out["kind"] = node[1] as! Int == 1 ? "accessibility_element" : "view"
            if node[1] as! Int == 1 { out["element"] = element(node[2] as! String) }
            return out
        }
        let hosts = try (fixture["hosts"] as! [[String: Any]]).map { host in
            (cellOf: (host["cellOf"] as! Int) < 0 ? nil : host["cellOf"] as? Int,
             roots: try XCTUnwrap(DebugNode.tree(from: (host["roots"] as! [Any]).map(dictionary)) { ($0 as! String, $0 as! String) }))
        }
        let owners = ViewOwners(bodies: try XCTUnwrap(fixture["bodies"] as? [String: String]))
        hosts.enumerated().forEach { owners.match(roots: $1.roots, host: $0, cellOf: $1.cellOf) }

        let answers = (fixture["labels"] as! [String]).map { label -> String in
            switch owners.element(element(label), hosts: hosts.map(\.roots)) {
            case .found(let found):
                let inside = found.inside.isEmpty ? "" : " inside \(found.inside.joined(separator: ", "))"
                return "\(label): \(found.owner)\(inside), \(found.path.last?.type ?? "")"
            case .noNode: return "\(label): no node"
            case .notKnown: return "\(label): not known"
            }
        }
        XCTAssertEqual(answers, [
            "row 1: Probe.RowView inside Probe.ContentView, SwiftUI.Text",
            "row 0: Probe.RowView inside Probe.ContentView, SwiftUI.Text",
            "Switch: Probe.ContentView, SwiftUI.Toggle<SwiftUI.Text>",
            "Switch#2: Probe.ContentView, SwiftUI.Toggle<SwiftUI.Text>",
            "Star: Probe.ContentView, SwiftUI.Button<SwiftUI.Label<SwiftUI.Text, SwiftUI.Image>>",
            "Star#2: Probe.ContentView, SwiftUI.Button<SwiftUI.Label<SwiftUI.Text, SwiftUI.Image>>",
            "Favorite: Probe.ContentView, SwiftUI.Button<SwiftUI.Label<SwiftUI.Text, SwiftUI.Image>>",
            "Go: Probe.ContentView, SwiftUI.Button<SwiftUI.Text>",
            "direct: Probe.ContentView, SwiftUI.Text",
            "Pay: Probe.Header inside Probe.ContentView, SwiftUI.Text",
            "item: Probe.Item inside Probe.ContentView, SwiftUI.Text",
            "in card: Probe.ContentView inside Probe.Card<SwiftUI.Text>, SwiftUI.Text",
            "list 0: Probe.ListRow inside Probe.ContentView, SwiftUI.Text",
            "Circle: Probe.ListRow inside Probe.ContentView, SwiftUI.Image",
            "list 1: Probe.ListRow inside Probe.ContentView, SwiftUI.Text",
            "Circle#2: Probe.ListRow inside Probe.ContentView, SwiftUI.Image",
            "list 2: Probe.ListRow inside Probe.ContentView, SwiftUI.Text",
            "Circle#3: Probe.ListRow inside Probe.ContentView, SwiftUI.Image",
        ])
    }

    private let label = NSObject()
    private let stackBody = "SwiftUI.VStack<SwiftUI.TupleView<(App.A, App.B)>>"

    /// Root's VStack holding A's Text, recorded with A's node when `withViewNode`, and the element made from the Text.
    private func tree(withViewNode: Bool = true) -> DebugNode {
        let text = DebugNode(type: "SwiftUI.Text", children: [
            DebugNode(type: "SwiftUI.StyledTextContentView", children: [], elements: [label]),
        ])
        let tuple = "SwiftUI.TupleView<(App.A, App.B)>"
        return DebugNode(type: "App.Root", children: [
            DebugNode(type: stackBody, children: [
                DebugNode(type: "SwiftUI._VariadicView.Tree<SwiftUI._VStackLayout, \(tuple)>", children: [
                    DebugNode(type: tuple, children: [withViewNode ? DebugNode(type: "App.A", children: [text]) : text]),
                ]),
            ]),
        ])
    }

    func testTheViewWhoseNodeHoldsAnElementWroteIt() {
        let owners = ViewOwners(bodies: ["App.Root": stackBody, "App.A": "SwiftUI.Text", "App.B": "SwiftUI.Text"])
        let root = tree()
        owners.match(roots: [root], host: 0, cellOf: nil)
        guard case .found(let found) = owners.element(label, hosts: [[root]]) else { return XCTFail("A's node holds the Text") }
        XCTAssertEqual(found.owner, "App.A")
        XCTAssertEqual(found.inside, ["App.Root"])
        XCTAssertEqual(found.path.last?.type, "SwiftUI.Text")
    }

    func testAnElementBelowNoNodeOfTheAppsViewsIsNotKnown() {
        let owners = ViewOwners(bodies: ["App.Root": stackBody, "App.A": "SwiftUI.Text", "App.B": "SwiftUI.Text"])
        let root = tree(withViewNode: false)
        owners.match(roots: [root], host: 0, cellOf: nil)
        guard case .notKnown = owners.element(label, hosts: [[root]]) else {
            return XCTFail("with no node of A or B above it, either could have written the Text")
        }
    }

    func testAViewWhoseBodyIsNotKnownClaimsNothingBelowItsNode() {
        let owners = ViewOwners(bodies: ["App.Root": stackBody, "App.B": "SwiftUI.Image"])
        let root = tree()
        owners.match(roots: [root], host: 0, cellOf: nil)
        guard case .notKnown = owners.element(label, hosts: [[root]]) else {
            return XCTFail("A's body is not known, so what is below its node is not known")
        }
    }

    func testAnElementNoNodeMadeHasNoNode() {
        let owners = ViewOwners(bodies: ["App.Root": stackBody, "App.A": "SwiftUI.Text", "App.B": "SwiftUI.Image"])
        let root = tree()
        owners.match(roots: [root], host: 0, cellOf: nil)
        guard case .noNode = owners.element(NSObject(), hosts: [[root]]) else {
            return XCTFail("an element no node made has no node")
        }
    }

    /// A view listed twice under one attribute, childless where it sits and with its children higher up, comes back
    /// once, where it sits, with the element made from its full copy.
    func testAHoistedViewGetsItsChildrenAndElementBack() throws {
        let element = NSObject()
        let data: [[String: Any]] = [[
            "kind": "view", "type": "Outer", "attribute": UInt32(1), "children": [
                ["kind": "accessibility_element", "attribute": UInt32(2), "element": element, "children": [
                    ["kind": "view", "type": "Toggle", "attribute": UInt32(3), "children": [
                        ["kind": "view", "type": "Inside", "attribute": UInt32(4), "children": [[String: Any]]()],
                    ]],
                ]],
                ["kind": "view", "type": "Holder", "attribute": UInt32(5), "children": [
                    ["kind": "view", "type": "Toggle", "attribute": UInt32(3), "children": [[String: Any]]()],
                ]],
            ],
        ]]
        let roots = try XCTUnwrap(DebugNode.tree(from: data) { ($0 as! String, $0 as! String) })
        XCTAssertEqual(roots.first?.children.map(\.type), ["Holder"])
        let toggle = try XCTUnwrap(roots.first?.children.first?.children.first)
        XCTAssertEqual(toggle.type, "Toggle")
        XCTAssertEqual(toggle.children.map(\.type), ["Inside"])
        XCTAssertTrue(toggle.elements.contains { $0 === element })
    }
}
