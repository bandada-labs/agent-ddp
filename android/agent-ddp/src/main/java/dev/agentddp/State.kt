package dev.agentddp

import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap

/**
 * Named values the app shows agents from its Debug code: a total, a mode, the logged-in user, a value of its own
 * engine. An agent lists them with `state.list`, reads them with `state.get` and changes the writable ones with
 * `state.set`. Getters and setters run on the main thread, where app state usually lives.
 */
object AgentDdpState {
    private class Value(val get: () -> Any?, val set: ((Any?) -> Unit)?)

    private val values = ConcurrentHashMap<String, Value>()

    /**
     * [get] returns a JSON value: JSONObject, JSONArray, Map, List, String, number, Boolean or null. With [set] the
     * value is writable; it gets what the agent sent, parsed as JSON.
     */
    fun add(name: String, get: () -> Any?, set: ((Any?) -> Unit)? = null) {
        values[name] = Value(get, set)
    }

    internal fun list(): JSONObject = JSONObject().put("values", JSONArray(values.keys.sorted().map { name ->
        JSONObject().put("name", name).put("writable", values.getValue(name).set != null)
    }))

    /** Every value, or the ones [params] names. */
    internal fun get(params: JSONObject): JSONObject {
        val names = params.optJSONArray("names")?.let { array -> List(array.length()) { array.getString(it) } }
            ?: values.keys.sorted()
        val read = JSONObject()
        onMain { names.forEach { name -> read.put(name, json(name, value(name).get())) } }
        return JSONObject().put("values", read)
    }

    /** Sets the value, then answers what the getter reads back. */
    internal fun set(params: JSONObject): JSONObject {
        val name = params.optString("name")
        val value = value(name)
        val setter = value.set ?: throw IllegalArgumentException("'$name' is read-only: the app gave no setter")
        val given = params.opt("value").takeUnless { it == JSONObject.NULL }
        return JSONObject().put("value", onMain {
            setter(given)
            json(name, value.get())
        })
    }

    private fun value(name: String): Value =
        values[name] ?: throw IllegalArgumentException("No value '$name'; state.list lists them")

    private fun json(name: String, value: Any?): Any {
        // JSONObject.wrap would turn any other object into its toString() or null, which hides the mistake.
        if (value != null && value !is JSONObject && value !is JSONArray && value !is Map<*, *> && value !is Collection<*> &&
            value !is String && value !is Number && value !is Boolean) {
            throw IllegalArgumentException("'$name' is a ${value::class.java.simpleName}, not JSON")
        }
        return JSONObject.wrap(value) ?: JSONObject.NULL
    }
}
