import XCTest
@testable import AgentDDP

final class ControlTests: XCTestCase {
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
        var buffer = Data("GET / HTTP/1.1\r\nHost: localhost:9222\r\nUpgrade: websocket\r\n\r\n".utf8)
        buffer += clientFrame(fin: true, opcode: WebSocket.text, text: "{}")
        let request = HttpRequest.parse(&buffer)
        XCTAssertEqual(request?.headers["upgrade"], "websocket")
        XCTAssertNotNil(WebSocket.parse(&buffer))
    }

    func testAnUnknownMethodAnswersWithItsName() {
        let answer = answer(to: "{\"id\":7,\"method\":\"foo.bar\"}")
        XCTAssertEqual(answer["id"] as? Int, 7)
        let error = answer["error"] as? [String: Any]
        XCTAssertEqual(error?["code"] as? Int, -32601)
        XCTAssertEqual(error?["message"] as? String, "Unknown method 'foo.bar'")
    }

    func testOnlyLocalHosts() {
        XCTAssertTrue(Control.hostAllowed("localhost:9222"))
        XCTAssertTrue(Control.hostAllowed("127.0.0.1:50123"))
        XCTAssertTrue(Control.hostAllowed("[::1]:9222"))
        XCTAssertFalse(Control.hostAllowed("evil.example:9222"))
    }

    func testATouchNeedsItsPhase() {
        let error = answer(to: #"{"id":1,"method":"input.touch","params":{"x":1,"y":1}}"#)["error"] as? [String: Any]
        XCTAssertEqual(error?["message"] as? String, "phase is down, move or up")
    }

    func testALogEntryCarriesItsTagAndLevel() {
        let entry = AppLog.logEntry(level: .fault, category: "network", message: "timed out",
                                    date: Date(timeIntervalSince1970: 1_729_700_000.123))
        XCTAssertEqual(entry["level"] as? String, "error")
        XCTAssertEqual(entry["tag"] as? String, "network")
        XCTAssertEqual(entry["text"] as? String, "timed out")
        XCTAssertEqual(entry["t"] as? Int, 1_729_700_000_123)
    }

    func testTheLibrarysOwnLogLinesAreLeftOut() {
        XCTAssertTrue(AppLog.isOwnNoise(subsystem: "com.apple.network", message: "nw_connection_report_state"))
        XCTAssertTrue(AppLog.isOwnNoise(subsystem: "com.apple.Accessibility", message: "Attempting to send notification"))
        XCTAssertFalse(AppLog.isOwnNoise(subsystem: "com.apple.UIKit", message: "Unable to simultaneously satisfy constraints"))
        XCTAssertFalse(AppLog.isOwnNoise(subsystem: "dev.agentddp.sample", message: "animatedButton tapped"))
    }

    func testAPausedRequestShowsRedactedAndGoesOnWithItsRealSecret() async throws {
        let paused = expectation(description: "net.paused")
        var event: [String: Any] = [:]
        let session = ControlSession(app: ControlApp(bundleIdentifier: "com.example", name: "Example")) { text in
            let message = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
            guard message["method"] as? String == "net.paused" else { return }
            event = message["params"] as? [String: Any] ?? [:]
            paused.fulfill()
        }
        defer { session.close() }
        session.receive(#"{"id":1,"method":"net.intercept","params":{"urls":["*/items\\?*"]}}"#)
        var request = URLRequest(url: URL(string: "http://127.0.0.1/items?page=1")!)
        request.setValue("Bearer real", forHTTPHeaderField: "Authorization")
        let sent = Task { [request] in
            try await AgentDDP.send(request) { sent in
                (Data(), HTTPURLResponse(url: sent.url!, statusCode: 200, httpVersion: nil, headerFields: [
                    "X-Authorization": sent.value(forHTTPHeaderField: "Authorization") ?? "",
                    "X-Extra": sent.value(forHTTPHeaderField: "X-Extra") ?? "",
                ])!)
            }
        }
        await fulfillment(of: [paused], timeout: 2)
        let shownHeaders = (event["request"] as? [String: Any])?["headers"] as? [String: String]
        XCTAssertEqual(shownHeaders?["Authorization"], "[redacted]")
        let id = event["id"] as? String ?? ""
        session.receive(#"{"id":2,"method":"net.continue","params":{"id":"\#(id)","headers":{"Authorization":"[redacted]","X-Extra":"1"}}}"#)
        let response = try await sent.value.1 as? HTTPURLResponse
        XCTAssertEqual(response?.value(forHTTPHeaderField: "X-Authorization"), "Bearer real")
        XCTAssertEqual(response?.value(forHTTPHeaderField: "X-Extra"), "1")
        XCTAssertNil(Fetch.wildcard("*/items\\?*").firstMatch(in: "http://a/itemsX", range: NSRange(location: 0, length: 15)))
    }

    func testACommandAnswersItsValue() async throws {
        AgentDDP.command("login") { params in ["user": params["user"] as? String ?? ""] }
        let answers = AsyncStream<[String: Any]>.makeStream()
        let session = ControlSession(app: ControlApp(bundleIdentifier: "com.example", name: "Example")) { text in
            answers.continuation.yield(try! JSONSerialization.jsonObject(with: Data(text.utf8)) as! [String: Any])
        }
        var iterator = answers.stream.makeAsyncIterator()
        session.receive(#"{"id":1,"method":"app.run","params":{"name":"login","params":{"user":"test"}}}"#)
        let result = await iterator.next()?["result"] as? [String: Any]
        XCTAssertEqual((result?["value"] as? [String: String])?["user"], "test")
        session.receive(#"{"id":2,"method":"app.run","params":{"name":"nope"}}"#)
        let error = await iterator.next()?["error"] as? [String: Any]
        XCTAssertEqual(error?["message"] as? String, "No command 'nope'; app.commands lists them")
        session.receive(#"{"id":3,"method":"app.commands"}"#)
        let commands = await iterator.next()?["result"] as? [String: Any]
        XCTAssertEqual(commands?["commands"] as? [String], ["login"])
    }

    func testACommandThatReturnsNoJSONSaysSo() async throws {
        AgentDDP.command("date") { _ in Date() }
        do {
            _ = try await Commands.shared.run(["name": "date"])
            XCTFail("a Date is not JSON")
        } catch {
            XCTAssertEqual("\(error)", "'date' returned a value that is not JSON")
        }
    }

    func testPreferencesKeepATypeHideASecretAndClear() throws {
        let suite = "agentddp-prefs-test"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defaults.removePersistentDomain(forName: suite)
        defaults.set(false, forKey: "onboarded")
        defaults.set("real", forKey: "apiToken")
        func call(_ method: String, _ params: String = "") -> [String: Any] {
            answer(to: #"{"id":1,"method":"\#(method)","params":{"file":"\#(suite)"\#(params)}}"#)
        }

        XCTAssertNotNil(call("prefs.write", #","key":"onboarded","value":"true""#)["result"])
        XCTAssertNotNil(call("prefs.write", #","key":"apiToken","value":"[redacted]""#)["result"])
        XCTAssertNotNil(call("prefs.write", #","key":"onboarded","value":"yes""#)["error"])
        let values = (call("prefs.read")["result"] as? [String: Any])?["values"] as? [String: String]
        XCTAssertEqual(values, ["apiToken": "[redacted]", "onboarded": "true"])
        XCTAssertEqual(defaults.object(forKey: "onboarded") as? Bool, true)
        XCTAssertEqual(defaults.string(forKey: "apiToken"), "real")

        _ = call("prefs.clear")
        XCTAssertNil(defaults.persistentDomain(forName: suite)?["onboarded"])
    }

    private func answer(to message: String) -> [String: Any] {
        var text = ""
        ControlSession(app: ControlApp(bundleIdentifier: "com.example", name: "Example")) { text = $0 }.receive(message)
        return (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
    }

    private func clientFrame(fin: Bool, opcode: UInt8, text: String) -> Data {
        let mask: [UInt8] = [1, 2, 3, 4]
        let payload = Array(text.utf8)
        return Data([(fin ? 0x80 : 0) | opcode, 0x80 | UInt8(payload.count)] + mask
            + payload.enumerated().map { $0.element ^ mask[$0.offset % 4] })
    }
}
