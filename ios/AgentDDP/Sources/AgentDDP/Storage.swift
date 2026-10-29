#if DEBUG && canImport(UIKit)
import Foundation
import Security
import SQLite3

/// `storage.*`: the app's own files under its home folder, its SQLite databases read-only, and its Keychain item
/// names. Text is redacted as in Network; Keychain data is never asked for.
enum Storage {
    private static let readLimit = 256 * 1024
    private static let rowLimit = 200
    private static var root: URL { URL(fileURLWithPath: NSHomeDirectory()).resolvingSymlinksInPath() }

    static func files(_ params: [String: Any]) throws -> [String: Any] {
        let path = params["path"] as? String ?? ""
        let folder = try inside(path)
        var isFolder: ObjCBool = false
        guard FileManager.default.fileExists(atPath: folder.path, isDirectory: &isFolder), isFolder.boolValue else {
            throw Control.Failure(description: "'\(path)' is not a folder; storage.files lists them")
        }
        let names = try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
        let entries: [[String: Any]] = names.map { name in
            let url = folder.appendingPathComponent(name)
            let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey])
            var entry: [String: Any] = ["name": name, "folder": values?.isDirectory ?? false]
            if values?.isDirectory != true { entry["bytes"] = values?.fileSize ?? 0 }
            return entry
        }
        return ["path": relative(folder), "entries": entries]
    }

    static func read(_ params: [String: Any]) throws -> [String: Any] {
        let path = params["path"] as? String ?? ""
        let file = try inside(path)
        guard let handle = FileHandle(forReadingAtPath: file.path) else { throw Control.Failure(description: "No file '\(path)'") }
        defer { handle.closeFile() }
        let size = (try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        let bytes = handle.readData(ofLength: readLimit)
        var answer: [String: Any] = ["bytes": size, "truncated": size > readLimit]
        if let text = String(data: bytes, encoding: .utf8) {
            answer["text"] = NetworkLog.shared.currentRedactor.text(text)
        } else {
            answer["binary"] = true
        }
        return answer
    }

    static func remove(_ params: [String: Any]) throws -> [String: Any] {
        let path = params["path"] as? String ?? ""
        let file = try inside(path)
        guard file != root else { throw Control.Failure(description: "Not the whole home folder; ddp launch --reset clears it") }
        guard FileManager.default.fileExists(atPath: file.path) else { throw Control.Failure(description: "No file '\(path)'") }
        try FileManager.default.removeItem(at: file)
        return [:]
    }

    /// One read-only query: the database is opened read-only, so a write fails in SQLite itself.
    static func query(_ params: [String: Any]) throws -> [String: Any] {
        let path = params["path"] as? String ?? ""
        let file = try inside(path)
        var database: OpaquePointer?
        guard FileManager.default.fileExists(atPath: file.path),
              sqlite3_open_v2(file.path, &database, SQLITE_OPEN_READONLY, nil) == SQLITE_OK else {
            sqlite3_close(database)
            throw Control.Failure(description: "No database '\(path)'")
        }
        defer { sqlite3_close(database) }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, params["sql"] as? String ?? "", -1, &statement, nil) == SQLITE_OK else {
            throw Control.Failure(description: String(cString: sqlite3_errmsg(database)))
        }
        defer { sqlite3_finalize(statement) }
        let count = sqlite3_column_count(statement)
        let columns = (0..<count).map { String(cString: sqlite3_column_name(statement, $0)) }
        var rows: [[Any]] = []
        var step = sqlite3_step(statement)
        while step == SQLITE_ROW && rows.count < rowLimit {
            rows.append((0..<count).map { cell(statement, $0, column: columns[Int($0)]) })
            step = sqlite3_step(statement)
        }
        guard step == SQLITE_ROW || step == SQLITE_DONE else { throw Control.Failure(description: String(cString: sqlite3_errmsg(database))) }
        return ["columns": columns, "rows": rows, "more": step == SQLITE_ROW]
    }

    /// The app's Keychain items by class, service and account. Never their data: ddp does not ask for it.
    static func keys() -> [String: Any] {
        let classes: [(CFString, String)] = [(kSecClassGenericPassword, "generic"), (kSecClassInternetPassword, "internet"),
                                             (kSecClassKey, "key"), (kSecClassCertificate, "certificate")]
        var refused: OSStatus?
        let keys: [[String: Any]] = classes.flatMap { secClass, name -> [[String: Any]] in
            let query: [String: Any] = [kSecClass as String: secClass, kSecReturnAttributes as String: true,
                                        kSecMatchLimit as String: kSecMatchLimitAll]
            var result: CFTypeRef?
            let status = SecItemCopyMatching(query as CFDictionary, &result)
            if status != errSecSuccess && status != errSecItemNotFound { refused = status }
            guard status == errSecSuccess, let items = result as? [[String: Any]] else { return [] }
            return items.map { item in
                ["store": name, "service": item[kSecAttrService as String] as? String ?? item[kSecAttrServer as String] as? String ?? "",
                 "name": item[kSecAttrAccount as String] as? String ?? item[kSecAttrLabel as String] as? String ?? ""]
            }
        }
        guard let refused else { return ["keys": keys] }
        // Not "no keys": the Keychain did not answer, so what it holds is unknown.
        let why = refused == errSecMissingEntitlement ? "the app has no Keychain entitlement (an unsigned build)" : "status \(refused)"
        return ["keys": keys, "unread": "the Keychain refused: \(why)"]
    }

    private static func cell(_ statement: OpaquePointer?, _ index: Int32, column: String) -> Any {
        switch sqlite3_column_type(statement, index) {
        case SQLITE_NULL: return NSNull()
        case SQLITE_INTEGER: return sqlite3_column_int64(statement, index)
        case SQLITE_FLOAT: return sqlite3_column_double(statement, index)
        case SQLITE_BLOB: return "<\(sqlite3_column_bytes(statement, index)) bytes>"
        default:
            let text = sqlite3_column_text(statement, index).map { String(cString: $0) } ?? ""
            return Redactor.isSecret(column) ? Redactor.mask : NetworkLog.shared.currentRedactor.text(text)
        }
    }

    /// A path in the home folder; one that leads outside it (`..`, a link) is refused.
    private static func inside(_ path: String) throws -> URL {
        let file = root.appendingPathComponent(path).standardizedFileURL.resolvingSymlinksInPath()
        guard file.path == root.path || file.path.hasPrefix(root.path + "/") else {
            throw Control.Failure(description: "'\(path)' is outside the app's home folder")
        }
        return file
    }

    private static func relative(_ url: URL) -> String {
        url.path == root.path ? "" : String(url.path.dropFirst(root.path.count + 1))
    }
}
#endif
