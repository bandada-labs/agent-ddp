#if DEBUG && canImport(UIKit)
import CryptoKit
import Foundation
import Network
import UIKit

/// The control endpoint (ADR 0008): JSON-RPC over a WebSocket, with method names taken from the app, not from a
/// browser. Bound to 127.0.0.1; the port goes to `Library/Caches/agent-ddp/control.json`. The Host check stops DNS
/// rebinding and the Origin check stops web pages.
final class ControlServer {
    static let shared = ControlServer()

    private let queue = DispatchQueue(label: "dev.agentddp.control")
    private var listener: NWListener?

    func start(directory: URL) {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
        guard listener == nil, let listener = try? NWListener(using: parameters) else { return }
        let app = ControlApp(bundleIdentifier: Bundle.main.bundleIdentifier ?? "app",
                             name: Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
                                 ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "app")
        let endpointFile = directory.appendingPathComponent("control.json")
        listener.stateUpdateHandler = { state in
            guard case .ready = state, let port = listener.port?.rawValue,
                  let data = try? JSONSerialization.data(withJSONObject: ["port": Int(port)]) else { return }
            try? data.write(to: endpointFile, options: .atomic)
        }
        listener.newConnectionHandler = { [queue] connection in
            ControlConnection(connection: connection, app: app, queue: queue).start()
        }
        listener.start(queue: queue)
        self.listener = listener
    }
}

struct ControlApp {
    let bundleIdentifier: String
    let name: String
}

/// One connection: a WebSocket upgrade, then messages until the client closes.
private final class ControlConnection {
    private let connection: NWConnection
    private let app: ControlApp
    private let queue: DispatchQueue
    private var buffer = Data()
    private var session: ControlSession?

    init(connection: NWConnection, app: ControlApp, queue: DispatchQueue) {
        self.connection = connection
        self.app = app
        self.queue = queue
    }

    // The connection's handlers keep the object alive until it closes.
    func start() {
        connection.stateUpdateHandler = { [connection] state in
            if case .failed = state { connection.cancel() }
        }
        connection.start(queue: queue)
        receive()
    }

    private func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [self] data, _, isComplete, error in
            if let data { buffer.append(data) }
            if buffer.count > WebSocket.maxMessageBytes { return connection.cancel() }
            if session == nil, let request = HttpRequest.parse(&buffer) { upgrade(request) }
            while session != nil, let frame = WebSocket.parse(&buffer) { handle(frame) }
            if isComplete || error != nil {
                session?.close()
                connection.cancel()
            } else {
                receive()
            }
        }
    }

    private func upgrade(_ request: HttpRequest) {
        guard Control.hostAllowed(request.headers["host"] ?? "localhost") else { return respond("403 Forbidden", "Host must be localhost") }
        guard request.headers["upgrade"]?.lowercased() == "websocket" else { return respond("426 Upgrade Required", "Use a WebSocket") }
        guard let key = request.headers["sec-websocket-key"] else { return respond("400 Bad Request", "No Sec-WebSocket-Key") }
        // A web page always sends its origin; tools do not.
        guard request.headers["origin"] == nil else { return respond("403 Forbidden", "Web pages may not connect") }
        let head = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Accept: \(WebSocket.acceptKey(key))\r\n\r\n"
        connection.send(content: Data(head.utf8), completion: .contentProcessed { _ in })
        session = ControlSession(app: app) { [connection] message in
            connection.send(content: WebSocket.frame(opcode: WebSocket.text, payload: Data(message.utf8)),
                            completion: .contentProcessed { _ in })
        }
    }

    private func handle(_ frame: WebSocket.Frame) {
        switch frame.opcode {
        case WebSocket.text: session?.receive(String(decoding: frame.payload, as: UTF8.self))
        case WebSocket.ping:
            connection.send(content: WebSocket.frame(opcode: WebSocket.pong, payload: frame.payload),
                            completion: .contentProcessed { _ in })
        case WebSocket.close:
            session?.close()
            session = nil
            connection.send(content: WebSocket.frame(opcode: WebSocket.close, payload: frame.payload),
                            completion: .contentProcessed { [connection] _ in connection.cancel() })
        default: break
        }
    }

    private func respond(_ status: String, _ body: String) {
        let bytes = Data(body.utf8)
        let head = "HTTP/1.1 \(status)\r\nContent-Type: text/plain\r\nContent-Length: \(bytes.count)\r\nConnection: close\r\n\r\n"
        connection.send(content: Data(head.utf8) + bytes, isComplete: true,
                        completion: .contentProcessed { [connection] _ in connection.cancel() })
    }
}

/// Answers the messages of one client. Every handler calls a native module and only converts JSON.
final class ControlSession {
    private let app: ControlApp
    private let send: (String) -> Void
    private var networkWatch: UUID?
    private var appLog: AppLog?
    // Main thread only.
    private var screenStream: FrameStream?

    private lazy var methods: [String: ([String: Any]) throws -> Any] = [
        "app.info": { [app] _ in Control.info(app) },
        "app.commands": { _ in ["commands": Commands.shared.names()] },
        "screen.elements": { _ in ["elements": onMain { FrameCapture.visibleWindows().flatMap(AccessibilityReader.elements(in:)) }] },
        "screen.capture": { params in try Control.capture(params) },
        "app.hooks": { _ in ["hooks": Hooks.report()] },
        "screen.pixels": { params in
            let rects = (params["rects"] as? [[Double]] ?? []).filter { $0.count == 4 }
                .map { CGRect(x: $0[0], y: $0[1], width: $0[2], height: $0[3]) }
            return onMain { Pixels.answer(rects) }
        },
        "screen.hit": { params in
            let touch = CGFloat(params["touch"] as? Double ?? 0)
            return ["elements": try onMain { Result { try Hit.check(name: params["id"] as? String, minimumTouch: touch) } }.get()]
        },
        "screen.frame": { _ in onMain { FrameCapture.frame(index: 0, milliseconds: 0) } },
        // The header, frame and end lines of a recording, as `screen.stream` events.
        "screen.watch": { [unowned self] params in
            let frames = params["frames"] as? Int ?? 0
            onMain {
                screenStream?.stop()
                screenStream = FrameStream(frames: frames, send: { [weak self] line in
                    guard let self else { return false }
                    event("screen.stream", line)
                    return true
                })
                screenStream?.start()
            }
            return [:]
        },
        "screen.unwatch": { [unowned self] _ in onMain { screenStream?.stop() }; return [:] },
        "screen.value": { params in
            let path = params["path"] as? String ?? ""
            // LiveEval answers the key path without its `layer.` or `element.` scope; this names what was asked.
            return try Control.answer(onMain { LiveEval.evaluate(name: params["id"] as? String ?? "", path: path) })
                .merging(["path": path]) { _, asked in asked }
        },
        "time.pause": Self.time("pause"),
        "time.resume": Self.time("resume"),
        "time.step": Self.time("step", "frames"),
        "time.rate": Self.time("rate", "rate"),
        "time.seek": Self.time("seek", "ms"),
        "time.status": Self.time("status"),
        "history.files": { _ in ["files": History.shared.files()] },
        "history.read": { params in ["data": try History.shared.read(params["file"] as? String ?? "").base64EncodedString()] },
        "input.touch": { params in
            let point = CGPoint(x: params["x"] as? Double ?? 0, y: params["y"] as? Double ?? 0)
            switch params["phase"] as? String {
            case "down", "move": try onMain { Result { try Input.touch(point) } }.get()
            case "up": try onMain { Result { try Input.touch(nil) } }.get()
            default: throw Control.Failure(description: "phase is down, move or up")
            }
            return [:]
        },
        "input.type": { params in onMain { Input.type(params["text"] as? String ?? "") }; return [:] },
        "input.key": { params in onMain { Input.press(params["key"] as? String ?? "") }; return [:] },
        "net.list": { _ in ["requests": NetworkLog.shared.summaries()] },
        "net.get": { params in
            let id = params["id"] as? String ?? ""
            guard let entry = NetworkLog.shared.entry(id: id) else { throw Control.Failure(description: "No request \(id)") }
            return entry
        },
        "net.watch": { [unowned self] _ in
            stopNetwork()
            networkWatch = NetworkLog.shared.watch { [weak self] summary in
                guard let self else { return false }
                event("net.request", summary)
                return true
            }
            return [:]
        },
        "net.unwatch": { [unowned self] _ in stopNetwork(); return [:] },
        "net.intercept": { [unowned self] params in
            Fetch.shared.intercept(owner: self, params: params) { [weak self] paused in self?.event("net.paused", paused) }
            return [:]
        },
        "net.release": { [unowned self] _ in Fetch.shared.release(owner: self); return [:] },
        "net.continue": { params in try Fetch.shared.proceed(params); return [:] },
        "net.respond": { params in try Fetch.shared.respond(params); return [:] },
        "net.fail": { params in try Fetch.shared.fail(params); return [:] },
        "log.watch": { [unowned self] _ in
            appLog?.stop()
            appLog = AppLog { [weak self] entry in self?.event("log.entry", entry) }
            appLog?.start()
            return [:]
        },
        "log.unwatch": { [unowned self] _ in appLog?.stop(); appLog = nil; return [:] },
        "prefs.files": { _ in ["files": Preferences.files()] },
        "prefs.read": { params in ["values": try Preferences.read(Self.file(params))] },
        "prefs.write": { params in
            guard let key = params["key"] as? String, let value = params["value"] as? String else {
                throw Control.Failure(description: "prefs.write needs key and value")
            }
            try Preferences.write(Self.file(params), key: key, text: value)
            return [:]
        },
        "prefs.remove": { params in try Preferences.remove(Self.file(params), key: params["key"] as? String ?? ""); return [:] },
        "prefs.clear": { params in try Preferences.clear(Self.file(params)); return [:] },
    ]

    /// These wait on the app's code or the network, apart from the other messages, so a request they send can still be
    /// paused and continued.
    private let waitingMethods: [String: ([String: Any]) async throws -> Any] = [
        "app.run": { try await Commands.shared.run($0) },
        "net.send": { try await AppClient.shared.send($0) },
    ]

    init(app: ControlApp, send: @escaping (String) -> Void) {
        self.app = app
        self.send = send
    }

    /// The connection closed: stop feeding it events and let paused requests go on.
    func close() {
        DispatchQueue.main.async { [self] in screenStream?.stop() }
        stopNetwork()
        Fetch.shared.release(owner: self)
        appLog?.stop()
    }

    private func stopNetwork() {
        if let networkWatch { NetworkLog.shared.stopWatching(networkWatch) }
        networkWatch = nil
    }

    private static func time(_ action: String, _ key: String? = nil) -> ([String: Any]) throws -> Any {
        { params in try Control.answer(onMain { TimeControl.handle(action: action, value: key.flatMap { params[$0] as? Double }) }) }
    }

    private static func file(_ params: [String: Any]) -> String? { (params["file"] as? String).flatMap { $0.isEmpty ? nil : $0 } }

    private func event(_ method: String, _ params: [String: Any]) { reply(["method": method, "params": params]) }

    func receive(_ text: String) {
        guard let message = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] else {
            return reply(Control.error(id: nil, code: -32700, message: "Not JSON"))
        }
        guard let id = message["id"] as? Int else { return reply(Control.error(id: nil, code: -32600, message: "No integer id")) }
        let method = message["method"] as? String ?? ""
        let params = message["params"] as? [String: Any] ?? [:]
        if let waiting = waitingMethods[method] {
            Task {
                do {
                    reply(["id": id, "result": try await waiting(params)])
                } catch {
                    reply(Control.error(id: id, code: -32000, message: "\(error)"))
                }
            }
            return
        }
        guard let handler = methods[method] else {
            return reply(Control.error(id: id, code: -32601, message: "Unknown method '\(method)'"))
        }
        do {
            reply(["id": id, "result": try handler(params)])
        } catch {
            reply(Control.error(id: id, code: -32000, message: "\(error)"))
        }
    }

    private func reply(_ answer: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: answer, options: .withoutEscapingSlashes) else { return }
        send(String(decoding: data, as: UTF8.self))
    }
}

func onMain<T>(_ work: () -> T) -> T { Thread.isMainThread ? work() : DispatchQueue.main.sync(execute: work) }

enum Control {
    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    /// An answer line (`type` value or error) as a result: its `type` goes, and an error line throws.
    static func answer(_ line: [String: Any]) throws -> [String: Any] {
        if line["type"] as? String == "error" { throw Failure(description: line["error"] as? String ?? "Failed") }
        return line.filter { $0.key != "type" }
    }

    static func info(_ app: ControlApp) -> [String: Any] {
        ["protocol": FrameCapture.protocolVersion, "platform": "ios", "app": app.bundleIdentifier, "name": app.name,
         "unit": "pt", "scale": onMain { UIScreen.main.scale }]
    }

    /// Every window of the app as one image in pixels, like a frame capture. `clip` is `[x, y, width, height]` in points.
    static func capture(_ params: [String: Any]) throws -> [String: Any] {
        let format = params["format"] as? String ?? "png"
        guard ["png", "jpeg"].contains(format) else { throw Failure(description: "format is png or jpeg") }
        let quality = (params["quality"] as? Double ?? 80) / 100
        let data: Data? = onMain {
            let windows = FrameCapture.visibleWindows()
            guard let screen = windows.first?.screen else { return nil }
            let clip = (params["clip"] as? [Double]).flatMap { $0.count == 4 ? CGRect(x: $0[0], y: $0[1], width: $0[2], height: $0[3]) : nil }
                ?? screen.bounds
            let renderFormat = UIGraphicsImageRendererFormat()
            renderFormat.scale = screen.scale
            let image = UIGraphicsImageRenderer(bounds: clip, format: renderFormat).image { _ in
                windows.forEach { $0.drawHierarchy(in: $0.frame, afterScreenUpdates: false) }
            }
            return format == "png" ? image.pngData() : image.jpegData(compressionQuality: quality)
        }
        guard let data else { throw Failure(description: "No window to capture") }
        return ["data": data.base64EncodedString()]
    }

    /// A page that rebinds its own domain to 127.0.0.1 still sends that domain as Host.
    static func hostAllowed(_ host: String) -> Bool {
        let name = host.hasPrefix("[") ? String(host.prefix { $0 != "]" }.dropFirst())
            : String(host.split(separator: ":").first ?? "")
        return ["localhost", "127.0.0.1", "::1"].contains(name)
    }

    static func error(id: Int?, code: Int, message: String) -> [String: Any] {
        var answer: [String: Any] = ["error": ["code": code, "message": message]]
        if let id { answer["id"] = id }
        return answer
    }
}

struct HttpRequest {
    let path: String
    let headers: [String: String]

    /// Takes the request off the front of `buffer` once its blank line has arrived, leaving any WebSocket bytes.
    static func parse(_ buffer: inout Data) -> HttpRequest? {
        guard let end = buffer.range(of: Data("\r\n\r\n".utf8)) else { return nil }
        let lines = String(decoding: buffer[buffer.startIndex..<end.lowerBound], as: UTF8.self)
            .components(separatedBy: "\r\n")
        buffer.removeSubrange(buffer.startIndex..<end.upperBound)
        let target = lines.first?.split(separator: " ").dropFirst().first.map(String.init) ?? "/"
        var headers: [String: String] = [:]
        for line in lines.dropFirst() {
            guard let colon = line.firstIndex(of: ":") else { continue }
            headers[line[..<colon].trimmingCharacters(in: .whitespaces).lowercased()] =
                line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }
        return HttpRequest(path: String(target.split(separator: "?").first ?? ""), headers: headers)
    }
}

/// RFC 6455, only what the control endpoint needs: text, ping, pong and close frames. Clients mask, the server does not.
enum WebSocket {
    static let continuation: UInt8 = 0
    static let text: UInt8 = 1
    static let close: UInt8 = 8
    static let ping: UInt8 = 9
    static let pong: UInt8 = 10
    static let maxMessageBytes = 64 * 1024 * 1024
    private static let guid = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

    struct Frame {
        let opcode: UInt8
        let payload: Data
    }

    static func acceptKey(_ key: String) -> String {
        Data(Insecure.SHA1.hash(data: Data((key + guid).utf8))).base64EncodedString()
    }

    /// Takes one whole message off the front of `buffer`, joining continuation frames. Nil until it has all arrived.
    static func parse(_ buffer: inout Data) -> Frame? {
        var bytes = [UInt8](buffer)
        var offset = 0
        var opcode: UInt8 = 0
        var message = Data()
        while true {
            guard bytes.count >= offset + 2 else { return nil }
            let first = bytes[offset], second = bytes[offset + 1]
            var length = Int(second & 0x7F)
            var header = 2
            if length == 126 || length == 127 {
                let size = length == 126 ? 2 : 8
                guard bytes.count >= offset + 2 + size else { return nil }
                length = bytes[(offset + 2)..<(offset + 2 + size)].reduce(0) { $0 << 8 | Int($1) }
                header += size
            }
            let masked = second & 0x80 != 0
            if masked { header += 4 }
            guard bytes.count >= offset + header + length else { return nil }
            let start = offset + header
            if masked {
                let mask = Array(bytes[(start - 4)..<start])
                for index in 0..<length { bytes[start + index] ^= mask[index % 4] }
            }
            let payload = Data(bytes[start..<(start + length)])
            offset = start + length
            let frameOpcode = first & 0x0F
            // Control frames may arrive between the pieces of a fragmented message.
            if frameOpcode >= close {
                buffer.removeFirst(offset)
                return Frame(opcode: frameOpcode, payload: payload)
            }
            if frameOpcode != continuation { opcode = frameOpcode }
            message.append(payload)
            if first & 0x80 != 0 {
                buffer.removeFirst(offset)
                return Frame(opcode: opcode, payload: message)
            }
        }
    }

    static func frame(opcode: UInt8, payload: Data) -> Data {
        var frame = Data([0x80 | opcode])
        switch payload.count {
        case ..<126: frame.append(UInt8(payload.count))
        case ..<65_536: frame.append(contentsOf: [126, UInt8(payload.count >> 8), UInt8(payload.count & 0xFF)])
        default: frame.append(127); frame.append(contentsOf: (0..<8).reversed().map { UInt8((payload.count >> ($0 * 8)) & 0xFF) })
        }
        return frame + payload
    }
}

#endif
