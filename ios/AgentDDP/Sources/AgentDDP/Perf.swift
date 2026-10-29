#if DEBUG && canImport(UIKit)
import QuartzCore
import UIKit

/// `perf.metrics`: how the app runs for a while, from the platform's own clock. Refreshes the main thread missed come
/// from CADisplayLink's vsync times, as on Android from Choreographer's, so both platforms count the same thing. iOS
/// gives an app no draw time per frame (the render server draws), so that is not measured. History captures nothing
/// meanwhile, so its cost is not counted.
@MainActor
final class Perf: NSObject {
    private var timestamps: [CFTimeInterval] = []
    private var periodMs = 1000.0 / 60

    static func measure(_ params: [String: Any]) async throws -> [String: Any] {
        let ms = min(max((params["ms"] as? NSNumber)?.doubleValue ?? 5000, 100), 60_000)
        let perf = Perf()
        let link = CADisplayLink(target: perf, selector: #selector(tick(_:)))
        History.activeStreams += 1
        link.add(to: .main, forMode: .common)
        defer {
            link.invalidate()
            History.activeStreams -= 1
        }
        try await Task.sleep(nanoseconds: UInt64(ms * 1_000_000))
        return perf.answer(ms: ms)
    }

    @objc private func tick(_ link: CADisplayLink) {
        timestamps.append(link.timestamp)
        if link.targetTimestamp > link.timestamp { periodMs = (link.targetTimestamp - link.timestamp) * 1000 }
    }

    private func answer(ms: Double) -> [String: Any] {
        let gaps = zip(timestamps, timestamps.dropFirst()).map { ($1 - $0) * 1000 }
        // A gap of two periods is one refresh missed; a little over one period is timer noise, not a miss.
        let missed = gaps.reduce(0) { $0 + max(0, Int(($1 / periodMs).rounded()) - 1) }
        return ["ms": ms, "refreshMs": (periodMs * 100).rounded() / 100, "refreshes": gaps.count, "missed": missed,
                "worstGapMs": ((gaps.max() ?? 0) * 100).rounded() / 100, "drawn": NSNull()]
    }
}
#endif
