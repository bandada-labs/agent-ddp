#if DEBUG && canImport(UIKit)
import Foundation

/// Pausing the app's requests. Requests the app sends through `AgentDDP.send` wait here while a client intercepts
/// them, then go on (changed or not), get the client's answer, or fail with the URLError the app's code would see.
/// Paused requests show redacted, as in the network log; a value the client sends back unchanged keeps the real one,
/// so a secret never has to leave the app.
// ponytail: one client intercepts at a time, the last net.intercept wins. Chain them when two clients need it.
// ponytail: bodies are UTF-8 text. Add base64 when a client needs to send binary bodies.
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

    /// `urls` are wildcards; without them every request pauses.
    func intercept(owner: AnyObject, params: [String: Any], listener: @escaping ([String: Any]) -> Void) {
        let patterns = (params["urls"] as? [String] ?? ["*"]).map(Self.wildcard)
        lock.lock()
        let released = paused.values
        paused = [:]
        self.owner = ObjectIdentifier(owner)
        self.patterns = patterns
        self.listener = listener
        lock.unlock()
        released.forEach { $0.resume(.proceed($0.request)) }
    }

    /// Lets every paused request go on unchanged.
    func release(owner: AnyObject) {
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
            listener(["id": id, "request": shown])
        }
    }

    func proceed(_ params: [String: Any]) throws {
        try resume(params) { paused in
            var request = paused.request
            if let url = params["url"] as? String, url != paused.shown["url"] as? String {
                guard let changed = URL(string: url) else { throw Control.Failure(description: "Invalid url") }
                request.url = changed
            }
            if let method = params["method"] as? String { request.httpMethod = method }
            if let body = params["body"] as? String, body != paused.shown["body"] as? String { request.httpBody = Data(body.utf8) }
            if let headers = params["headers"] as? [String: String] {
                let shown = paused.shown["headers"] as? [String: String] ?? [:]
                let real = paused.request.allHTTPHeaderFields ?? [:]
                request.allHTTPHeaderFields = headers.reduce(into: [:]) { result, header in
                    result[header.key] = header.value == shown[header.key] ? real[header.key] ?? header.value : header.value
                }
            }
            return .proceed(request)
        }
    }

    func respond(_ params: [String: Any]) throws {
        try resume(params) { paused in
            guard let status = params["status"] as? Int, (100...599).contains(status) else {
                throw Control.Failure(description: "status is 100 to 599")
            }
            guard let url = paused.request.url,
                  let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1",
                                                 headerFields: params["headers"] as? [String: String] ?? [:]) else {
                throw Control.Failure(description: "Invalid response")
            }
            return .fulfill(Data((params["body"] as? String ?? "").utf8), response)
        }
    }

    func fail(_ params: [String: Any]) throws {
        try resume(params) { _ in
            guard let code = Self.errors[params["error"] as? String ?? ""] else {
                throw Control.Failure(description: "error is one of \(Self.errors.keys.sorted().joined(separator: ", "))")
            }
            return .fail(URLError(code))
        }
    }

    /// Decides first, so a bad message leaves the request paused for the next one.
    private func resume(_ params: [String: Any], _ decide: (Paused) throws -> Decision) throws {
        let id = params["id"] as? String ?? ""
        lock.lock()
        let found = paused[id]
        lock.unlock()
        guard let found else { throw Control.Failure(description: "No paused request '\(id)'") }
        let decision = try decide(found)
        lock.lock()
        let stillPaused = paused.removeValue(forKey: id) != nil
        lock.unlock()
        if stillPaused { found.resume(decision) }
    }

    /// `*` is any run of characters, `?` one character, a backslash escapes the next one.
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
            "headers": redactor.headers(request.allHTTPHeaderFields ?? [:]),
        ]
        if let body = redactor.body(request.httpBody) { shown["body"] = body }
        return shown
    }

    private static let errors: [String: URLError.Code] = [
        "offline": .notConnectedToInternet, "host": .cannotFindHost, "timeout": .timedOut, "refused": .cannotConnectToHost,
        "unreachable": .cannotConnectToHost, "lost": .networkConnectionLost, "cancelled": .cancelled, "failed": .unknown,
    ]
}
#endif
