import XCTest
@testable import AgentDDP

final class StateTests: XCTestCase {
    func testAValueReadsWritesAndRefusesWithoutASetter() throws {
        var budget = 100.0
        AppState.shared.add("budget", AppState.Value(get: { budget }, set: { budget = $0 as? Double ?? budget }))
        AppState.shared.add("total", AppState.Value(get: { 42 }, set: nil))

        let listed = AppState.shared.list()["values"] as? [[String: Any]] ?? []
        XCTAssertTrue(listed.contains { $0["name"] as? String == "budget" && $0["writable"] as? Bool == true })
        XCTAssertTrue(listed.contains { $0["name"] as? String == "total" && $0["writable"] as? Bool == false })

        let read = try AppState.shared.get(["names": ["total"]])["values"] as? [String: Any]
        XCTAssertEqual(read?["total"] as? Int, 42)
        XCTAssertEqual(try AppState.shared.set(["name": "budget", "value": 500.0])["value"] as? Double, 500)
        XCTAssertThrowsError(try AppState.shared.set(["name": "total", "value": 1]))
        XCTAssertThrowsError(try AppState.shared.get(["names": ["missing"]]))
    }

    func testAValueThatIsNotJSONNamesItself() {
        AppState.shared.add("date", AppState.Value(get: { Date() }, set: nil))
        XCTAssertThrowsError(try AppState.shared.get(["names": ["date"]])) { error in
            XCTAssertTrue("\(error)".contains("'date'"))
        }
    }
}

final class EventsTests: XCTestCase {
    func testAWatcherGetsTheKeptEventsThenEachNewOne() {
        let events = AppEvents()
        for index in 0..<205 { events.send("tick", ["n": index]) }
        let kept = events.list()["events"] as? [[String: Any]] ?? []
        XCTAssertEqual(kept.count, 200)
        XCTAssertEqual((kept.first?["fields"] as? [String: Any])?["n"] as? Int, 5)

        var seen: [String] = []
        let id = events.watch { seen.append($0["name"] as? String ?? "") }
        events.send("rule", ["when": Date()])
        events.stop(id)
        events.send("after", [:])
        XCTAssertEqual(seen.count, 201)
        XCTAssertEqual(seen.last, "rule")
    }

    func testAFieldThatIsNotJSONGoesAsItsDescription() {
        let events = AppEvents()
        events.send("rule", ["when": Date(timeIntervalSince1970: 0)])
        let fields = (events.list()["events"] as? [[String: Any]])?.last?["fields"] as? [String: Any]
        XCTAssertTrue(fields?["when"] is String)
    }
}
