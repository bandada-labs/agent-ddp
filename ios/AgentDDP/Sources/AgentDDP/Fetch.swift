#if DEBUG && canImport(UIKit)
import Foundation

/// The CDP Fetch domain at the request stage. Requests the app sends through `AgentDDP.send` wait here while a client
/// has them paused, then go on (changed or not), get the client's answer, or fail. Paused requests show redacted, as
/// in Network; a value the client sends back unchanged keeps the real one, so a secret never has to leave the app.
// ponytail: one client intercepts at a time, the last Fetch.enable wins. Chain them when two clients need it.
final class Fetch {
    static let shared = Fetch()

    enum Decision {
        case proceed(URLRequest)
        case fulfill(Data, HTTPURLResponse)
        case fail(URLError)
    }

    private struct Paused {
        let request: URLRequest
        let shown: [String: Any]
        let resume: (Decision) -> Void
    }

    private let lock = NSLock()
    private var owner: ObjectIdentifier?
    private var patterns: [NSRegularExpression] = []
    private var listener: (([String: Any]) -> Void)?
    private var paused: [String: Paused] = [:]
    private var nextNumber = 1

    func enable(owner: AnyObject, params: [String: Any], listener: @escaping ([String: Any]) -> Void) throws {
        let patterns = try (params["patterns"] as? [[String: Any]] ?? [["urlPattern": "*"]]).compactMap(Self.pattern)
        lock.lock()
        let released = paused.values
        paused = [:]
        self.owner = ObjectIdentifier(owner)
        self.patterns = patterns
        self.listener = listener
        lock.unlock()
        released.forEach { $0.resume(.proceed($0.request)) }
    }

    /// Lets every paused request go on unchanged, as Chrome does when the client goes away.
    func disable(owner: AnyObject) {
        lock.lock()
        guard self.owner == ObjectIdentifier(owner) else { return lock.unlock() }
        let released = paused.values
        paused = [:]
        self.owner = nil
        patterns = []
        listener = nil
        lock.unlock()
        released.forEach { $0.resume(.proceed($0.request)) }
    }

    func pause(_ request: URLRequest) async -> Decision {
        await withCheckedContinuation { continuation in
            let url = request.url?.absoluteString ?? ""
            lock.lock()
            guard let listener, patterns.contains(where: { $0.firstMatch(in: url, range: NSRange(url.startIndex..., in: url)) != nil }) else {
                lock.unlock()
                return continuation.resume(returning: .proceed(request))
            }
            let id = "f\(nextNumber)"
            nextNumber += 1
            let shown = Self.shown(request, NetworkLog.shared.currentRedactor)
            paused[id] = Paused(request: request, shown: shown, resume: { continuation.resume(returning: $0) })
            lock.unlock()
            listener(["requestId": id, "request": shown, "frameId": "app", "resourceType": "Fetch"])
        }
    }

    func continueRequest(_ params: [String: Any]) throws {
        try resume(params) { paused in
            var request = paused.request
            if let url = params["url"] as? String, url != paused.shown["url"] as? String {
                guard let changed = URL(string: url) else { throw Cdp.Failure(description: "Invalid URL") }
                request.url = changed
            }
            if let method = params["method"] as? String { request.httpMethod = method }
            if let postData = params["postData"] as? String {
                let body = try Self.base64(postData)
                if String(decoding: body, as: UTF8.self) != paused.shown["postData"] as? String { request.httpBody = body }
            }
            if let headers = params["headers"] as? [[String: Any]] {
                let shown = paused.shown["headers"] as? [String: String] ?? [:]
                let real = paused.request.allHTTPHeaderFields ?? [:]
                request.allHTTPHeaderFields = headers.reduce(into: [:]) { result, header in
                    let name = header["name"] as? String ?? "", value = header["value"] as? String ?? ""
                    result[name] = value == shown[name] ? real[name] ?? value : value
                }
            }
            return .proceed(request)
        }
    }

    func fulfillRequest(_ params: [String: Any]) throws {
        try resume(params) { paused in
            guard let status = params["responseCode"] as? Int else { throw Cdp.Failure(description: "Invalid parameters: responseCode") }
            let headers = (params["responseHeaders"] as? [[String: Any]] ?? []).reduce(into: [String: String]()) {
                $0[$1["name"] as? String ?? ""] = $1["value"] as? String ?? ""
            }
            let body = try (params["body"] as? String).map(Self.base64) ?? Data()
            guard let url = paused.request.url,
                  let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers) else {
                throw Cdp.Failure(description: "Invalid http status code or phrase")
            }
            return .fulfill(body, response)
        }
    }

    func failRequest(_ params: [String: Any]) throws {
        try resume(params) { _ in
            let reason = params["errorReason"] as? String ?? ""
            guard let code = Self.errorCodes[reason] else { throw Cdp.Failure(description: "Invalid parameters: errorReason") }
            return .fail(URLError(code))
        }
    }

    /// Decides first, so a bad message leaves the request paused for the next one.
    private func resume(_ params: [String: Any], _ decide: (Paused) throws -> Decision) throws {
        let id = params["requestId"] as? String ?? ""
        lock.lock()
        let found = paused[id]
        lock.unlock()
        guard let found else { throw Cdp.Failure(description: "Invalid InterceptionId.") }
        let decision = try decide(found)
        lock.lock()
        let stillPaused = paused.removeValue(forKey: id) != nil
        lock.unlock()
        if stillPaused { found.resume(decision) }
    }

    private static func pattern(_ pattern: [String: Any]) throws -> NSRegularExpression? {
        if let stage = pattern["requestStage"] as? String, stage != "Request" {
            throw Cdp.Failure(description: "Only requestStage Request; answer a request with Fetch.fulfillRequest")
        }
        if let type = pattern["resourceType"] as? String, !["Fetch", "XHR"].contains(type) { return nil }
        return wildcard(pattern["urlPattern"] as? String ?? "*")
    }

    /// Chrome's URL pattern: `*` is any run of characters, `?` one character, a backslash escapes the next one.
    static func wildcard(_ pattern: String) -> NSRegularExpression {
        var regex = "^"
        var escaped = false
        for character in pattern {
            switch (escaped, character) {
            case (false, "\\"): escaped = true; continue
            case (false, "*"): regex += ".*"
            case (false, "?"): regex += "."
            default: regex += NSRegularExpression.escapedPattern(for: String(character))
            }
            escaped = false
        }
        return try! NSRegularExpression(pattern: regex + "$", options: .dotMatchesLineSeparators)
    }

    private static func shown(_ request: URLRequest, _ redactor: Redactor) -> [String: Any] {
        var shown: [String: Any] = [
            "url": request.url.map(redactor.url) ?? "", "method": request.httpMethod ?? "GET",
            "headers": redactor.headers(request.allHTTPHeaderFields ?? [:]), "hasPostData": request.httpBody != nil,
            "initialPriority": "High", "referrerPolicy": "strict-origin-when-cross-origin",
        ]
        if let body = redactor.body(request.httpBody) { shown["postData"] = body }
        return shown
    }

    private static func base64(_ text: String) throws -> Data {
        guard let data = Data(base64Encoded: text) else { throw Cdp.Failure(description: "Invalid parameters: not base64") }
        return data
    }

    private static let errorCodes: [String: URLError.Code] = [
        "Failed": .unknown, "Aborted": .cancelled, "TimedOut": .timedOut, "AccessDenied": .noPermissionsToReadFile,
        "ConnectionClosed": .networkConnectionLost, "ConnectionReset": .networkConnectionLost,
        "ConnectionAborted": .networkConnectionLost, "ConnectionRefused": .cannotConnectToHost,
        "ConnectionFailed": .cannotConnectToHost, "AddressUnreachable": .cannotConnectToHost,
        "NameNotResolved": .cannotFindHost, "InternetDisconnected": .notConnectedToInternet,
        "BlockedByClient": .cancelled, "BlockedByResponse": .badServerResponse,
    ]
}
#endif
