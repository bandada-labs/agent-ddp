package dev.agentddp

import android.app.Application
import android.content.Context
import android.content.SharedPreferences
import org.json.JSONArray

/**
 * The app's SharedPreferences, by file name; null is the default file. Values read as text, secrets redacted as in
 * Network. A write keeps the type the key already holds, so a Boolean flag stays a Boolean, and writing back
 * `[redacted]` leaves the real value.
 */
// ponytail: no change events yet. Add them with OnSharedPreferenceChangeListener when a client watches.
internal object Preferences {
    lateinit var application: Application

    /** The preferences files the app has written, the default one included. */
    fun files(): List<String> =
        java.io.File(application.applicationInfo.dataDir, "shared_prefs").list().orEmpty()
            .filter { it.endsWith(".xml") }.map { it.removeSuffix(".xml") }.sorted()

    fun read(file: String?): List<Pair<String, String>> {
        val redactor = NetworkLog.redactor
        return open(file).all.toSortedMap().map { (key, value) ->
            key to if (Redactor.isSecret(key)) Redactor.MASK else redactor.text(shown(value))
        }
    }

    fun write(file: String?, key: String, text: String) {
        if (text == Redactor.MASK) return
        val preferences = open(file)
        val editor = preferences.edit()
        when (preferences.all[key]) {
            is Boolean -> editor.putBoolean(key, text.toBooleanStrictOrNull() ?: throw notA("Boolean", key))
            is Int -> editor.putInt(key, text.toIntOrNull() ?: throw notA("Int", key))
            is Long -> editor.putLong(key, text.toLongOrNull() ?: throw notA("Long", key))
            is Float -> editor.putFloat(key, text.toFloatOrNull() ?: throw notA("Float", key))
            is Set<*> -> editor.putStringSet(key, stringSet(text) ?: throw notA("Set<String> as a JSON array", key))
            else -> editor.putString(key, text)
        }
        editor.commit()
    }

    fun remove(file: String?, key: String) {
        open(file).edit().remove(key).commit()
    }

    fun clear(file: String?) {
        open(file).edit().clear().commit()
    }

    /** A value as text: a string set as a JSON array, so it can be written back the same way. */
    fun shown(value: Any?): String = when (value) {
        is Set<*> -> JSONArray(value.map { it.toString() }.sorted()).toString()
        else -> value.toString()
    }

    fun stringSet(text: String): Set<String>? = runCatching {
        JSONArray(text).let { array -> (0 until array.length()).map { array.get(it) as String }.toSet() }
    }.getOrNull()

    private fun open(file: String?): SharedPreferences =
        application.getSharedPreferences(file ?: "${application.packageName}_preferences", Context.MODE_PRIVATE)

    private fun notA(type: String, key: String) = IllegalArgumentException("'$key' holds a $type")
}
