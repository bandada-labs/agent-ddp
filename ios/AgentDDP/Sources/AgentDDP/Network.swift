#if DEBUG && canImport(UIKit)
import Foundation

/// The last requests the app's Debug network decorator reported, already redacted, for `ddp net`.
final class NetworkLog {
    static let shared = NetworkLog()

    private static let kept = 200
    private let lock = NSLock()
    private var entries: [[String: Any]] = []
    private var nextNumber = 1
    private var watchers: [UUID: ([String: Any]) -> Bool] = [:]
    private var redactor = Redactor(values: [])

    func redact(values: [String]) {
        lock.lock()
        redactor = Redactor(values: values)
        lock.unlock()
    }

    var currentRedactor: Redactor {
        lock.lock()
        defer { lock.unlock() }
        return redactor
    }

    func add(request: URLRequest, response: URLResponse?, body: Data?, error: Error?,
             startedAt: Date, finishedAt: Date) {
        let redactor = currentRedactor
        lock.lock()
        let id = "n\(nextNumber)"
        nextNumber += 1
        lock.unlock()

        let http = response as? HTTPURLResponse
        var entry: [String: Any] = [
            "type": "request", "id": id, "method": request.httpMethod ?? "GET",
            "url": request.url.map(redactor.url) ?? "",
            "startedAt": (startedAt.timeIntervalSince1970 * 1000).rounded(),
            "ms": ((finishedAt.timeIntervalSince(startedAt)) * 1000).rounded(),
            "requestHeaders": redactor.headers(request.allHTTPHeaderFields ?? [:]),
            "requestBytes": request.httpBody?.count ?? 0,
            "responseBytes": body?.count ?? 0,
        ]
        if let requestBody = redactor.body(request.httpBody) { entry["requestBody"] = requestBody }
        if let http {
            entry["status"] = http.statusCode
            let headers = http.allHeaderFields.reduce(into: [String: String]()) { $0["\($1.key)"] = "\($1.value)" }
            entry["responseHeaders"] = redactor.headers(headers)
        }
        if let responseBody = redactor.body(body) { entry["responseBody"] = responseBody }
        if let error { entry["error"] = redactor.text(error.localizedDescription) }

        lock.lock()
        entries.append(entry)
        if entries.count > Self.kept { entries.removeFirst(entries.count - Self.kept) }
        let current = watchers
        lock.unlock()
        let summary = Self.summary(entry)
        current.filter { !$0.value(summary) }.forEach { stopWatching($0.key) }
    }

    func summaries() -> [[String: Any]] {
        lock.lock()
        defer { lock.unlock() }
        return entries.map(Self.summary)
    }

    func entry(id: String) -> [String: Any]? {
        lock.lock()
        defer { lock.unlock() }
        return entries.first { $0["id"] as? String == id }
    }

    func watch(_ watcher: @escaping ([String: Any]) -> Bool) -> UUID {
        let key = UUID()
        lock.lock()
        watchers[key] = watcher
        lock.unlock()
        return key
    }

    func stopWatching(_ key: UUID) {
        lock.lock()
        watchers[key] = nil
        lock.unlock()
    }

    private static func summary(_ entry: [String: Any]) -> [String: Any] {
        entry.filter { ["type", "id", "method", "url", "status", "ms", "responseBytes", "error", "startedAt"].contains($0.key) }
    }
}

/// Removes secrets before a request is kept: header and query names and JSON keys that look secret,
/// plus literal values the app registers (its dev secret). Bodies are cut at 16 KB.
/// `net.send`: a request sent through `AgentDDP.send` with the last client the app passed there, so it is logged and
/// can be paused like the app's own. Headers the app adds before calling `AgentDDP.send` are not in it. Before the app
/// sends a request, URLSession.shared.
// ponytail: the last client wins. Name clients when an app has several that matter.
final class AppClient {
    static let shared = AppClient()

    typealias Send = (URLRequest) async throws -> (Data, URLResponse)

    private let lock = NSLock()
    private var latest: Send?

    func remember(_ send: @escaping Send) {
        lock.lock()
        latest = send
        lock.unlock()
    }

    func send(_ params: [String: Any]) async throws -> [String: Any] {
        guard let url = (params["url"] as? String).flatMap(URL.init(string:)) else { throw Control.Failure(description: "net.send needs a url") }
        var request = URLRequest(url: url)
        request.httpMethod = params["method"] as? String ?? "GET"
        request.allHTTPHeaderFields = params["headers"] as? [String: String]
        request.httpBody = (params["body"] as? String).map { Data($0.utf8) }
        let (body, response) = try await AgentDDP.logged(request, using: client())
        let http = response as? HTTPURLResponse
        let redactor = NetworkLog.shared.currentRedactor
        return ["status": http?.statusCode ?? 0, "headers": redactor.headers(http?.allHeaderFields as? [String: String] ?? [:]),
                "body": redactor.body(body) ?? ""]
    }

    private func client() -> Send {
        lock.lock()
        defer { lock.unlock() }
        return latest ?? { try await URLSession.shared.data(for: $0) }
    }
}

struct Redactor {
    static let mask = "[redacted]"
    private static let secretName = try! NSRegularExpression(
        pattern: "secret|token|key|password|attest|authorization|cookie", options: .caseInsensitive)
    private static let bodyLimit = 16 * 1024

    let values: [String]

    static func isSecret(_ name: String) -> Bool {
        secretName.firstMatch(in: name, range: NSRange(name.startIndex..., in: name)) != nil
    }

    func text(_ text: String) -> String {
        values.filter { !$0.isEmpty }.reduce(text) { $0.replacingOccurrences(of: $1, with: Self.mask) }
    }

    func headers(_ headers: [String: String]) -> [String: String] {
        headers.reduce(into: [:]) { $0[$1.key] = Self.isSecret($1.key) ? Self.mask : text($1.value) }
    }

    func url(_ url: URL) -> String {
        guard var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return text(url.absoluteString) }
        components.queryItems = components.queryItems?.map {
            URLQueryItem(name: $0.name, value: Self.isSecret($0.name) ? Self.mask : $0.value)
        }
        return text(components.string ?? url.absoluteString)
    }

    func body(_ data: Data?) -> String? {
        guard let data, !data.isEmpty else { return nil }
        if let json = try? JSONSerialization.jsonObject(with: data, options: .fragmentsAllowed),
           let cleaned = try? JSONSerialization.data(withJSONObject: redactJSON(json), options: [.sortedKeys, .fragmentsAllowed]) {
            return cut(text(String(decoding: cleaned, as: UTF8.self)))
        }
        guard let string = String(data: data, encoding: .utf8) else { return "<\(data.count) bytes, not text>" }
        return cut(text(string))
    }

    private func redactJSON(_ value: Any) -> Any {
        if let object = value as? [String: Any] {
            return object.reduce(into: [String: Any]()) { $0[$1.key] = Self.isSecret($1.key) ? Self.mask : redactJSON($1.value) }
        }
        if let array = value as? [Any] { return array.map(redactJSON) }
        return value
    }

    private func cut(_ text: String) -> String {
        text.utf8.count <= Self.bodyLimit ? text
            : String(decoding: text.utf8.prefix(Self.bodyLimit), as: UTF8.self) + "... [cut at 16 KB]"
    }
}
#endif
