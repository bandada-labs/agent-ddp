package dev.agentddp

import okhttp3.Interceptor
import okhttp3.Response
import okio.Buffer
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener
import java.util.UUID

/**
 * What the app's Debug network decorator calls so `ddp net` can list requests. The app opts in from its
 * Debug composition root: add [AgentDdpInterceptor] to its OkHttp client, or call [log] from its own
 * port decorator. Everything is redacted before it is kept.
 */
object AgentDdpNetwork {
    /** Literal values (the app's dev secret, a test token) to replace with "[redacted]" wherever they show up. */
    fun redact(vararg values: String) = NetworkLog.redact(values.toList())

    fun log(
        method: String, url: String, requestHeaders: Map<String, String>, requestBody: ByteArray?,
        status: Int?, responseHeaders: Map<String, String>, responseBody: ByteArray?, responseBytes: Long,
        error: String?, startedAtMillis: Long, durationMillis: Long,
    ) = NetworkLog.add(method, url, requestHeaders, requestBody, status, responseHeaders, responseBody,
        responseBytes, error, startedAtMillis, durationMillis)
}

/** OkHttp interceptor for the app's Debug client. Reads at most 16 KB of each body, without consuming it. */
class AgentDdpInterceptor : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val request = chain.request()
        val startedAt = System.currentTimeMillis()
        val requestBody = request.body?.takeUnless { it.isOneShot() || it.isDuplex() }
            ?.let { body -> Buffer().also(body::writeTo).readByteArray() }
        val requestHeaders = request.headers.toMultimap().mapValues { it.value.joinToString() }
        val response = try {
            chain.proceed(request)
        } catch (error: Exception) {
            AgentDdpNetwork.log(request.method, request.url.toString(), requestHeaders, requestBody, null, emptyMap(),
                null, 0, error.toString(), startedAt, System.currentTimeMillis() - startedAt)
            throw error
        }
        val peeked = response.peekBody(Redactor.BODY_LIMIT + 1L).bytes()
        AgentDdpNetwork.log(request.method, request.url.toString(), requestHeaders, requestBody, response.code,
            response.headers.toMultimap().mapValues { it.value.joinToString() }, peeked,
            response.body?.contentLength()?.takeIf { it >= 0 } ?: peeked.size.toLong(), null,
            startedAt, System.currentTimeMillis() - startedAt)
        return response
    }
}

internal object NetworkLog {
    private const val KEPT = 200
    private val entries = ArrayDeque<JSONObject>()
    private val watchers = mutableMapOf<UUID, (JSONObject) -> Boolean>()
    private var nextNumber = 1
    @Volatile private var redactor = Redactor(emptyList())

    fun redact(values: List<String>) { redactor = Redactor(values) }

    fun add(
        method: String, url: String, requestHeaders: Map<String, String>, requestBody: ByteArray?,
        status: Int?, responseHeaders: Map<String, String>, responseBody: ByteArray?, responseBytes: Long,
        error: String?, startedAtMillis: Long, durationMillis: Long,
    ) {
        val redactor = redactor
        val entry = JSONObject().put("type", "request").put("method", method).put("url", redactor.url(url))
            .put("startedAt", startedAtMillis).put("ms", durationMillis)
            .put("requestHeaders", JSONObject(redactor.headers(requestHeaders)))
            .put("requestBytes", requestBody?.size ?: 0).put("responseBytes", responseBytes)
            .put("responseHeaders", JSONObject(redactor.headers(responseHeaders)))
        redactor.body(requestBody)?.let { entry.put("requestBody", it) }
        redactor.body(responseBody)?.let { entry.put("responseBody", it) }
        status?.let { entry.put("status", it) }
        error?.let { entry.put("error", redactor.text(it)) }
        val current = synchronized(this) {
            entry.put("id", "n${nextNumber++}")
            entries.addLast(entry)
            while (entries.size > KEPT) entries.removeFirst()
            watchers.toMap()
        }
        val summary = summary(entry)
        current.filterValues { !it(summary) }.keys.forEach(::stopWatching)
    }

    fun summaries(): List<JSONObject> = synchronized(this) { entries.map(::summary) }

    fun entry(id: String): JSONObject? = synchronized(this) { entries.firstOrNull { it.optString("id") == id } }

    fun watch(watcher: (JSONObject) -> Boolean): UUID = UUID.randomUUID().also { synchronized(this) { watchers[it] = watcher } }

    fun stopWatching(key: UUID) = synchronized(this) { watchers.remove(key) }

    private fun summary(entry: JSONObject) = JSONObject().apply {
        listOf("type", "id", "method", "url", "status", "ms", "responseBytes", "error", "startedAt")
            .filter(entry::has).forEach { put(it, entry.get(it)) }
    }
}

/**
 * Removes secrets before a request is kept: header and query names and JSON keys that look secret,
 * plus literal values the app registers (its dev secret). Bodies are cut at 16 KB.
 */
internal class Redactor(private val values: List<String>) {
    companion object {
        const val BODY_LIMIT = 16 * 1024
        private const val MASK = "[redacted]"
        private val secretName = Regex("secret|token|key|password|attest|authorization|cookie", RegexOption.IGNORE_CASE)
        fun isSecret(name: String) = secretName.containsMatchIn(name)
    }

    fun text(text: String) = values.filter { it.isNotEmpty() }.fold(text) { result, value -> result.replace(value, MASK) }

    fun headers(headers: Map<String, String>) = headers.mapValues { (name, value) -> if (isSecret(name)) MASK else text(value) }

    fun url(url: String): String {
        val query = url.substringAfter('?', "").takeIf { it.isNotEmpty() } ?: return text(url)
        val cleaned = query.split('&').joinToString("&") { pair ->
            val name = pair.substringBefore('=')
            if (isSecret(name)) "$name=$MASK" else pair
        }
        return text(url.substringBefore('?') + "?" + cleaned)
    }

    fun body(bytes: ByteArray?): String? {
        if (bytes == null || bytes.isEmpty()) return null
        val string = bytes.toString(Charsets.UTF_8)
        if (string.contains('�')) return "<${bytes.size} bytes, not text>"
        val json = runCatching { JSONTokener(string).nextValue() }.getOrNull()
        val cleaned = if (json is JSONObject || json is JSONArray) redactJson(json).toString() else string
        return cut(text(cleaned))
    }

    private fun redactJson(value: Any?): Any? = when (value) {
        is JSONObject -> JSONObject().also { result ->
            value.keys().forEach { key -> result.put(key, if (isSecret(key)) MASK else redactJson(value.get(key))) }
        }
        is JSONArray -> JSONArray((0 until value.length()).map { redactJson(value.get(it)) })
        else -> value
    }

    private fun cut(text: String) = if (text.length <= BODY_LIMIT) text else text.take(BODY_LIMIT) + "... [cut at 16 KB]"
}
