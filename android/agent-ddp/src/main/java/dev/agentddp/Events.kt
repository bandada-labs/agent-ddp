package dev.agentddp

import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Events the app sends from its Debug code: a rule that fired, a sync that ended, a level that started. The app
 * decides what an event is. The last [KEPT] stay in memory for `events.watch`, and each one goes into History, so
 * `ddp history` shows it next to the frames and it survives a crash. Any thread may send.
 */
object AgentDdpEvents {
    private const val KEPT = 200

    private val kept = ArrayDeque<JSONObject>()
    private val listeners = CopyOnWriteArrayList<(JSONObject) -> Unit>()

    /** [fields] are JSON values: String, number, Boolean, null, List or Map of those. */
    fun send(name: String, fields: Map<String, Any?> = emptyMap()) {
        val wall = System.currentTimeMillis()
        val json = JSONObject(fields)
        val entry = JSONObject().put("t", wall).put("name", name).put("fields", json)
        synchronized(kept) {
            kept.addLast(entry)
            if (kept.size > KEPT) kept.removeFirst()
        }
        History.event("app", JSONObject().put("name", name).put("fields", JSONObject(json.toString())), wall)
        listeners.forEach { it(entry) }
    }

    /** Sends the kept events to [listener], then each new one, until the returned stop runs. */
    internal fun watch(listener: (JSONObject) -> Unit): () -> Unit {
        // Registered first, so an event sent while the kept ones go out is not lost; it may come twice at worst.
        listeners.add(listener)
        synchronized(kept) { kept.toList() }.forEach(listener)
        return { listeners.remove(listener) }
    }

    internal fun list(): JSONObject = JSONObject().put("events", JSONArray(synchronized(kept) { kept.toList() }))
}
