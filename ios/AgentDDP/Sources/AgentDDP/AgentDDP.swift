import Foundation
import QuartzCore
#if DEBUG && canImport(UIKit)
import UIKit
#endif

/// Records what the app drew, frame by frame, when the `ddp` CLI asks for it.
/// Everything below compiles to an empty `start()` outside Debug builds.
public enum AgentDDP {
    public static func start() {
        #if DEBUG && canImport(UIKit)
        FrameRecorder.shared.listenForRequests()
        TouchRadius.watch()
        ControlServer.shared.start(directory: FrameRecorder.shared.outputDirectory)
        History.shared.start(directory: FrameRecorder.shared.outputDirectory.appendingPathComponent("history"))
        #endif
    }

    /// Call from the app's Debug network decorator once a request finishes, so `ddp net` can list it.
    /// Headers, query items and JSON keys that look secret are redacted before anything is kept.
    public static func logRequest(_ request: URLRequest, response: URLResponse?, body: Data?, error: Error?,
                                  startedAt: Date, finishedAt: Date = Date()) {
        #if DEBUG && canImport(UIKit)
        NetworkLog.shared.add(request: request, response: response, body: body, error: error,
                              startedAt: startedAt, finishedAt: finishedAt)
        #endif
    }

    /// Sends `request` with the app's own client, for its Debug network decorator: `ddp net` sees it,
    /// and a client can pause, change, answer or fail it before `send` runs. `net.send` uses the last `send` given here.
    /// Outside Debug it only calls `send`.
    public static func send(_ request: URLRequest,
                            using send: @escaping (URLRequest) async throws -> (Data, URLResponse)) async throws -> (Data, URLResponse) {
        #if DEBUG && canImport(UIKit)
        AppClient.shared.remember(send)
        return try await logged(request, using: send)
        #else
        return try await send(request)
        #endif
    }

    /// Adds a named action for agents (`app.commands`, `app.run`): log in a test user, reset state, open a screen.
    /// `run` gets the agent's params and returns a JSON value (dictionary, array, string, number, Bool or nil). It runs
    /// off the main thread; UI work goes through `MainActor.run`. Outside Debug it does nothing.
    public static func command(_ name: String, _ run: @escaping ([String: Any]) async throws -> Any?) {
        #if DEBUG && canImport(UIKit)
        Commands.shared.add(name, run)
        #endif
    }

    #if DEBUG && canImport(UIKit)
    static func logged(_ request: URLRequest,
                       using send: (URLRequest) async throws -> (Data, URLResponse)) async throws -> (Data, URLResponse) {
        let startedAt = Date()
        var sent = request
        do {
            let result: (Data, URLResponse)
            switch await Fetch.shared.pause(request) {
            case .proceed(let changed):
                sent = changed
                result = try await send(changed)
            case .fulfill(let body, let response): result = (body, response)
            case .fail(let error): throw error
            }
            logRequest(sent, response: result.1, body: result.0, error: nil, startedAt: startedAt)
            return result
        } catch {
            logRequest(sent, response: nil, body: nil, error: error, startedAt: startedAt)
            throw error
        }
    }
    #endif

    /// Seconds on ddp's controllable clock (`ddp time pause|step|seek|rate`). An app's Debug clock port can read
    /// this instead of the system clock so its own timelines pause, step and rewind with ddp.
    /// Outside Debug it is CACurrentMediaTime().
    public static func elapsedTime() -> TimeInterval {
        #if DEBUG && canImport(UIKit)
        return Thread.isMainThread ? TimeControl.now() : DispatchQueue.main.sync { TimeControl.now() }
        #else
        return CACurrentMediaTime()
        #endif
    }

    /// Literal values (the app's dev secret, a test token) to replace with "[redacted]" wherever they show up.
    public static func redact(values: [String]) {
        #if DEBUG && canImport(UIKit)
        NetworkLog.shared.redact(values: values)
        #endif
    }
}

#if DEBUG && canImport(UIKit)

/// Starts a recording when the CLI posts the Darwin notification (offline mode: the file in the app container).
final class FrameRecorder {
    static let shared = FrameRecorder()

    private static let requestNotificationName = "dev.agentddp.record"
    private static let defaultFrameCount = 120
    private static let maxFrameCount = 600

    let outputDirectory = FileManager.default
        .urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("agent-ddp")
    private var stream: FrameStream?
    private var output: FileHandle?

    private var partialFile: URL { outputDirectory.appendingPathComponent("recording.partial") }
    private var finishedFile: URL { outputDirectory.appendingPathComponent("recording.jsonl") }
    private var requestFile: URL { outputDirectory.appendingPathComponent("request.json") }

    func listenForRequests() {
        AccessibilityReader.enableAutomationTree()
        try? FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(), nil,
            { _, _, _, _, _ in DispatchQueue.main.async { FrameRecorder.shared.beginRecording() } },
            Self.requestNotificationName as CFString, nil, .deliverImmediately)
    }

    private func beginRecording() {
        guard stream == nil else { return }
        let request = (try? Data(contentsOf: requestFile))
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        let frames = min(max(request?["frames"] as? Int ?? Self.defaultFrameCount, 1), Self.maxFrameCount)
        FileManager.default.createFile(atPath: partialFile.path, contents: nil)
        output = try? FileHandle(forWritingTo: partialFile)
        stream = FrameStream(frames: frames, send: { [weak self] line in
            guard let self, let output = self.output, let data = jsonLine(line) else { return false }
            output.write(data)
            return true
        }, finished: { [weak self] in self?.finishRecording() })
        stream?.start()
    }

    private func finishRecording() {
        try? output?.close()
        output = nil
        stream = nil
        try? FileManager.default.removeItem(at: finishedFile)
        try? FileManager.default.moveItem(at: partialFile, to: finishedFile)
    }
}

/// Captures display frames and hands each line (header, frames, end) to `send`, for a file or a live client.
/// `frames` 0 runs until `send` returns false or `stop()` is called.
final class FrameStream {
    private let frameLimit: Int
    private let send: ([String: Any]) -> Bool
    private let finished: () -> Void
    private var displayLink: CADisplayLink?
    private var frameIndex = 0
    private var firstTimestamp: CFTimeInterval = 0
    private var slowestCaptureMs = 0.0
    private var totalCaptureMs = 0.0

    init(frames: Int, send: @escaping ([String: Any]) -> Bool, finished: @escaping () -> Void = {}) {
        frameLimit = frames
        self.send = send
        self.finished = finished
    }

    /// Main thread only.
    func start() {
        guard send(FrameCapture.header(frames: frameLimit)) else { return finished() }
        let link = CADisplayLink(target: self, selector: #selector(captureFrame(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
        History.activeStreams += 1
    }

    func stop() {
        guard let link = displayLink else { return }
        link.invalidate()
        displayLink = nil
        History.activeStreams -= 1
        _ = send(["type": "end", "frames": frameIndex, "slowestCaptureMs": round2(slowestCaptureMs),
                  "averageCaptureMs": round2(totalCaptureMs / Double(max(frameIndex, 1)))])
        finished()
    }

    @objc private func captureFrame(_ link: CADisplayLink) {
        if frameIndex == 0 { firstTimestamp = link.timestamp }
        let captureStart = CACurrentMediaTime()
        let line = FrameCapture.frame(index: frameIndex, milliseconds: (link.timestamp - firstTimestamp) * 1000)
        let captureMs = (CACurrentMediaTime() - captureStart) * 1000
        slowestCaptureMs = max(slowestCaptureMs, captureMs)
        totalCaptureMs += captureMs
        frameIndex += 1
        History.shared.offer(line)
        if !send(line) || (frameLimit > 0 && frameIndex >= frameLimit) { stop() }
    }
}

enum FrameCapture {
    /// The DDP protocol version (docs/protocol.md). Raise it only when a field changes meaning or goes away.
    static let protocolVersion = 2

    static func header(frames: Int) -> [String: Any] {
        ["type": "header", "protocol": protocolVersion, "platform": "ios", "unit": "pt", "scale": UIScreen.main.scale,
         "frames": frames,
         "startedAt": (Date().timeIntervalSince1970 * 1000).rounded()]
    }

    /// Main thread only.
    static func frame(index: Int, milliseconds: Double) -> [String: Any] {
        let windows = visibleWindows()
        return ["type": "frame", "i": index, "t": round2(milliseconds),
                "elements": windows.flatMap(AccessibilityReader.elements(in:)),
                "nodes": LayerReader.paintedNodes(in: windows)]
    }

    static func visibleWindows() -> [UIWindow] {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
            .filter { !$0.isHidden }
            .sorted { $0.windowLevel < $1.windowLevel }
    }
}

func jsonLine(_ line: [String: Any]) -> Data? {
    guard var data = try? JSONSerialization.data(withJSONObject: line) else { return nil }
    data.append(0x0A)
    return data
}

#endif
