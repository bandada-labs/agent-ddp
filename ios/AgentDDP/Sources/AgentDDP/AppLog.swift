#if DEBUG && canImport(UIKit)
import Foundation
import OSLog

/// The app's own log (os_log, Logger, NSLog) for CDP `Log.entryAdded`. `print` goes to stdout, not the log, so it
/// is not seen. Polls once a second while someone listens.
final class AppLog {
    private static let replayed = 100
    private let queue = DispatchQueue(label: "dev.agentddp.log")
    private let send: ([String: Any]) -> Void
    private var timer: DispatchSourceTimer?
    private var lastDate = Date.distantPast
    private var seenAtLastDate = Set<String>()

    init(send: @escaping ([String: Any]) -> Void) { self.send = send }

    /// Sends the last entries, as Chrome replays its buffer on `Log.enable`, then each new one.
    func start() {
        queue.async { [self] in
            guard let store = try? OSLogStore(scope: .currentProcessIdentifier) else { return }
            let all = (try? store.getEntries().compactMap { $0 as? OSLogEntryLog }) ?? []
            all.suffix(Self.replayed).forEach(emit)
            let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now() + 1, repeating: 1)
            timer.setEventHandler { [weak self] in self?.poll(store) }
            timer.resume()
            self.timer = timer
        }
    }

    func stop() {
        queue.async { [self] in
            timer?.cancel()
            timer = nil
        }
    }

    private func poll(_ store: OSLogStore) {
        let position = store.position(date: lastDate)
        let entries = (try? store.getEntries(at: position).compactMap { $0 as? OSLogEntryLog }) ?? []
        // The position is not exact and dates repeat, so older entries are dropped and those at the last date are
        // checked by content.
        entries.filter { $0.date > lastDate || ($0.date == lastDate && !seenAtLastDate.contains(key($0))) }.forEach(emit)
    }

    private func emit(_ entry: OSLogEntryLog) {
        if entry.date > lastDate {
            lastDate = entry.date
            seenAtLastDate = []
        }
        seenAtLastDate.insert(key(entry))
        send(Self.logEntry(level: entry.level, category: entry.category, message: entry.composedMessage, date: entry.date))
    }

    private func key(_ entry: OSLogEntryLog) -> String { "\(entry.category)|\(entry.composedMessage)" }

    /// Chrome's `Log.LogEntry`. The category goes before the text, the way `ddp logs` shows it.
    static func logEntry(level: OSLogEntryLog.Level, category: String, message: String, date: Date) -> [String: Any] {
        let levels: [OSLogEntryLog.Level: String] = [.debug: "verbose", .info: "info", .notice: "info",
                                                     .error: "error", .fault: "error"]
        return ["source": "other", "level": levels[level] ?? "info",
                "text": category.isEmpty ? message : "\(category): \(message)",
                "timestamp": (date.timeIntervalSince1970 * 1000).rounded()]
    }
}
#endif
