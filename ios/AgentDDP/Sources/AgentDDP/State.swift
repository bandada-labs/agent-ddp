#if DEBUG && canImport(UIKit)
import Foundation

/// The named values the app added with `AgentDDP.state`, for `state.list`, `state.get` and `state.set`. Getters and
/// setters run on the main thread, where app state usually lives.
final class AppState {
    static let shared = AppState()

    struct Value {
        let get: () -> Any?
        let set: ((Any?) -> Void)?
    }

    private let lock = NSLock()
    private var values: [String: Value] = [:]

    func add(_ name: String, _ value: Value) {
        lock.lock()
        values[name] = value
        lock.unlock()
    }

    func list() -> [String: Any] {
        let all = snapshot()
        return ["values": all.keys.sorted().map { ["name": $0, "writable": all[$0]!.set != nil] }]
    }

    /// Every value, or the ones `names` lists.
    func get(_ params: [String: Any]) throws -> [String: Any] {
        let all = snapshot()
        let names = params["names"] as? [String] ?? all.keys.sorted()
        let getters = try names.map { name in (name, try value(name, in: all).get) }
        let read = onMain { getters.map { ($0.0, $0.1() ?? NSNull()) } }
        for (name, value) in read where !JSONSerialization.isValidJSONObject([value]) {
            throw Control.Failure(description: "'\(name)' is a \(type(of: value)), not JSON")
        }
        return ["values": Dictionary(read, uniquingKeysWith: { first, _ in first })]
    }

    /// Sets the value, then answers what the getter reads back.
    func set(_ params: [String: Any]) throws -> [String: Any] {
        let name = params["name"] as? String ?? ""
        let value = try value(name, in: snapshot())
        guard let setter = value.set else { throw Control.Failure(description: "'\(name)' is read-only: the app gave no setter") }
        let given = params["value"] is NSNull ? nil : params["value"]
        let read = onMain { () -> Any in
            setter(given)
            return value.get() ?? NSNull()
        }
        guard JSONSerialization.isValidJSONObject([read]) else {
            throw Control.Failure(description: "'\(name)' is a \(type(of: read)), not JSON")
        }
        return ["value": read]
    }

    private func value(_ name: String, in all: [String: Value]) throws -> Value {
        guard let value = all[name] else { throw Control.Failure(description: "No value '\(name)'; state.list lists them") }
        return value
    }

    private func snapshot() -> [String: Value] {
        lock.lock()
        defer { lock.unlock() }
        return values
    }
}
#endif
