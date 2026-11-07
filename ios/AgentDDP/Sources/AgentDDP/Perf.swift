#if DEBUG && canImport(UIKit)
import QuartzCore
import UIKit

/// `perf.metrics`: how the app runs for a while, from the platform's own clock. Refreshes the main thread missed come
/// from CADisplayLink's vsync times, as on Android from Choreographer's, so both platforms count the same thing. The
/// app's work is the main run loop's busy time, from waking to going back to sleep after Core Animation's commit:
/// input, animations, layout, drawing and the commit, the app's share of a frame (the render server draws the rest).
/// iOS does not tell an app which refreshes drew something, so `drawn` is null and the work is per refresh. History
/// captures nothing meanwhile, so its cost is not counted.
@MainActor
final class Perf: NSObject {
    private var timestamps: [CFTimeInterval] = []
    private var periodMs = 1000.0 / 60
    /// Main thread busy time, ms, per refresh: what ended between one vsync callback and the next.
    private var workPerRefresh: [Double] = []
    private var workSinceTick = 0.0
    private var awakeSince: CFTimeInterval?

    static func measure(_ params: [String: Any]) async throws -> [String: Any] {
        let ms = min(max((params["ms"] as? NSNumber)?.doubleValue ?? 5000, 100), 60_000)
        let perf = Perf()
        let link = CADisplayLink(target: perf, selector: #selector(tick(_:)))
        // First to run on waking, last before sleeping (after Core Animation's commit at 2_000_000).
        let wakes = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.afterWaiting.rawValue, true, .min) { _, _ in
            MainActor.assumeIsolated { perf.awakeSince = CACurrentMediaTime() }
        }
        let sleeps = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue, true, .max) { _, _ in
            MainActor.assumeIsolated { perf.goesToSleep() }
        }
        History.activeStreams += 1
        CFRunLoopAddObserver(CFRunLoopGetMain(), wakes, .commonModes)
        CFRunLoopAddObserver(CFRunLoopGetMain(), sleeps, .commonModes)
        link.add(to: .main, forMode: .common)
        defer {
            link.invalidate()
            CFRunLoopObserverInvalidate(wakes)
            CFRunLoopObserverInvalidate(sleeps)
            History.activeStreams -= 1
        }
        try await Task.sleep(nanoseconds: UInt64(ms * 1_000_000))
        return perf.answer(ms: ms)
    }

    private func goesToSleep() {
        guard let since = awakeSince else { return }
        workSinceTick += (CACurrentMediaTime() - since) * 1000
        awakeSince = nil
    }

    @objc private func tick(_ link: CADisplayLink) {
        if !timestamps.isEmpty { workPerRefresh.append(workSinceTick) }
        workSinceTick = 0
        timestamps.append(link.timestamp)
        if link.targetTimestamp > link.timestamp { periodMs = (link.targetTimestamp - link.timestamp) * 1000 }
    }

    private func answer(ms: Double) -> [String: Any] {
        let gaps = zip(timestamps, timestamps.dropFirst()).map { ($1 - $0) * 1000 }
        // A gap of two periods is one refresh missed; a little over one period is timer noise, not a miss.
        let missed = gaps.reduce(0) { $0 + max(0, Int(($1 / periodMs).rounded()) - 1) }
        let work = workPerRefresh.sorted()
        let rounded = { (value: Double) in (value * 100).rounded() / 100 }
        // The vsync callbacks the main thread ran, a second: the rate the app can draw at.
        let fps: Any = gaps.isEmpty ? NSNull() : (Double(gaps.count) * 10_000 / gaps.reduce(0, +)).rounded() / 10
        return ["ms": ms, "refreshMs": rounded(periodMs), "refreshes": gaps.count, "missed": missed, "fps": fps,
                "worstGapMs": rounded(gaps.max() ?? 0), "drawn": NSNull(),
                "drawMeanMs": work.isEmpty ? NSNull() : rounded(work.reduce(0, +) / Double(work.count)),
                "drawP95Ms": work.isEmpty ? NSNull() : rounded(work[Int((Double(work.count) * 0.95).rounded(.up)) - 1]),
                "drawnOverRefresh": work.filter { $0 > periodMs }.count]
    }
}
#endif
