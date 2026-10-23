#if DEBUG && canImport(UIKit)
import QuartzCore
import UIKit

/// `ddp time`: one virtual clock for the app. Core Animation (UIKit animations) follows it through each
/// window layer's speed and timeOffset; an app that reads `AgentDDP.elapsedTime()` from its clock port
/// follows it too, so seek and rewind replay its own timelines. SwiftUI's own `withAnimation` runs on a
/// clock nothing public reaches, so it does not pause (measured on iOS 27). Main thread only.
enum TimeControl {
    private static let frameDuration = 1.0 / 60
    private static var rate = 1.0
    private static var anchorReal = CACurrentMediaTime()
    private static var anchorVirtual = CACurrentMediaTime()
    private static var pausePoint: Double?

    static func now() -> Double { anchorVirtual + (CACurrentMediaTime() - anchorReal) * rate }

    static func handle(action: String, value: Double?) -> [String: Any] {
        switch action {
        case "pause":
            if pausePoint == nil { pausePoint = now() }
            set(virtual: now(), rate: 0)
        case "resume":
            pausePoint = nil
            set(virtual: now(), rate: 1)
        case "rate":
            guard let value, value >= 0 else { return error("rate needs a number >= 0, e.g. 0.1") }
            set(virtual: now(), rate: value)
        case "step":
            guard rate == 0 else { return error("pause first, then step") }
            set(virtual: now() + Double(Int(value ?? 1)) * frameDuration, rate: 0)
        case "seek":
            guard let pausePoint, let value else { return error("pause first; seek takes a time from the pause point") }
            set(virtual: pausePoint + value / 1000, rate: 0)
        case "status":
            break
        default:
            return error("time actions: pause, resume, step [frames], rate <factor>, seek <time>, status")
        }
        return state()
    }

    private static func set(virtual: Double, rate newRate: Double) {
        anchorReal = CACurrentMediaTime()
        anchorVirtual = virtual
        rate = newRate
        // A layer's local time is (media time - beginTime) * speed + timeOffset: now it equals `virtual`.
        for window in FrameCapture.visibleWindows() {
            window.layer.beginTime = anchorReal
            window.layer.timeOffset = virtual
            window.layer.speed = Float(newRate)
        }
    }

    private static func state() -> [String: Any] {
        var state: [String: Any] = ["type": "time", "rate": rate, "paused": rate == 0]
        if let pausePoint { state["sincePauseMs"] = round2((now() - pausePoint) * 1000) }
        return state
    }

    private static func error(_ message: String) -> [String: Any] { ["type": "error", "error": message] }
}
#endif
