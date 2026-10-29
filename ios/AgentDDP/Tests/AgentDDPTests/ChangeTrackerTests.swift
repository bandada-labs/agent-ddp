import XCTest
@testable import AgentDDP

final class ChangeTrackerTests: XCTestCase {
    func testAnElementThatKeepsChangingBecomesBusy() {
        var tracker = ChangeTracker<String>(busyAfterMs: 2_000, quietMs: 250)
        // Changes in two of every three captures, like an animation slower than the capture rate.
        for step in 0..<150 where step % 3 != 2 {
            _ = tracker.changed(["clock": "\(step)"], at: Double(step) * 16)
        }
        XCTAssertTrue(tracker.onlyBusyChanging)
    }

    func testACaptureWithNothingNewKeepsTheBusyState() {
        var tracker = ChangeTracker<String>(busyAfterMs: 2_000, quietMs: 250)
        for step in 0..<150 { _ = tracker.changed(["clock": "\(step)"], at: Double(step) * 16) }
        XCTAssertEqual(tracker.changed(["clock": "149"], at: 150 * 16), [])
        XCTAssertTrue(tracker.onlyBusyChanging)
    }

    func testAnotherElementChangingEndsTheBusyState() {
        var tracker = ChangeTracker<String>(busyAfterMs: 2_000, quietMs: 250)
        for step in 0..<150 { _ = tracker.changed(["clock": "\(step)", "button": "off"], at: Double(step) * 16) }
        XCTAssertTrue(tracker.onlyBusyChanging)
        XCTAssertEqual(tracker.changed(["clock": "150", "button": "on"], at: 150 * 16), ["button", "clock"])
        XCTAssertFalse(tracker.onlyBusyChanging)
    }

    func testRedrawsThatChangeNothingThrottleAfterTheBusyTime() {
        var tracker = ChangeTracker<String>(busyAfterMs: 2_000, quietMs: 250)
        _ = tracker.changed(["row": "a"], at: 0)
        // A spinner that ddp reads no value from: the app draws every frame, the capture sees nothing new.
        for step in 1..<120 { _ = tracker.changed(["row": "a"], at: Double(step) * 16) }
        XCTAssertFalse(tracker.throttled)
        for step in 120..<130 { _ = tracker.changed(["row": "a"], at: Double(step) * 16) }
        XCTAssertTrue(tracker.throttled)
        XCTAssertFalse(tracker.onlyBusyChanging)
    }

    func testAChangeAfterRedrawsThatChangeNothingEndsTheThrottle() {
        var tracker = ChangeTracker<String>(busyAfterMs: 2_000, quietMs: 250)
        for step in 0..<200 { _ = tracker.changed(["row": "a"], at: Double(step) * 16) }
        XCTAssertTrue(tracker.throttled)
        XCTAssertEqual(tracker.changed(["row": "b"], at: 200 * 16), ["row"])
        XCTAssertFalse(tracker.throttled)
    }

    func testAPauseLongerThanTheQuietGapStartsOver() {
        var tracker = ChangeTracker<String>(busyAfterMs: 2_000, quietMs: 250)
        for step in 0..<150 { _ = tracker.changed(["clock": "\(step)"], at: Double(step) * 16) }
        _ = tracker.changed(["clock": "149"], at: 3_000)
        _ = tracker.changed(["clock": "again"], at: 3_400)
        XCTAssertFalse(tracker.onlyBusyChanging)
    }

    func testANodeBelongsToTheSmallestElementThatHoldsIt() {
        let frame: [String: Any] = [
            "elements": [["id": "card", "frame": [0, 0, 300, 200]], ["id": "button", "frame": [10, 10, 100, 40]]],
            "nodes": [["z": 0, "kind": "layer", "frame": [10, 10, 100, 40], "fill": "#0000FFFF"],
                      ["z": 1, "kind": "layer", "frame": [500, 500, 10, 10]]],
        ]
        let signatures = FrameSignatures.of(frame)
        XCTAssertEqual(signatures["button"]?.count, 2)
        XCTAssertEqual(signatures["card"]?.count, 1)
        XCTAssertEqual(signatures[""]?.count, 1)
        var moved = frame
        moved["nodes"] = [["z": 0, "kind": "layer", "frame": [10, 10, 100, 40], "fill": "#FF0000FF"]]
        XCTAssertNotEqual(FrameSignatures.of(moved)["button"], signatures["button"])
        XCTAssertEqual(FrameSignatures.of(frame), signatures)
    }

    func testCrc32MatchesTheStandardCheckValue() {
        XCTAssertEqual(crc32(Data("123456789".utf8)), 0xCBF4_3926)
    }
}
