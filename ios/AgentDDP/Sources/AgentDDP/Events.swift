#if DEBUG && canImport(UIKit)
import Foundation

/// The events the app sent with `AgentDDP.event`. The last `kept` stay in memory for `events.watch`, and each one goes
/// into History, so `ddp history` shows it next to the frames and it survives a crash. Any thread may send.
final class AppEvents {
    static let shared = AppEvents()

    private static let kept = 200

    private let lock = NSLock()
    private var entries: [[String: Any]] = []
    private var listeners: [UUID: ([String: Any]) -> Void] = [:]

    func send(_ name: String, _ fields: [String: Any]) {
        // A field that is not JSON would break every line it travels in: it goes as its description.
        let json = fields.mapValues { JSONSerialization.isValidJSONObject([$0]) ? $0 : "\($0)" }
        let wall = History.wallMs()
        let entry: [String: Any] = ["t": wall.rounded(), "name": name, "fields": json]
        lock.lock()
        entries.append(entry)
        if entries.count > Self.kept { entries.removeFirst(entries.count - Self.kept) }
        let current = Array(listeners.values)
        lock.unlock()
        History.shared.event("app", ["name": name, "fields": json], wall: wall)
        current.forEach { $0(entry) }
    }

    /// Sends the kept events to `listener`, then each new one, until `stop` runs with the returned id.
    func watch(_ listener: @escaping ([String: Any]) -> Void) -> UUID {
        let id = UUID()
        lock.lock()
        // Registered first, so an event sent while the kept ones go out is not lost; it may come twice at worst.
        listeners[id] = listener
        let kept = entries
        lock.unlock()
        kept.forEach(listener)
        return id
    }

    func stop(_ id: UUID) {
        lock.lock()
        listeners[id] = nil
        lock.unlock()
    }

    func list() -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        return ["events": entries]
    }
}
#endif
