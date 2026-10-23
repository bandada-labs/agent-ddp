#if DEBUG && canImport(UIKit)
import Foundation
import Network
import UIKit

/// The live endpoint `ddp connect` attaches to, like a DevTools port.
/// Bound to 127.0.0.1 only. Clients must send the token the app writes to its own container
/// (`Library/Caches/agent-ddp/live.json`), so only someone who can read the app's files can connect.
final class LiveServer {
    static let shared = LiveServer()

    private let queue = DispatchQueue(label: "dev.agentddp.live")
    private let token = UUID().uuidString
    private var listener: NWListener?

    func start(directory: URL) {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
        guard listener == nil, let listener = try? NWListener(using: parameters) else { return }
        let endpointFile = directory.appendingPathComponent("live.json")
        listener.stateUpdateHandler = { [token] state in
            guard case .ready = state, let port = listener.port?.rawValue,
                  let data = try? JSONSerialization.data(withJSONObject: ["port": Int(port), "token": token])
            else { return }
            try? data.write(to: endpointFile, options: .atomic)
        }
        listener.newConnectionHandler = { [queue, token] connection in
            LiveSession(connection: connection, token: token, queue: queue).start()
        }
        listener.start(queue: queue)
        self.listener = listener
    }
}

/// One client connection: a token line, then one command, answered with JSON lines that end with
/// {"type":"end"}. Streams (frames, network) run until the client closes the connection.
private final class LiveSession {
    private let connection: NWConnection
    private let token: String
    private let queue: DispatchQueue
    private let lock = NSLock()
    private var buffer = Data()
    private var authenticated = false
    private var isOpen = true
    private var stream: FrameStream?
    private var networkWatch: UUID?

    init(connection: NWConnection, token: String, queue: DispatchQueue) {
        self.connection = connection
        self.token = token
        self.queue = queue
    }

    // The connection's handlers keep the session alive until it closes.
    func start() {
        connection.stateUpdateHandler = { [self] state in
            if case .failed = state { close() }
            if case .cancelled = state { close() }
        }
        connection.start(queue: queue)
        receive()
    }

    private func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [self] data, _, isComplete, error in
            if let data { buffer.append(data) }
            while let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer.subdata(in: buffer.startIndex..<newline)
                buffer.removeSubrange(buffer.startIndex...newline)
                handle(line)
            }
            if isComplete || error != nil { close() } else { receive() }
        }
    }

    private func handle(_ line: Data) {
        guard let message = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] else {
            return fail("each line must be a JSON object")
        }
        guard authenticated else {
            guard message["token"] as? String == token else { return fail("wrong token; run `ddp connect` again") }
            authenticated = true
            return
        }
        switch message["cmd"] as? String {
        case "snapshot":
            DispatchQueue.main.async { [self] in
                _ = send(FrameCapture.header(frames: 1))
                _ = send(FrameCapture.frame(index: 0, milliseconds: 0))
                finish()
            }
        case "stream":
            let frames = message["frames"] as? Int ?? 0
            DispatchQueue.main.async { [self] in
                stream = FrameStream(frames: frames, send: { [weak self] in self?.send($0) ?? false },
                                     finished: { [weak self] in self?.finish(sendingEnd: false) })
                stream?.start()
            }
        case "eval":
            DispatchQueue.main.async { [self] in
                _ = send(LiveEval.evaluate(name: message["id"] as? String ?? "", path: message["path"] as? String ?? ""))
                finish()
            }
        case "time":
            DispatchQueue.main.async { [self] in
                _ = send(TimeControl.handle(action: message["action"] as? String ?? "", value: message["value"] as? Double))
                finish()
            }
        case "net":
            NetworkLog.shared.summaries().forEach { _ = send($0) }
            finish()
        case "net-get":
            _ = send(NetworkLog.shared.entry(id: message["id"] as? String ?? "")
                     ?? ["type": "error", "error": "no request with that id; see `ddp net list`"])
            finish()
        case "net-watch":
            networkWatch = NetworkLog.shared.watch { [weak self] summary in self?.send(summary) ?? false }
        case let other:
            fail("unknown command \(other ?? "nil")")
        }
    }

    private func send(_ line: [String: Any]) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard isOpen, let data = jsonLine(line) else { return false }
        connection.send(content: data, completion: .contentProcessed { [weak self] error in
            if error != nil { self?.close() }
        })
        return true
    }

    private func finish(sendingEnd: Bool = true) {
        if sendingEnd { _ = send(["type": "end"]) }
        connection.send(content: nil, isComplete: true, completion: .contentProcessed { [connection] _ in
            connection.cancel()
        })
    }

    private func fail(_ error: String) {
        _ = send(["type": "error", "error": error])
        finish()
    }

    private func close() {
        lock.lock()
        let wasOpen = isOpen
        isOpen = false
        lock.unlock()
        guard wasOpen else { return }
        if let networkWatch { NetworkLog.shared.stopWatching(networkWatch) }
        DispatchQueue.main.async { [self] in
            stream?.stop()
            stream = nil
        }
        connection.cancel()
    }
}

/// Read-only property reads for `ddp eval <element> <path>`:
/// `element.<key path>` on the accessibility element, `layer.<key path>` on the topmost layer that
/// matches the element's frame (presentation values, so mid-animation).
private enum LiveEval {
    static func evaluate(name: String, path: String) -> [String: Any] {
        let parts = path.split(separator: ".", maxSplits: 1).map(String.init)
        guard parts.count == 2, ["element", "layer"].contains(parts[0]) else {
            return error("path is element.<key path> or layer.<key path>, e.g. layer.shadowRadius")
        }
        let windows = FrameCapture.visibleWindows()
        guard let element = AccessibilityReader.object(named: name, in: windows) else {
            return error("no element '\(name)'")
        }
        if parts[0] == "element" { return value(of: element, keyPath: parts[1]) }
        guard let layer = LayerReader.topLayer(matching: element.accessibilityFrame, in: windows) else {
            return error("no layer matches the frame of '\(name)'")
        }
        return value(of: layer.presentation() ?? layer, keyPath: parts[1])
    }

    private static func value(of object: NSObject, keyPath: String) -> [String: Any] {
        var current: Any? = object
        for key in keyPath.split(separator: ".").map(String.init) {
            // KVC on an unknown key throws an Objective-C exception Swift can't catch, so ask first.
            // CALayer answers any key (nil when unset).
            guard let target = current as? NSObject,
                  target is CALayer || target.responds(to: NSSelectorFromString(key)) else {
                return error("'\(key)' is not a property of \(type(of: current ?? "nil"))")
            }
            current = target.value(forKey: key)
        }
        return ["type": "value", "path": keyPath, "value": jsonValue(current)]
    }

    private static func jsonValue(_ value: Any?) -> Any {
        guard let value else { return NSNull() }
        if let number = value as? NSNumber { return number }
        if let string = value as? String { return string }
        if CFGetTypeID(value as CFTypeRef) == CGColor.typeID { return hex((value as! CGColor)) ?? "transparent" }
        return String(describing: value)
    }

    private static func error(_ message: String) -> [String: Any] { ["type": "error", "error": message] }
}
#endif
