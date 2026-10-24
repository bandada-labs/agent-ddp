package dev.agentddp

import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap

/**
 * Named actions the app gives agents from its Debug code: log in a test user, reset state, open a screen. An agent
 * lists them with `app.commands` and runs one with `app.run`. Each run has a background thread of its own, so it may
 * block; UI work has to move to the main thread itself.
 */
object AgentDdpCommands {
    private val commands = ConcurrentHashMap<String, (JSONObject) -> Any?>()

    /** [run] gets the agent's params and returns a JSON value: JSONObject, JSONArray, Map, List, String, number, Boolean or null. */
    fun add(name: String, run: (JSONObject) -> Any?) {
        commands[name] = run
    }

    internal fun names(): List<String> = commands.keys.sorted()

    internal fun run(params: JSONObject): JSONObject {
        val name = params.optString("name")
        val command = commands[name] ?: throw IllegalArgumentException("No command '$name'; app.commands lists them")
        val value = command(params.optJSONObject("params") ?: JSONObject())
        val json = JSONObject.wrap(value) ?: throw IllegalArgumentException("'$name' returned a ${value!!::class.java.simpleName}, not JSON")
        return JSONObject().put("value", json)
    }
}
