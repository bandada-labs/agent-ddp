#if DEBUG && canImport(UIKit)
import Foundation
import UIKit
import UIKit.UIGestureRecognizerSubclass

/// The flight recorder behind `ddp history`: every frame that differs from the one before, plus taps and screen
/// changes, for the last `keepMs`. It captures when the main run loop is about to sleep after a turn, so a still
/// app, whose run loop sleeps, costs nothing. Files are recordings of their own, one per `segmentMs`, gzipped when
/// closed, so the history of a run that crashed is still there on the next one.
final class History {
    static let shared = History()

    private static let segmentMs = 60_000.0
    private static let keepMs = 15 * 60_000.0
    /// While only busy elements change, or redraws keep changing nothing, capture at most this often. Anything else
    /// changing goes back to every turn.
    private static let busyIntervalMs = 250.0

    /// Streams capture every frame already and offer them here, so history does not capture twice. `perf.metrics`
    /// counts as one too, so History's cost is not in what it measures.
    static var activeStreams = 0

    // Main thread only.
    // The quiet gap must be longer than busyIntervalMs: while throttled, a busy element is only seen that often.
    private var tracker = ChangeTracker<[NSDictionary]>(busyAfterMs: 2_000, quietMs: 2 * busyIntervalMs)
    private var captureScheduled = false
    private var lastCaptureAt = 0.0
    private var lastScreen: String?
    private let watchedWindows = NSHashTable<UIWindow>.weakObjects()

    // Set once on the main thread in start(), then read by the writer queue.
    private var directory: URL!
    private var header: [String: Any] = [:]

    // Writer queue only.
    private let writer = DispatchQueue(label: "dev.agentddp.history")
    private var segment: HistorySegment?

    /// For `history.files` and `history.read`: the files as they are on disk, the open one included.
    func files() -> [String] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        return names.filter { $0.hasSuffix(".jsonl") || $0.hasSuffix(".jsonl.gz") }.sorted()
    }

    func read(_ name: String) throws -> Data {
        guard files().contains(name) else { throw Control.Failure(description: "No history file '\(name)'; history.files lists them") }
        return try Data(contentsOf: directory.appendingPathComponent(name))
    }

    /// Main thread, at launch.
    func start(directory: URL) {
        self.directory = directory
        header = FrameCapture.header(frames: 0).merging(["history": true]) { _, new in new }
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        writer.async { self.prune(now: Self.wallMs()) }
        event("launch")
        // Order after Core Animation's commit observer (2_000_000), so the turn's changes are already in the layers.
        // ponytail: animations Core Animation runs on its own (UIView.animate) do not wake the app, so only their
        // start and end are kept. Poll while layers have animationKeys if that bites.
        let observer = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue, true,
                                                          2_000_001) { [weak self] _, _ in self?.onTurnEnded() }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
    }

    /// A frame a stream already captured: kept here too, so recording does not pay twice.
    func offer(_ frame: [String: Any]) {
        lastCaptureAt = Self.uptimeMs()
        keep(frame, captureMs: nil)
    }

    /// `wall`: when it happened, ms since 1970; a touch is stamped when it went down, before what it caused.
    func event(_ kind: String, _ fields: [String: Any] = [:], wall: Double = History.wallMs()) {
        let line = fields.merging(["type": "event", "kind": kind]) { _, new in new }
        writer.async {
            // An app event can come before start(): there is no history yet to keep it in.
            guard self.directory != nil else { return }
            self.segment(at: wall).write(line, wall: wall)
        }
    }

    private func onTurnEnded() {
        guard !captureScheduled, Self.activeStreams == 0 else { return }
        let wait = tracker.throttled ? Self.busyIntervalMs - (Self.uptimeMs() - lastCaptureAt) : 0
        guard wait > 0 else { return captureNow() }
        // The wake-up itself ends a turn, and that turn captures.
        captureScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + wait / 1000) { [weak self] in self?.captureScheduled = false }
    }

    private func captureNow() {
        guard UIApplication.shared.applicationState != .background else { return }
        lastCaptureAt = Self.uptimeMs()
        let started = CACurrentMediaTime()
        let frame = FrameCapture.frame(index: 0, milliseconds: 0)
        let captureMs = (CACurrentMediaTime() - started) * 1000
        watchTouches()
        noteScreen()
        keep(frame, captureMs: captureMs)
    }

    private func keep(_ frame: [String: Any], captureMs: Double?) {
        let changed = tracker.changed(FrameSignatures.of(frame), at: Self.uptimeMs())
        guard !changed.isEmpty else { return }
        let wall = Self.wallMs()
        writer.async { self.segment(at: wall).writeFrame(frame, wall: wall, captureMs: captureMs) }
    }

    private func watchTouches() {
        for window in FrameCapture.visibleWindows() where !watchedWindows.contains(window) {
            watchedWindows.add(window)
            window.addGestureRecognizer(TouchWatcher())
        }
    }

    private func noteScreen() {
        guard let name = Self.screenName(), name != lastScreen else { return }
        lastScreen = name
        event("screen", ["name": name])
    }

    /// The title of what is on top, or its class when it has none (SwiftUI screens share one hosting class).
    private static func screenName() -> String? {
        var controller = FrameCapture.visibleWindows().last(where: \.isKeyWindow)?.rootViewController
        while let current = controller {
            if let presented = current.presentedViewController, !presented.isBeingDismissed {
                controller = presented
            } else if let top = (current as? UINavigationController)?.topViewController {
                controller = top
            } else if let selected = (current as? UITabBarController)?.selectedViewController {
                controller = selected
            } else {
                break
            }
        }
        guard let controller else { return nil }
        if let title = controller.navigationItem.title ?? controller.title, !title.isEmpty { return title }
        return String(describing: type(of: controller)).components(separatedBy: "<")[0]
    }

    private func segment(at wall: Double) -> HistorySegment {
        if let current = segment, wall - current.startedAt < Self.segmentMs { return current }
        segment?.close()
        prune(now: wall)
        let next = HistorySegment(directory: directory, startedAt: wall, header: header)
        segment = next
        return next
    }

    private func prune(now: Double) {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        for name in names {
            let startedAt = Double(name.components(separatedBy: ".")[0]) ?? 0
            if startedAt < now - Self.keepMs - Self.segmentMs {
                try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
            }
        }
    }

    static func wallMs() -> Double { (Date().timeIntervalSince1970 * 1000).rounded() }
    private static func uptimeMs() -> Double { CACurrentMediaTime() * 1000 }
}

/// One file of history: `<startedAt>.jsonl` while open, `<startedAt>.jsonl.gz` once closed.
private final class HistorySegment {
    let startedAt: Double
    private let file: URL
    private let output: FileHandle?
    private var frames = 0
    private var captures = 0
    private var totalCaptureMs = 0.0
    private var slowestCaptureMs = 0.0

    init(directory: URL, startedAt: Double, header: [String: Any]) {
        self.startedAt = startedAt
        file = directory.appendingPathComponent("\(Int64(startedAt)).jsonl")
        FileManager.default.createFile(atPath: file.path, contents: nil)
        output = try? FileHandle(forWritingTo: file)
        line(header.merging(["startedAt": startedAt]) { _, new in new })
    }

    func writeFrame(_ frame: [String: Any], wall: Double, captureMs: Double?) {
        if let captureMs {
            captures += 1
            totalCaptureMs += captureMs
            slowestCaptureMs = max(slowestCaptureMs, captureMs)
        }
        write(frame.merging(["i": frames]) { _, new in new }, wall: wall)
        frames += 1
    }

    func write(_ entry: [String: Any], wall: Double) {
        line(entry.merging(["t": wall - startedAt]) { _, new in new })
    }

    func close() {
        line(["type": "end", "frames": frames, "averageCaptureMs": round2(totalCaptureMs / Double(max(captures, 1))),
              "slowestCaptureMs": round2(slowestCaptureMs)])
        try? output?.close()
        if let plain = try? Data(contentsOf: file), let packed = gzip(plain),
           (try? packed.write(to: file.appendingPathExtension("gz"))) != nil {
            try? FileManager.default.removeItem(at: file)
        }
    }

    // FileHandle does not buffer, so `ddp history` sees the open segment up to now.
    private func line(_ entry: [String: Any]) {
        if let data = jsonLine(entry) { output?.write(data) }
    }
}

/// Taps and swipes in window coordinates (the recording's). It only watches: it never wins, blocks or delays a touch.
private final class TouchWatcher: UIGestureRecognizer {
    private static let slop: CGFloat = 10
    private var start: CGPoint?
    private var startedAt = 0.0

    init() {
        super.init(target: nil, action: nil)
        cancelsTouchesInView = false
        delaysTouchesBegan = false
        delaysTouchesEnded = false
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        if start == nil, let touch = touches.first {
            start = touch.location(in: nil)
            startedAt = History.wallMs()
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
        let stillDown = event.allTouches?.contains { $0.phase != .ended && $0.phase != .cancelled } ?? false
        guard let start, let touch = touches.first, !stillDown else { return }
        let end = touch.location(in: nil)
        let moved = hypot(end.x - start.x, end.y - start.y) > Self.slop
        var fields: [String: Any] = ["x": round2(start.x), "y": round2(start.y)]
        if moved {
            fields["toX"] = round2(end.x)
            fields["toY"] = round2(end.y)
        }
        History.shared.event(moved ? "swipe" : "tap", fields, wall: startedAt)
        state = .failed
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
        state = .failed
    }

    override func reset() {
        start = nil
    }

    override func canPrevent(_ preventedGestureRecognizer: UIGestureRecognizer) -> Bool { false }
    override func canBePrevented(by preventingGestureRecognizer: UIGestureRecognizer) -> Bool { false }
}

/// gzip (RFC 1952) around Foundation's raw DEFLATE, the format the CLI and `gunzip` read.
func gzip(_ data: Data) -> Data? {
    guard let deflated = try? (data as NSData).compressed(using: .zlib) as Data else { return nil }
    var packed = Data([0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF])
    packed.append(deflated)
    withUnsafeBytes(of: crc32(data).littleEndian) { packed.append(contentsOf: $0) }
    withUnsafeBytes(of: UInt32(truncatingIfNeeded: data.count).littleEndian) { packed.append(contentsOf: $0) }
    return packed
}

private let crcTable: [UInt32] = (0..<256).map { index in
    (0..<8).reduce(UInt32(index)) { value, _ in value & 1 == 1 ? 0xEDB8_8320 ^ (value >> 1) : value >> 1 }
}

func crc32(_ data: Data) -> UInt32 {
    ~data.reduce(~UInt32(0)) { crc, byte in crcTable[Int((crc ^ UInt32(byte)) & 0xFF)] ^ (crc >> 8) }
}

#endif
