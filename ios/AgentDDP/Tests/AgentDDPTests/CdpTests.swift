import XCTest
@testable import AgentDDP

final class CdpTests: XCTestCase {
    func testAcceptKeyMatchesTheRfcExample() {
        XCTAssertEqual(WebSocket.acceptKey("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    }

    func testAMaskedFragmentedMessageWaitsForItsLastPiece() {
        var buffer = clientFrame(fin: false, opcode: WebSocket.text, text: "{\"id\":")
        XCTAssertNil(WebSocket.parse(&buffer))
        buffer += clientFrame(fin: true, opcode: WebSocket.continuation, text: "1}")
        let frame = WebSocket.parse(&buffer)
        XCTAssertEqual(frame?.opcode, WebSocket.text)
        XCTAssertEqual(frame.map { String(decoding: $0.payload, as: UTF8.self) }, "{\"id\":1}")
        XCTAssertTrue(buffer.isEmpty)
    }

    func testALongServerFrameReadsBack() {
        let text = String(repeating: "x", count: 70_000)
        var buffer = WebSocket.frame(opcode: WebSocket.text, payload: Data(text.utf8))
        XCTAssertEqual(WebSocket.parse(&buffer).map { String(decoding: $0.payload, as: UTF8.self) }, text)
    }

    func testTheRequestLeavesTheWebSocketBytesInTheBuffer() {
        var buffer = Data("GET /devtools/page/app HTTP/1.1\r\nHost: localhost:9222\r\nUpgrade: websocket\r\n\r\n".utf8)
        buffer += clientFrame(fin: true, opcode: WebSocket.text, text: "{}")
        let request = HttpRequest.parse(&buffer)
        XCTAssertEqual(request?.path, "/devtools/page/app")
        XCTAssertEqual(request?.headers["upgrade"], "websocket")
        XCTAssertNotNil(WebSocket.parse(&buffer))
    }

    func testAnUnknownMethodAnswersLikeChrome() {
        let answer = answer(to: "{\"id\":7,\"method\":\"Foo.bar\"}")
        XCTAssertEqual(answer["id"] as? Int, 7)
        let error = answer["error"] as? [String: Any]
        XCTAssertEqual(error?["code"] as? Int, -32601)
        XCTAssertEqual(error?["message"] as? String, "'Foo.bar' wasn't found")
    }

    func testAKnownMethodAnswersWithItsResultAndSession() {
        let answer = answer(to: "{\"id\":1,\"method\":\"Target.getTargets\",\"sessionId\":\"s1\"}")
        let targets = (answer["result"] as? [String: Any])?["targetInfos"] as? [[String: Any]]
        XCTAssertEqual(targets?.first?["targetId"] as? String, "app")
        XCTAssertEqual(answer["sessionId"] as? String, "s1")
    }

    func testOnlyLocalHostsAndNoWebPages() {
        XCTAssertTrue(Cdp.hostAllowed("localhost:9222"))
        XCTAssertTrue(Cdp.hostAllowed("127.0.0.1:50123"))
        XCTAssertTrue(Cdp.hostAllowed("[::1]:9222"))
        XCTAssertFalse(Cdp.hostAllowed("evil.example:9222"))
        XCTAssertTrue(Cdp.originAllowed(nil))
        XCTAssertTrue(Cdp.originAllowed("devtools://devtools"))
        XCTAssertFalse(Cdp.originAllowed("http://localhost:3000"))
    }

    func testAttachingAnnouncesTheSessionBeforeAnswering() {
        let sent = messages(after: ["{\"id\":3,\"method\":\"Target.attachToTarget\",\"params\":{\"targetId\":\"app\",\"flatten\":true}}"])
        XCTAssertEqual(sent.first?["method"] as? String, "Target.attachedToTarget")
        let announced = (sent.first?["params"] as? [String: Any])?["sessionId"] as? String
        XCTAssertNotNil(announced)
        XCTAssertEqual((sent.last?["result"] as? [String: Any])?["sessionId"] as? String, announced)
    }

    func testOnlyTheBrowserTargetAutoAttachesThePage() {
        let autoAttach = "{\"id\":1,\"method\":\"Target.setAutoAttach\",\"params\":{\"autoAttach\":true,\"flatten\":true}}"
        XCTAssertEqual(messages(after: [autoAttach], browserLevel: true).count, 2)
        XCTAssertEqual(messages(after: [autoAttach], browserLevel: false).count, 1)
    }

    func testTheAccessibilityTreeHangsTheElementsFromOneRoot() {
        let nodes = Cdp.axTree(title: "Example", elements: [["label": "Tap me", "role": "button"], ["id": "flashBox"]])
        XCTAssertEqual(nodes.count, 3)
        XCTAssertEqual(nodes[0]["childIds"] as? [String], ["2", "3"])
        XCTAssertEqual((nodes[1]["role"] as? [String: Any])?["value"] as? String, "button")
        XCTAssertEqual((nodes[1]["name"] as? [String: Any])?["value"] as? String, "Tap me")
        XCTAssertEqual((nodes[2]["name"] as? [String: Any])?["value"] as? String, "flashBox")
        XCTAssertEqual(nodes[2]["parentId"] as? String, "1")
    }

    func testSelectorsMatchRoleIdAndLabel() throws {
        let dom = Dom(title: "Example", elements: [
            ["role": "button", "id": "saveButton", "label": "Save"], ["role": "StaticText", "label": "Saved at 10:00"],
            ["role": "button", "label": "Cancel"],
        ])
        XCTAssertEqual(try dom.query("button"), [4, 6])
        XCTAssertEqual(try dom.query("button#saveButton"), [4])
        XCTAssertEqual(try dom.query("[label='Save']"), [4])
        XCTAssertEqual(try dom.query("text[label^=Saved]"), [5])
        XCTAssertEqual(try dom.query("[label*=\"ance\"], #saveButton"), [4, 6])
        XCTAssertEqual(try dom.query("[id]"), [4])
        XCTAssertThrowsError(try dom.query("body button"))
        XCTAssertEqual(try dom.outerHTML(4), "<button role=\"button\" id=\"saveButton\" label=\"Save\">Save</button>")
    }

    func testTheDocumentStopsAtTheAskedDepth() {
        let dom = Dom(title: "Example", elements: [["role": "button"]])
        let html = (dom.document(depth: 1)["children"] as? [[String: Any]])?.first
        XCTAssertEqual(html?["nodeName"] as? String, "HTML")
        XCTAssertNil(html?["children"])
        XCTAssertEqual(dom.document(depth: -1).description.contains("BUTTON"), true)
    }

    func testTheBoxModelIsTheFrame() throws {
        let model = try Dom(title: "Example", elements: [["frame": [10.0, 20.0, 30.0, 40.0]]]).boxModel(4)
        XCTAssertEqual(model["content"] as? [Double], [10, 20, 40, 20, 40, 60, 10, 60])
        XCTAssertThrowsError(try Dom(title: "Example", elements: []).boxModel(4))
    }

    func testAFinishedRequestBecomesChromesThreeEvents() {
        let events = Cdp.networkEvents(["id": "n1", "method": "POST", "url": "https://api.example.com/items",
                                        "startedAt": 1_000_000.0, "ms": 250.0, "status": 201, "responseBytes": 12,
                                        "requestBody": "{}", "responseHeaders": ["Content-Type": "application/json; charset=utf-8"]])
        XCTAssertEqual(events.map(\.method), ["Network.requestWillBeSent", "Network.responseReceived", "Network.loadingFinished"])
        XCTAssertEqual((events[0].params["request"] as? [String: Any])?["postData"] as? String, "{}")
        let response = events[1].params["response"] as? [String: Any]
        XCTAssertEqual(response?["status"] as? Int, 201)
        XCTAssertEqual(response?["mimeType"] as? String, "application/json")
        XCTAssertEqual(events[0].params["timestamp"] as? Double, 1000)
        XCTAssertEqual(events[2].params["timestamp"] as? Double, 1000.25)
    }

    func testAFailedRequestEndsInLoadingFailed() {
        let events = Cdp.networkEvents(["id": "n2", "url": "https://down.example.com", "startedAt": 0.0, "ms": 5.0,
                                        "error": "The Internet connection appears to be offline."])
        XCTAssertEqual(events.map(\.method), ["Network.requestWillBeSent", "Network.loadingFailed"])
        XCTAssertEqual(events[1].params["errorText"] as? String, "The Internet connection appears to be offline.")
    }

    func testALogEntryCarriesItsCategoryAndLevel() {
        let entry = AppLog.logEntry(level: .fault, category: "network", message: "timed out",
                                    date: Date(timeIntervalSince1970: 1_729_700_000.123))
        XCTAssertEqual(entry["level"] as? String, "error")
        XCTAssertEqual(entry["text"] as? String, "network: timed out")
        XCTAssertEqual(entry["timestamp"] as? Double, 1_729_700_000_123)
    }

    private func messages(after input: [String], browserLevel: Bool = true) -> [[String: Any]] {
        var sent: [[String: Any]] = []
        let session = CdpSession(target: CdpTarget(bundleIdentifier: "com.example", title: "Example"),
                                 browserLevel: browserLevel) {
            sent.append((try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any] ?? [:])
        }
        input.forEach(session.receive)
        return sent
    }

    private func answer(to message: String) -> [String: Any] {
        var text = ""
        CdpSession(target: CdpTarget(bundleIdentifier: "com.example", title: "Example")) { text = $0 }.receive(message)
        return (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
    }

    private func clientFrame(fin: Bool, opcode: UInt8, text: String) -> Data {
        let mask: [UInt8] = [1, 2, 3, 4]
        let payload = Array(text.utf8)
        return Data([(fin ? 0x80 : 0) | opcode, 0x80 | UInt8(payload.count)] + mask
            + payload.enumerated().map { $0.element ^ mask[$0.offset % 4] })
    }
}
