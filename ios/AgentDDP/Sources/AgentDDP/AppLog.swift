#if DEBUG && canImport(UIKit)
import Foundation
import OSLog

/// The app's own log (os_log, Logger, NSLog). `print` goes to stdout, not the log, so it
/// is not seen. Polls once a second while someone listens.
final class AppLog {
    private static let replayed = 100
    private let queue = DispatchQueue(label: "dev.agentddp.log")
    private let send: ([String: Any]) -> Void
    private var timer: DispatchSourceTimer?
    private var lastDate = Date.distantPast
    private var seenAtLastDate = Set<String>()

    init(send: @escaping ([String: Any]) -> Void) { self.send = send }

    /// Sends the last entries, then each new one.
    func start() {
        queue.async { [self] in
            guard let store = try? OSLogStore(scope: .currentProcessIdentifier) else { return }
            let all = (try? store.getEntries().compactMap { $0 as? OSLogEntryLog }.filter { !Self.isOwnNoise($0) }) ?? []
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
        entries.filter { $0.date > lastDate || ($0.date == lastDate && !seenAtLastDate.contains(key($0))) }
            .filter { !Self.isOwnNoise($0) }.forEach(emit)
    }

    private func emit(_ entry: OSLogEntryLog) {
        if entry.date > lastDate {
            lastDate = entry.date
            seenAtLastDate = []
        }
        seenAtLastDate.insert(key(entry))
        send(Self.logEntry(level: entry.level, category: entry.category, message: entry.composedMessage, date: entry.date))
    }

    /// Lines the library makes the system write: its own control connections (Network.framework), the accessibility
    /// tree it turns on for reading SwiftUI, and reading this log. Hundreds a minute that say nothing about the app.
    static func isOwnNoise(subsystem: String, message: String) -> Bool {
        subsystem == "com.apple.network" || subsystem == "com.apple.Accessibility" || message.contains("OSLogService")
    }

    private static func isOwnNoise(_ entry: OSLogEntryLog) -> Bool { isOwnNoise(subsystem: entry.subsystem, message: entry.composedMessage) }

    private func key(_ entry: OSLogEntryLog) -> String { "\(entry.category)|\(entry.composedMessage)" }

    /// `{t, level, tag, text}`, with the os_log category as the tag, the way `ddp logs` shows it.
    static func logEntry(level: OSLogEntryLog.Level, category: String, message: String, date: Date) -> [String: Any] {
        let levels: [OSLogEntryLog.Level: String] = [.debug: "debug", .info: "info", .notice: "info",
                                                     .error: "error", .fault: "error"]
        return ["t": Int((date.timeIntervalSince1970 * 1000).rounded()), "level": levels[level] ?? "info",
                "tag": category, "text": message]
    }
}
#endif
