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
