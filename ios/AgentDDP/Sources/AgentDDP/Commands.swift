#if DEBUG && canImport(UIKit)
import Foundation

/// The named actions the app added with `AgentDDP.command`, for `app.commands` and `app.run`.
final class Commands {
    static let shared = Commands()

    typealias Run = ([String: Any]) async throws -> Any?

    private let lock = NSLock()
    private var commands: [String: Run] = [:]

    func add(_ name: String, _ run: @escaping Run) {
        lock.lock()
        commands[name] = run
        lock.unlock()
    }

    func names() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return commands.keys.sorted()
    }

    func run(_ params: [String: Any]) async throws -> [String: Any] {
        let name = params["name"] as? String ?? ""
        guard let command = command(name) else { throw Control.Failure(description: "No command '\(name)'; app.commands lists them") }
        let answer: [String: Any] = ["value": try await command(params["params"] as? [String: Any] ?? [:]) ?? NSNull()]
        guard JSONSerialization.isValidJSONObject(answer) else { throw Control.Failure(description: "'\(name)' returned a value that is not JSON") }
        return answer
    }

    private func command(_ name: String) -> Run? {
        lock.lock()
        defer { lock.unlock() }
        return commands[name]
    }
}
#endif
