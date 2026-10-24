#if DEBUG && canImport(UIKit)
import Foundation

/// The app's UserDefaults, by suite name; nil (or the bundle id) is the app's own defaults. Values read as text,
/// secrets redacted as in Network. A write keeps the type the key already holds, so a Bool flag stays a Bool, and
/// writing back `[redacted]` leaves the real value.
// ponytail: no change events yet. UserDefaults.didChangeNotification does not say which key; diff when a client watches.
enum Preferences {
    /// The suites the app has written, its own defaults included (by bundle id).
    static func files() -> [String] {
        let directory = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0].appendingPathComponent("Preferences")
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        return names.filter { $0.hasSuffix(".plist") }.map { String($0.dropLast(6)) }.sorted()
    }

    static func read(_ file: String?) throws -> [String: String] {
        let (defaults, domain) = try open(file)
        let redactor = NetworkLog.shared.currentRedactor
        return (defaults.persistentDomain(forName: domain) ?? [:]).reduce(into: [:]) { values, item in
            values[item.key] = Redactor.isSecret(item.key) ? Redactor.mask : redactor.text(shown(item.value))
        }
    }

    static func write(_ file: String?, key: String, text: String) throws {
        if text == Redactor.mask { return }
        let (defaults, domain) = try open(file)
        defaults.set(try typed(text, like: defaults.persistentDomain(forName: domain)?[key], key: key), forKey: key)
    }

    static func remove(_ file: String?, key: String) throws {
        try open(file).defaults.removeObject(forKey: key)
    }

    static func clear(_ file: String?) throws {
        let (defaults, domain) = try open(file)
        defaults.removePersistentDomain(forName: domain)
    }

    /// A value as text: arrays and dictionaries as JSON, dates as ISO 8601, data as base64, so they can be written
    /// back the same way.
    static func shown(_ value: Any) -> String {
        switch value {
        case let text as String: return text
        case let number as NSNumber where CFGetTypeID(number) == CFBooleanGetTypeID(): return number.boolValue ? "true" : "false"
        case let number as NSNumber: return number.stringValue
        case let date as Date: return ISO8601DateFormatter().string(from: date)
        case let data as Data: return data.base64EncodedString()
        default:
            guard JSONSerialization.isValidJSONObject(value),
                  let json = try? JSONSerialization.data(withJSONObject: value, options: .sortedKeys) else { return "\(value)" }
            return String(decoding: json, as: UTF8.self)
        }
    }

    /// The text as the type the key already holds. A new key is a String.
    static func typed(_ text: String, like old: Any?, key: String) throws -> Any {
        let parsed: Any?
        switch old {
        case let number as NSNumber where CFGetTypeID(number) == CFBooleanGetTypeID(): parsed = ["true": true, "false": false][text]
        case is NSNumber: parsed = Int(text) ?? Double(text)
        case is Date: parsed = ISO8601DateFormatter().date(from: text)
        case is Data: parsed = Data(base64Encoded: text)
        case is [Any], is [String: Any]: parsed = try? JSONSerialization.jsonObject(with: Data(text.utf8))
        default: return text
        }
        guard let parsed else { throw Control.Failure(description: "'\(key)' holds another type than '\(text)'") }
        return parsed
    }

    private static func open(_ file: String?) throws -> (defaults: UserDefaults, domain: String) {
        let bundleIdentifier = Bundle.main.bundleIdentifier ?? ""
        // UserDefaults(suiteName:) refuses the app's own bundle id.
        guard let file, file != bundleIdentifier else { return (.standard, bundleIdentifier) }
        guard let defaults = UserDefaults(suiteName: file) else { throw Control.Failure(description: "No preferences '\(file)'") }
        return (defaults, file)
    }
}
#endif
