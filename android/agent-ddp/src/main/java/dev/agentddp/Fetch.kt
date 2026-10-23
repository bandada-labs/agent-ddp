package dev.agentddp

import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketException
import java.net.SocketTimeoutException
import java.net.URL
import java.net.UnknownHostException
import java.util.Base64
import java.util.concurrent.ArrayBlockingQueue

/**
 * The CDP Fetch domain at the request stage. Requests through [AgentDdpInterceptor] wait here while a client has them
 * paused, then go on (changed or not), get the client's answer, or fail. Paused requests show redacted, as in Network;
 * a value the client sends back unchanged keeps the real one, so a secret never has to leave the app. No OkHttp types
 * here: the CDP session loads this class in apps without OkHttp.
 */
// ponytail: one client intercepts at a time, the last Fetch.enable wins. Chain them when two clients need it.
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

    fun enable(owner: Any, params: JSONObject, listener: (JSONObject) -> Unit) {
        val patterns = params.optJSONArray("patterns")?.objects()?.mapNotNull(::pattern) ?: listOf(wildcard("*"))
        release {
            this.owner = owner
            this.patterns = patterns
            this.listener = listener
            true
        }
    }

    /** Lets every paused request go on unchanged, as Chrome does when the client goes away. */
    fun disable(owner: Any) = release {
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
        listener(JSONObject().put("requestId", id).put("request", waiting.shown).put("frameId", "app").put("resourceType", "Fetch"))
        return try {
            waiting.answer.take()
        } catch (interrupted: InterruptedException) {
            synchronized(this) { paused.remove(id) }
            Decision.Fail(InterruptedIOException("Interrupted while paused"))
        }
    }

    fun continueRequest(params: JSONObject) = resume(params) { paused ->
        val request = paused.request
        val url = params.optString("url").takeIf { it.isNotEmpty() && it != paused.shown.optString("url") }
        url?.let { runCatching { URL(it) }.getOrNull() ?: throw IllegalArgumentException("Invalid URL") }
        val shownHeaders = paused.shown.getJSONObject("headers")
        val headers = params.optJSONArray("headers")?.objects()?.associate { header ->
            val name = header.optString("name")
            val value = header.optString("value")
            name to if (shownHeaders.has(name) && value == shownHeaders.getString(name)) request.headers[name] ?: value else value
        }
        val body = params.optString("postData").takeIf { it.isNotEmpty() }?.let(::base64)
            ?.takeUnless { it.toString(Charsets.UTF_8) == paused.shown.optString("postData") }
        Decision.Proceed(Sent(url ?: request.url, params.optString("method").ifEmpty { request.method }, headers ?: request.headers,
            body ?: request.body))
    }

    fun fulfillRequest(params: JSONObject) = resume(params) {
        require(params.has("responseCode")) { "Invalid parameters: responseCode" }
        val headers = params.optJSONArray("responseHeaders")?.objects()?.associate { it.optString("name") to it.optString("value") }
        Decision.Fulfill(params.getInt("responseCode"), headers.orEmpty(), base64(params.optString("body")))
    }

    fun failRequest(params: JSONObject) = resume(params) {
        val reason = params.optString("errorReason")
        require(reason in ERROR_REASONS) { "Invalid parameters: errorReason" }
        val message = "net::ERR_" + reason.replace(Regex("([a-z])([A-Z])"), "$1_$2").uppercase()
        Decision.Fail(ERRORS[reason]?.invoke(message) ?: IOException(message))
    }

    /** Decides first, so a bad message leaves the request paused for the next one. */
    private fun resume(params: JSONObject, decide: (Paused) -> Decision): JSONObject {
        val id = params.optString("requestId")
        val found = synchronized(this) { paused[id] } ?: throw IllegalArgumentException("Invalid InterceptionId.")
        val decision = decide(found)
        if (synchronized(this) { paused.remove(id) } != null) found.answer.put(decision)
        return JSONObject()
    }

    private fun release(change: () -> Boolean) {
        val released = synchronized(this) { if (change()) paused.values.toList().also { paused.clear() } else emptyList() }
        released.forEach { it.answer.put(Decision.Proceed(it.request)) }
    }

    private fun pattern(pattern: JSONObject): Regex? {
        val stage = pattern.optString("requestStage")
        require(stage.isEmpty() || stage == "Request") { "Only requestStage Request; answer a request with Fetch.fulfillRequest" }
        val type = pattern.optString("resourceType")
        if (type.isNotEmpty() && type != "Fetch" && type != "XHR") return null
        return wildcard(pattern.optString("urlPattern").ifEmpty { "*" })
    }

    /** Chrome's URL pattern: `*` is any run of characters, `?` one character, a backslash escapes the next one. */
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
            .put("headers", JSONObject(redactor.headers(request.headers))).put("hasPostData", request.body != null)
            .put("initialPriority", "High").put("referrerPolicy", "strict-origin-when-cross-origin")
            .apply { redactor.body(request.body)?.let { put("postData", it) } }
    }

    private fun base64(text: String): ByteArray =
        runCatching { Base64.getDecoder().decode(text) }.getOrNull() ?: throw IllegalArgumentException("Invalid parameters: not base64")

    private fun JSONArray.objects() = (0 until length()).map(::getJSONObject)

    private val ERROR_REASONS = setOf(
        "Failed", "Aborted", "TimedOut", "AccessDenied", "ConnectionClosed", "ConnectionReset", "ConnectionRefused",
        "ConnectionAborted", "ConnectionFailed", "NameNotResolved", "InternetDisconnected", "AddressUnreachable",
        "BlockedByClient", "BlockedByResponse",
    )

    /** The exception the app's code would get from OkHttp for the same failure. */
    private val ERRORS: Map<String, (String) -> IOException> = mapOf(
        "TimedOut" to ::SocketTimeoutException, "NameNotResolved" to ::UnknownHostException,
        "InternetDisconnected" to ::UnknownHostException, "ConnectionRefused" to ::ConnectException,
        "ConnectionFailed" to ::ConnectException, "AddressUnreachable" to ::NoRouteToHostException,
        "ConnectionReset" to ::SocketException, "ConnectionClosed" to ::SocketException, "ConnectionAborted" to ::SocketException,
    )
}
