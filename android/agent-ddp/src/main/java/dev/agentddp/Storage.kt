package dev.agentddp

import android.app.Application
import android.database.Cursor
import android.database.sqlite.SQLiteDatabase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.KeyStore

/**
 * `storage.*`: the app's own files under its data folder, its SQLite databases read-only, and its Keystore key names.
 * Text is redacted as in Network; a key's material never leaves the Keystore, and ddp does not ask for it.
 */
internal object Storage {
    private const val READ_LIMIT = 256 * 1024
    private const val ROW_LIMIT = 200

    private lateinit var root: File

    fun start(application: Application) {
        root = application.dataDir.canonicalFile
    }

    fun files(params: JSONObject): JSONObject {
        val folder = inside(params.optString("path"))
        if (!folder.isDirectory) throw IllegalArgumentException("'${params.optString("path")}' is not a folder; storage.files lists them")
        val entries = folder.listFiles().orEmpty().sortedBy { it.name }.map { file ->
            JSONObject().put("name", file.name).put("folder", file.isDirectory).apply { if (file.isFile) put("bytes", file.length()) }
        }
        return JSONObject().put("path", folder.relativeTo(root).path).put("entries", JSONArray(entries))
    }

    fun read(params: JSONObject): JSONObject {
        val file = inside(params.getString("path"))
        if (!file.isFile) throw IllegalArgumentException("No file '${params.getString("path")}'")
        val bytes = ByteArray(minOf(file.length(), READ_LIMIT.toLong()).toInt())
        file.inputStream().use { input ->
            var read = 0
            while (read < bytes.size) read += input.read(bytes, read, bytes.size - read).takeIf { it > 0 } ?: break
        }
        val text = bytes.toString(Charsets.UTF_8)
        val answer = JSONObject().put("bytes", file.length()).put("truncated", file.length() > READ_LIMIT)
        // A replacement character means the bytes were not UTF-8: a binary file says only its size.
        return if (text.contains('�')) answer.put("binary", true) else answer.put("text", NetworkLog.redactor.text(text))
    }

    fun remove(params: JSONObject): JSONObject {
        val file = inside(params.getString("path"))
        if (file == root) throw IllegalArgumentException("Not the whole data folder; ddp launch --reset clears it")
        if (!file.exists()) throw IllegalArgumentException("No file '${params.getString("path")}'")
        if (!file.deleteRecursively()) throw IllegalStateException("Could not remove '${params.getString("path")}'")
        return JSONObject()
    }

    /** One read-only query: the database is opened read-only, so a write fails in SQLite itself. */
    fun query(params: JSONObject): JSONObject {
        val file = inside(params.getString("path"))
        if (!file.isFile) throw IllegalArgumentException("No database '${params.getString("path")}'")
        SQLiteDatabase.openDatabase(file.path, null, SQLiteDatabase.OPEN_READONLY).use { database ->
            database.rawQuery(params.getString("sql"), null).use { cursor ->
                val rows = JSONArray()
                while (rows.length() < ROW_LIMIT && cursor.moveToNext()) {
                    rows.put(JSONArray((0 until cursor.columnCount).map { column -> cell(cursor, column) }))
                }
                return JSONObject().put("columns", JSONArray(cursor.columnNames.toList())).put("rows", rows)
                    .put("more", !cursor.isAfterLast && cursor.moveToNext())
            }
        }
    }

    /** The app's Keystore aliases. Never key material: the Keystore would not give it, and ddp does not ask. */
    fun keys(): JSONObject {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        return JSONObject().put("keys", JSONArray(store.aliases().toList().sorted().map { JSONObject().put("name", it).put("store", "AndroidKeyStore") }))
    }

    private fun cell(cursor: Cursor, column: Int): Any = when (cursor.getType(column)) {
        Cursor.FIELD_TYPE_NULL -> JSONObject.NULL
        Cursor.FIELD_TYPE_INTEGER -> cursor.getLong(column)
        Cursor.FIELD_TYPE_FLOAT -> cursor.getDouble(column)
        Cursor.FIELD_TYPE_BLOB -> "<${cursor.getBlob(column).size} bytes>"
        else -> cursor.getString(column).let { if (Redactor.isSecret(cursor.getColumnName(column))) Redactor.MASK else NetworkLog.redactor.text(it) }
    }

    /** A path in the data folder; one that leads outside it (`..`, a link) is refused. */
    private fun inside(path: String): File {
        val file = File(root, path).canonicalFile
        if (file != root && !file.path.startsWith(root.path + File.separator)) {
            throw IllegalArgumentException("'$path' is outside the app's data folder")
        }
        return file
    }
}
