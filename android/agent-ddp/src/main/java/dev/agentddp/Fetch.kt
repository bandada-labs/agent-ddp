package dev.agentddp

import org.json.JSONObject
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.net.URL
import java.net.UnknownHostException
import java.util.concurrent.ArrayBlockingQueue

/**
 * Pausing the app's requests. Requests through [AgentDdpInterceptor] wait here while a client intercepts them, then go
 * on (changed or not), get the client's answer, or fail the way the app's code sees a failure. Paused requests show
 * redacted, as in the network log; a value the client sends back unchanged keeps the real one, so a secret never has
 * to leave the app. No OkHttp types here: the control session loads this class in apps without OkHttp.
 */
// ponytail: one client intercepts at a time, the last net.intercept wins. Chain them when two clients need it.
// ponytail: bodies are UTF-8 text. Add base64 when a client needs to send binary bodies.
internal object Fetch {
    class Sent(val url: String, val method: String, val headers: Map<String, String>, val body: ByteArray?)

    sealed interface Decision {
        class Proceed(val request: Sent) : Decision
        class Fulfill(val status: Int, val headers: Map<String, String>, val body: ByteArray) : Decision
        class Fail(val error: IOException) : Decision
    }

    private class Paused(val request: Sent, val shown: JSONObject) {
        val answer = ArrayBlockingQueue<Decision>(1)
    }

    private var owner: Any? = null
    private var patterns: List<Regex> = emptyList()
    private var listener: ((JSONObject) -> Unit)? = null
    private val paused = mutableMapOf<String, Paused>()
    private var nextNumber = 1

    /** `urls` are wildcards; without them every request pauses. */
    fun intercept(owner: Any, params: JSONObject, listener: (JSONObject) -> Unit) {
        val patterns = params.optJSONArray("urls")?.let { urls -> (0 until urls.length()).map { wildcard(urls.getString(it)) } }
            ?: listOf(wildcard("*"))
        release {
            this.owner = owner
            this.patterns = patterns
            this.listener = listener
            true
        }
    }

    /** Lets every paused request go on unchanged. */
    fun release(owner: Any) = release {
        (this.owner === owner).also { isOwner ->
            if (isOwner) {
                this.owner = null
                patterns = emptyList()
                listener = null
            }
        }
    }

    /** Blocks the calling network thread while the request is paused. */
    fun pause(request: Sent): Decision {
        val (id, waiting, listener) = synchronized(this) {
            val listener = listener?.takeIf { patterns.any { it.matches(request.url) } } ?: return Decision.Proceed(request)
            Triple("f${nextNumber++}", Paused(request, shown(request)), listener).also { paused[it.first] = it.second }
        }
        listener(JSONObject().put("id", id).put("request", waiting.shown))
        return try {
            waiting.answer.take()
        } catch (interrupted: InterruptedException) {
            synchronized(this) { paused.remove(id) }
            Decision.Fail(InterruptedIOException("Interrupted while paused"))
        }
    }

    fun proceed(params: JSONObject) = resume(params) { paused ->
        val request = paused.request
        val url = params.optString("url").takeIf { it.isNotEmpty() && it != paused.shown.optString("url") }
        url?.let { runCatching { URL(it) }.getOrNull() ?: throw IllegalArgumentException("Invalid url") }
        val shownHeaders = paused.shown.getJSONObject("headers")
        val headers = params.optJSONObject("headers")?.let { sent ->
            sent.keys().asSequence().associateWith { name ->
                val value = sent.getString(name)
                if (shownHeaders.optString(name, null) == value) request.headers[name] ?: value else value
            }
        }
        val body = params.optString("body", null)?.takeUnless { it == paused.shown.optString("body", null) }?.toByteArray()
        Decision.Proceed(Sent(url ?: request.url, params.optString("method").ifEmpty { request.method }, headers ?: request.headers,
            body ?: request.body))
    }

    fun respond(params: JSONObject) = resume(params) {
        val status = params.optInt("status", -1).takeIf { it in 100..599 } ?: throw IllegalArgumentException("status is 100 to 599")
        val headers = params.optJSONObject("headers")?.let { sent -> sent.keys().asSequence().associateWith(sent::getString) }
        Decision.Fulfill(status, headers.orEmpty(), params.optString("body").toByteArray())
    }

    fun fail(params: JSONObject) = resume(params) {
        val failure = ERRORS[params.optString("error")] ?: throw IllegalArgumentException("error is one of ${ERRORS.keys.joinToString()}")
        Decision.Fail(failure())
    }

    /** Decides first, so a bad message leaves the request paused for the next one. */
    private fun resume(params: JSONObject, decide: (Paused) -> Decision): JSONObject {
        val id = params.optString("id")
        val found = synchronized(this) { paused[id] } ?: throw IllegalArgumentException("No paused request '$id'")
        val decision = decide(found)
        if (synchronized(this) { paused.remove(id) } != null) found.answer.put(decision)
        return JSONObject()
    }

    private fun release(change: () -> Boolean) {
        val released = synchronized(this) { if (change()) paused.values.toList().also { paused.clear() } else emptyList() }
        released.forEach { it.answer.put(Decision.Proceed(it.request)) }
    }

    /** `*` is any run of characters, `?` one character, a backslash escapes the next one. */
    fun wildcard(pattern: String): Regex {
        val regex = StringBuilder()
        var escaped = false
        for (character in pattern) {
            when {
                !escaped && character == '\\' -> { escaped = true; continue }
                !escaped && character == '*' -> regex.append(".*")
                !escaped && character == '?' -> regex.append('.')
                else -> regex.append(Regex.escape(character.toString()))
            }
            escaped = false
        }
        return Regex(regex.toString(), RegexOption.DOT_MATCHES_ALL)
    }

    private fun shown(request: Sent): JSONObject {
        val redactor = NetworkLog.redactor
        return JSONObject().put("url", redactor.url(request.url)).put("method", request.method)
            .put("headers", JSONObject(redactor.headers(request.headers)))
            .apply { redactor.body(request.body)?.let { put("body", it) } }
    }

    /** The exception the app's code would get from OkHttp for the same failure. */
    private val ERRORS: Map<String, () -> IOException> = mapOf(
        "offline" to { UnknownHostException("Unable to resolve host: no network") },
        "host" to { UnknownHostException("Unable to resolve host") },
        "timeout" to { SocketTimeoutException("timeout") },
        "refused" to { ConnectException("Failed to connect") },
        "unreachable" to { NoRouteToHostException("No route to host") },
        "lost" to { SocketException("Connection reset") },
        "cancelled" to { IOException("Canceled") },
        "failed" to { IOException("Request failed") },
    )
}
