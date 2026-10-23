#if DEBUG && canImport(UIKit)
import CryptoKit
import Foundation
import Network
import UIKit

/// The Chrome DevTools Protocol endpoint (ADR 0007): HTTP discovery on `/json/version` and `/json/list`,
/// then JSON-RPC over a WebSocket. Bound to 127.0.0.1; the port goes to `Library/Caches/agent-ddp/cdp.json`.
/// Like Chrome, discovery needs no token: the Host check stops DNS rebinding and the Origin check stops web pages.
final class CdpServer {
    static let shared = CdpServer()

    private let queue = DispatchQueue(label: "dev.agentddp.cdp")
    private var listener: NWListener?

    func start(directory: URL) {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
        guard listener == nil, let listener = try? NWListener(using: parameters) else { return }
        let target = CdpTarget(bundleIdentifier: Bundle.main.bundleIdentifier ?? "app",
                               title: Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
                                   ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "app")
        let endpointFile = directory.appendingPathComponent("cdp.json")
        listener.stateUpdateHandler = { state in
            guard case .ready = state, let port = listener.port?.rawValue,
                  let data = try? JSONSerialization.data(withJSONObject: ["port": Int(port)]) else { return }
            try? data.write(to: endpointFile, options: .atomic)
        }
        listener.newConnectionHandler = { [queue] connection in
            CdpConnection(connection: connection, target: target, queue: queue).start()
        }
        listener.start(queue: queue)
        self.listener = listener
    }
}

struct CdpTarget {
    let bundleIdentifier: String
    let title: String
    let id = "app"
    var url: String { "ios-app://\(bundleIdentifier)" }

    func info() -> [String: Any] {
        ["targetId": id, "type": "page", "title": title, "url": url, "attached": false, "canAccessOpener": false]
    }
}

/// One HTTP request: a discovery answer and close, or a WebSocket upgrade that then carries CDP messages.
private final class CdpConnection {
    private let connection: NWConnection
    private let target: CdpTarget
    private let queue: DispatchQueue
    private var buffer = Data()
    private var session: CdpSession?

    init(connection: NWConnection, target: CdpTarget, queue: DispatchQueue) {
        self.connection = connection
        self.target = target
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
            if session == nil, let request = HttpRequest.parse(&buffer) { handle(request) }
            while session != nil, let frame = WebSocket.parse(&buffer) { handle(frame) }
            if isComplete || error != nil {
                session?.close()
                connection.cancel()
            } else {
                receive()
            }
        }
    }

    private func handle(_ request: HttpRequest) {
        let host = request.headers["host"] ?? "localhost"
        guard Cdp.hostAllowed(host) else { return respond("500 Internal Server Error", "text/plain",
            "Host header is specified and is not an IP address or localhost.") }
        if request.headers["upgrade"]?.lowercased() == "websocket" { return upgrade(request) }
        let socketURL = "ws://\(host)/devtools/page/\(target.id)"
        switch request.path {
        case "/json/version":
            json(Cdp.version(target).merging(["webSocketDebuggerUrl": "ws://\(host)/devtools/browser/\(target.id)"]) { $1 })
        case "/json", "/json/list":
            json([["id": target.id, "type": "page", "title": target.title, "url": target.url,
                   "description": "Agent-DDP", "webSocketDebuggerUrl": socketURL]])
        default:
            respond("404 Not Found", "text/plain", "Unknown path \(request.path)")
        }
    }

    private func upgrade(_ request: HttpRequest) {
        guard let key = request.headers["sec-websocket-key"] else {
            return respond("400 Bad Request", "text/plain", "No Sec-WebSocket-Key")
        }
        guard Cdp.originAllowed(request.headers["origin"]) else {
            return respond("403 Forbidden", "text/plain",
                           "Rejected the WebSocket connection from origin \(request.headers["origin"] ?? "")")
        }
        let head = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Accept: \(WebSocket.acceptKey(key))\r\n\r\n"
        connection.send(content: Data(head.utf8), completion: .contentProcessed { _ in })
        session = CdpSession(target: target, browserLevel: request.path.hasPrefix("/devtools/browser")) { [connection] message in
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

    private func json(_ body: Any) {
        let data = (try? JSONSerialization.data(withJSONObject: body, options: .withoutEscapingSlashes)) ?? Data()
        respond("200 OK", "application/json; charset=UTF-8", String(decoding: data, as: UTF8.self))
    }

    private func respond(_ status: String, _ type: String, _ body: String) {
        let bytes = Data(body.utf8)
        let head = "HTTP/1.1 \(status)\r\nContent-Type: \(type)\r\nContent-Length: \(bytes.count)\r\nConnection: close\r\n\r\n"
        connection.send(content: Data(head.utf8) + bytes, isComplete: true,
                        completion: .contentProcessed { [connection] _ in connection.cancel() })
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

/// RFC 6455, only what CDP needs: text, ping, pong and close frames. Clients mask, the server does not.
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

/// Answers CDP messages for one client. The same error codes and messages as Chrome, so clients behave the same.
final class CdpSession {
    private let target: CdpTarget
    /// Connected to `/devtools/browser`, as clients that drive several targets do. A page connection is the page.
    private let browserLevel: Bool
    private let send: (String) -> Void
    /// Flat sessions: the app is one page, so every session id reaches the same handlers and only goes back on answers.
    private var sessionIds = Set<String>()
    /// The session that enabled Network, whose id its events carry, and the log watch feeding them.
    private var networkWatch: (sessionId: Any?, key: UUID)?
    /// The `sessionId` of the message being answered.
    private var currentSessionId: Any?
    private lazy var methods: [String: ([String: Any]) throws -> [String: Any]] = [
        "Browser.getVersion": { [target] _ in Cdp.browserVersion(target) },
        "Target.getTargets": { [target] _ in ["targetInfos": [target.info()]] },
        "Target.getTargetInfo": { [target] _ in ["targetInfo": target.info()] },
        "Target.setDiscoverTargets": { _ in [:] },
        "Target.attachToTarget": { [unowned self] _ in ["sessionId": attach()] },
        "Target.setAutoAttach": { [unowned self] params in
            // Chrome's browser target auto-attaches its pages; a page has no child targets here.
            if browserLevel, params["autoAttach"] as? Bool == true, sessionIds.isEmpty { attach() }
            return [:]
        },
        "Target.detachFromTarget": { [unowned self] params in
            sessionIds.remove(params["sessionId"] as? String ?? "")
            return [:]
        },
        "Schema.getDomains": { _ in ["domains": Cdp.domains.map { ["name": $0, "version": Cdp.protocolVersion] }] },
        "Runtime.runIfWaitingForDebugger": { _ in [:] },
        "Accessibility.getFullAXTree": { [target] _ in ["nodes": Cdp.axTree(title: target.title, elements: Dom.read())] },
        "Page.captureScreenshot": { params in try Cdp.screenshot(params) },
        "Network.enable": { [unowned self] _ in
            stopNetwork()
            let sessionId = currentSessionId
            let key = NetworkLog.shared.watch { [weak self] summary in
                guard let self, let entry = NetworkLog.shared.entry(id: summary["id"] as? String ?? "") else { return false }
                Cdp.networkEvents(entry).forEach { self.event($0.method, $0.params, sessionId: sessionId) }
                return true
            }
            networkWatch = (sessionId, key)
            return [:]
        },
        "Network.disable": { [unowned self] _ in stopNetwork(); return [:] },
        "Network.getResponseBody": { params in
            ["body": try Cdp.networkEntry(params)["responseBody"] as? String ?? "", "base64Encoded": false]
        },
        "Network.getRequestPostData": { params in
            guard let body = try Cdp.networkEntry(params)["requestBody"] as? String else {
                throw Cdp.Failure(description: "No post data available for the request")
            }
            return ["postData": body]
        },
        "DOM.getDocument": { [target] params in
            ["root": Dom(title: target.title, elements: Dom.read()).document(depth: params["depth"] as? Int ?? 1)]
        },
        "DOM.requestChildNodes": { _ in [:] },
        "DOM.describeNode": { [target] params in
            ["node": try Dom(title: target.title, elements: Dom.read()).node(Dom.nodeId(params))]
        },
        "DOM.getAttributes": { [target] params in
            ["attributes": try Dom(title: target.title, elements: Dom.read()).node(Dom.nodeId(params))["attributes"] ?? []]
        },
        "DOM.getOuterHTML": { [target] params in
            ["outerHTML": try Dom(title: target.title, elements: Dom.read()).outerHTML(Dom.nodeId(params))]
        },
        "DOM.querySelector": { [target] params in
            ["nodeId": try Dom(title: target.title, elements: Dom.read()).query(params["selector"] as? String ?? "").first ?? 0]
        },
        "DOM.querySelectorAll": { [target] params in
            ["nodeIds": try Dom(title: target.title, elements: Dom.read()).query(params["selector"] as? String ?? "")]
        },
        "DOM.getBoxModel": { [target] params in
            ["model": try Dom(title: target.title, elements: Dom.read()).boxModel(Dom.nodeId(params))]
        },
    ].merging(Cdp.toggledDomains.flatMap { domain -> [(String, ([String: Any]) throws -> [String: Any])] in
        [("\(domain).enable", { _ in [:] }), ("\(domain).disable", { _ in [:] })]
    }) { $1 }

    init(target: CdpTarget, browserLevel: Bool = false, send: @escaping (String) -> Void) {
        self.target = target
        self.browserLevel = browserLevel
        self.send = send
    }

    @discardableResult
    private func attach() -> String {
        let sessionId = UUID().uuidString.replacingOccurrences(of: "-", with: "")
        sessionIds.insert(sessionId)
        var info = target.info()
        info["attached"] = true
        reply(["method": "Target.attachedToTarget",
               "params": ["sessionId": sessionId, "targetInfo": info, "waitingForDebugger": false]])
        return sessionId
    }

    /// The connection closed: stop feeding it events.
    func close() { stopNetwork() }

    private func stopNetwork() {
        if let networkWatch { NetworkLog.shared.stopWatching(networkWatch.key) }
        networkWatch = nil
    }

    private func event(_ method: String, _ params: [String: Any], sessionId: Any?) {
        var message: [String: Any] = ["method": method, "params": params]
        if let sessionId { message["sessionId"] = sessionId }
        reply(message)
    }

    func receive(_ text: String) {
        guard let message = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] else {
            return reply(Cdp.error(id: nil, code: -32700, message: "Message must be a valid JSON"))
        }
        guard let id = message["id"] as? Int else {
            return reply(Cdp.error(id: nil, code: -32600, message: "Message must have integer 'id' property"))
        }
        let method = message["method"] as? String ?? ""
        guard let handler = methods[method] else {
            return reply(Cdp.error(id: id, code: -32601, message: "'\(method)' wasn't found"))
        }
        currentSessionId = message["sessionId"]
        var answer: [String: Any]
        do {
            answer = ["id": id, "result": try handler(message["params"] as? [String: Any] ?? [:])]
        } catch {
            answer = Cdp.error(id: id, code: -32000, message: "\(error)")
        }
        if let sessionId = message["sessionId"] { answer["sessionId"] = sessionId }
        reply(answer)
    }

    private func reply(_ answer: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: answer, options: .withoutEscapingSlashes) else { return }
        send(String(decoding: data, as: UTF8.self))
    }
}

/// The DOM over the captured elements: `#document > html > body`, then one node per element in reading order,
/// the same order as the accessibility tree. The tag is the element's role (`text` for StaticText), and every node
/// has the same attributes on both platforms: `role`, `id`, `label`. Node ids are positions in one read, so they
/// hold only until the screen changes.
// ponytail: flat, no view hierarchy and no text nodes. Add the hierarchy when a client needs ancestors.
struct Dom {
    static let documentId = 1, htmlId = 2, bodyId = 3, firstElementId = 4

    let title: String
    let elements: [[String: Any]]

    static func read() -> [[String: Any]] { onMain { FrameCapture.visibleWindows().flatMap(AccessibilityReader.elements(in:)) } }

    /// `nodeId` or `backendNodeId`, which are the same number here.
    static func nodeId(_ params: [String: Any]) throws -> Int {
        guard let id = params["nodeId"] as? Int ?? params["backendNodeId"] as? Int else {
            throw Cdp.Failure(description: "Either nodeId, backendNodeId or objectId must be specified")
        }
        return id
    }

    /// `depth` -1 is the whole tree, as in Chrome.
    func document(depth: Int) -> [String: Any] { node(Dom.documentId, depth: depth) }

    func node(_ id: Int) throws -> [String: Any] {
        guard id >= Dom.documentId, id < Dom.firstElementId + elements.count else { throw Cdp.Failure(description: "Could not find node with given id") }
        return node(id, depth: 0)
    }

    private func node(_ id: Int, depth: Int) -> [String: Any] {
        let childIds: [Int] = switch id {
        case Dom.documentId: [Dom.htmlId]
        case Dom.htmlId: [Dom.bodyId]
        case Dom.bodyId: Array(Dom.firstElementId..<(Dom.firstElementId + elements.count))
        default: []
        }
        var node: [String: Any] = ["nodeId": id, "backendNodeId": id, "childNodeCount": childIds.count]
        if id == Dom.documentId {
            node.merge(["nodeType": 9, "nodeName": "#document", "localName": "", "nodeValue": "",
                        "documentURL": "", "baseURL": ""]) { $1 }
        } else {
            let tag = self.tag(id)
            node.merge(["nodeType": 1, "nodeName": tag.uppercased(), "localName": tag, "nodeValue": "",
                        "attributes": attributes(id).flatMap { [$0.0, $0.1] }]) { $1 }
        }
        if id > Dom.documentId { node["parentId"] = id == Dom.htmlId ? Dom.documentId : id == Dom.bodyId ? Dom.htmlId : Dom.bodyId }
        if depth != 0 { node["children"] = childIds.map { self.node($0, depth: depth - 1) } }
        return node
    }

    private func element(_ id: Int) -> [String: Any]? {
        id >= Dom.firstElementId && id - Dom.firstElementId < elements.count ? elements[id - Dom.firstElementId] : nil
    }

    private func tag(_ id: Int) -> String {
        switch id {
        case Dom.htmlId: return "html"
        case Dom.bodyId: return "body"
        default:
            let role = element(id)?["role"] as? String ?? "generic"
            return role == "StaticText" ? "text" : role
        }
    }

    private func attributes(_ id: Int) -> [(String, String)] {
        guard let element = element(id) else { return id == Dom.htmlId ? [("title", title)] : [] }
        return ["role", "id", "label"].compactMap { name in (element[name] as? String).map { (name, $0) } }
    }

    func outerHTML(_ id: Int) throws -> String {
        _ = try node(id)
        let tag = id == Dom.documentId ? "" : self.tag(id)
        let attributes = attributes(id).map { " \($0.0)=\"\(Dom.escape($0.1))\"" }.joined()
        let inner: String = switch id {
        case Dom.documentId: try outerHTML(Dom.htmlId)
        case Dom.htmlId: try outerHTML(Dom.bodyId)
        case Dom.bodyId: try (0..<elements.count).map { try outerHTML(Dom.firstElementId + $0) }.joined()
        default: Dom.escape(element(id)?["label"] as? String ?? "")
        }
        return tag.isEmpty ? inner : "<\(tag)\(attributes)>\(inner)</\(tag)>"
    }

    private static func escape(_ text: String) -> String {
        text.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
    }

    /// `DOM.getBoxModel`: no padding, border or margin, so the four quads are the frame.
    func boxModel(_ id: Int) throws -> [String: Any] {
        _ = try node(id)
        guard let frame = element(id)?["frame"] as? [Double] else { throw Cdp.Failure(description: "Could not compute box model.") }
        let (x, y, width, height) = (frame[0], frame[1], frame[2], frame[3])
        let quad = [x, y, x + width, y, x + width, y + height, x, y + height].map { round2($0) }
        return ["content": quad, "padding": quad, "border": quad, "margin": quad, "width": width, "height": height]
    }

    /// Selector lists of compound selectors: `button`, `#saveButton`, `[label='Save']`, `button[label^='Sa']`, `*`.
    /// Attribute operators `=`, `^=`, `$=`, `*=`. Combinators throw, since the tree is flat.
    func query(_ selector: String) throws -> [Int] {
        let compounds = try selector.split(separator: ",").map { try SimpleSelector(String($0)) }
        return (Dom.firstElementId..<(Dom.firstElementId + elements.count)).filter { id in
            let attributes = Dictionary(attributes(id), uniquingKeysWith: { first, _ in first })
            return compounds.contains { $0.matches(tag: tag(id), attributes: attributes) }
        }
    }
}

struct SimpleSelector {
    private var tag: String?
    private var conditions: [(name: String, operation: String, value: String?)] = []

    init(_ text: String) throws {
        let invalid = Cdp.Failure(description: "DOM Error while querying")
        let pattern = try NSRegularExpression(pattern: #"^\s*([a-zA-Z*][\w-]*)?((?:#[\w-]+|\[\s*[\w-]+\s*(?:[\^$*]?=\s*(?:'[^']*'|"[^"]*"|[^\]\s]+))?\s*\])*)\s*$"#)
        let range = NSRange(text.startIndex..., in: text)
        guard let match = pattern.firstMatch(in: text, range: range),
              let restRange = Range(match.range(at: 2), in: text),
              !text.trimmingCharacters(in: .whitespaces).isEmpty else { throw invalid }
        let tagText = Range(match.range(at: 1), in: text).map { String(text[$0]) } ?? "*"
        tag = tagText == "*" ? nil : tagText.lowercased()
        let parts = try NSRegularExpression(pattern: #"#([\w-]+)|\[\s*([\w-]+)\s*(?:([\^$*]?=)\s*('[^']*'|"[^"]*"|[^\]\s]+))?\s*\]"#)
        let rest = String(text[restRange])
        for part in parts.matches(in: rest, range: NSRange(rest.startIndex..., in: rest)) {
            func group(_ index: Int) -> String? { Range(part.range(at: index), in: rest).map { String(rest[$0]) } }
            if let id = group(1) {
                conditions.append(("id", "=", id))
            } else if let name = group(2) {
                let value = group(4).map { $0.first == "'" || $0.first == "\"" ? String($0.dropFirst().dropLast()) : $0 }
                conditions.append((name, group(3) ?? "", value))
            }
        }
    }

    func matches(tag: String, attributes: [String: String]) -> Bool {
        if let wanted = self.tag, wanted != tag { return false }
        return conditions.allSatisfy { condition in
            guard let actual = attributes[condition.name] else { return false }
            guard let value = condition.value else { return true }
            switch condition.operation {
            case "^=": return actual.hasPrefix(value)
            case "$=": return actual.hasSuffix(value)
            case "*=": return actual.contains(value)
            default: return actual == value
            }
        }
    }
}

/// The CDP queue waits for UIKit; tests call in from the main thread already.
func onMain<T>(_ work: () -> T) -> T { Thread.isMainThread ? work() : DispatchQueue.main.sync(execute: work) }

enum Cdp {
    static let protocolVersion = "1.3"
    static let domains = ["Accessibility", "Browser", "DOM", "Log", "Network", "Page", "Runtime", "Schema", "Target"]
    /// Domains whose enable and disable only switch events on and off.
    static let toggledDomains = ["Accessibility", "DOM", "Log", "Page", "Runtime"]

    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    /// `Accessibility.getFullAXTree`: a root like Chrome's RootWebArea, then the elements in reading order.
    /// Ids are positions in this read, so they hold only until the screen changes.
    static func axTree(title: String, elements: [[String: Any]]) -> [[String: Any]] {
        let children = elements.enumerated().map { index, element -> [String: Any] in
            ["nodeId": String(index + 2), "parentId": "1", "backendDOMNodeId": Dom.firstElementId + index, "ignored": false,
             "role": ["type": "role", "value": element["role"] as? String ?? "generic"],
             "name": ["type": "computedString", "value": element["label"] as? String ?? element["id"] as? String ?? ""],
             "childIds": [String]()]
        }
        let root: [String: Any] = ["nodeId": "1", "backendDOMNodeId": 1, "ignored": false,
                                   "role": ["type": "role", "value": "RootWebArea"],
                                   "name": ["type": "computedString", "value": title],
                                   "childIds": children.map { $0["nodeId"]! }]
        return [root] + children
    }

    /// One finished request from the network log as Chrome's events, in Chrome's order. The log only hears of a request
    /// when it ends, so all of them arrive together; the timestamps are when it started and ended (seconds since 1970).
    static func networkEvents(_ entry: [String: Any]) -> [(method: String, params: [String: Any])] {
        let id = entry["id"] as? String ?? ""
        let started = (entry["startedAt"] as? Double ?? 0) / 1000
        let ended = started + (entry["ms"] as? Double ?? 0) / 1000
        let url = entry["url"] as? String ?? ""
        var request: [String: Any] = ["url": url, "method": entry["method"] as? String ?? "GET",
                                      "headers": entry["requestHeaders"] ?? [:], "hasPostData": entry["requestBody"] != nil]
        if let body = entry["requestBody"] { request["postData"] = body }
        var events: [(String, [String: Any])] = [("Network.requestWillBeSent", [
            "requestId": id, "loaderId": "", "documentURL": url, "request": request, "timestamp": started,
            "wallTime": started, "initiator": ["type": "other"], "type": "Fetch", "redirectHasExtraInfo": false,
        ])]
        if let status = entry["status"] as? Int {
            let headers = entry["responseHeaders"] as? [String: String] ?? [:]
            let contentType = headers.first { $0.key.lowercased() == "content-type" }?.value ?? ""
            events.append(("Network.responseReceived", [
                "requestId": id, "loaderId": "", "timestamp": ended, "type": "Fetch", "hasExtraInfo": false,
                "response": ["url": url, "status": status, "statusText": "",
                             "headers": headers, "mimeType": contentType.split(separator: ";").first.map(String.init) ?? "",
                             "connectionReused": false, "connectionId": 0, "encodedDataLength": entry["responseBytes"] ?? 0,
                             "securityState": url.hasPrefix("https") ? "secure" : "insecure"],
            ]))
        }
        if let error = entry["error"] as? String {
            events.append(("Network.loadingFailed", ["requestId": id, "timestamp": ended, "type": "Fetch",
                                                     "errorText": error, "canceled": false]))
        } else {
            events.append(("Network.loadingFinished", ["requestId": id, "timestamp": ended,
                                                       "encodedDataLength": entry["responseBytes"] ?? 0]))
        }
        return events
    }

    static func networkEntry(_ params: [String: Any]) throws -> [String: Any] {
        guard let entry = NetworkLog.shared.entry(id: params["requestId"] as? String ?? "") else {
            throw Failure(description: "No resource with given identifier found")
        }
        return entry
    }

    /// `Page.captureScreenshot` in device pixels, like Chrome. `clip` is in points.
    static func screenshot(_ params: [String: Any]) throws -> [String: Any] {
        let format = params["format"] as? String ?? "png"
        guard ["png", "jpeg"].contains(format) else { throw Failure(description: "Unsupported image format: \(format)") }
        let quality = (params["quality"] as? Double ?? 80) / 100
        let data: Data? = onMain {
            let windows = FrameCapture.visibleWindows()
            guard let screen = windows.first?.screen else { return nil }
            let clip = (params["clip"] as? [String: Any]).map {
                CGRect(x: $0["x"] as? Double ?? 0, y: $0["y"] as? Double ?? 0,
                       width: $0["width"] as? Double ?? 0, height: $0["height"] as? Double ?? 0)
            } ?? screen.bounds
            let renderFormat = UIGraphicsImageRendererFormat()
            renderFormat.scale = screen.scale * ((params["clip"] as? [String: Any])?["scale"] as? Double ?? 1)
            let image = UIGraphicsImageRenderer(bounds: clip, format: renderFormat).image { _ in
                windows.forEach { $0.drawHierarchy(in: $0.frame, afterScreenUpdates: false) }
            }
            return format == "png" ? image.pngData() : image.jpegData(compressionQuality: quality)
        }
        guard let data else { throw Failure(description: "No window to capture") }
        return ["data": data.base64EncodedString()]
    }
    private static let product = "Agent-DDP/\(FrameCapture.protocolVersion)"

    /// `/json/version`.
    static func version(_ target: CdpTarget) -> [String: Any] {
        ["Browser": product, "Protocol-Version": protocolVersion, "User-Agent": userAgent(target)]
    }

    /// `Browser.getVersion`.
    static func browserVersion(_ target: CdpTarget) -> [String: Any] {
        ["protocolVersion": protocolVersion, "product": product, "revision": "", "userAgent": userAgent(target),
         "jsVersion": ""]
    }

    private static func userAgent(_ target: CdpTarget) -> String { "Agent-DDP iOS \(target.bundleIdentifier)" }

    /// Like Chrome: no Origin (a tool) or DevTools itself. A web page always sends its own origin.
    static func originAllowed(_ origin: String?) -> Bool {
        guard let origin else { return true }
        return origin.hasPrefix("devtools://") || origin.hasPrefix("chrome-devtools://")
    }

    /// Like Chrome: a page that rebinds its own domain to 127.0.0.1 still sends that domain as Host.
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
#endif
