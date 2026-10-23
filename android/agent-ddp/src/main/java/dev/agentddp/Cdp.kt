package dev.agentddp

import android.app.Application
import android.graphics.Bitmap
import android.graphics.Canvas
import android.net.LocalServerSocket
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.util.Base64
import kotlin.concurrent.thread

/**
 * The Chrome DevTools Protocol endpoint (ADR 0007): HTTP discovery on `/json/version` and `/json/list`, then
 * JSON-RPC over a WebSocket. The socket name ends in `_devtools_remote`, which is what `chrome://inspect` looks for.
 */
internal object CdpServer {
    fun start(application: Application) {
        val target = CdpTarget(
            packageName = application.packageName,
            title = application.applicationInfo.loadLabel(application.packageManager).toString(),
        )
        thread(name = "agentddp-cdp", isDaemon = true) {
            val server = runCatching { LocalServerSocket("agentddp_${target.packageName}_devtools_remote") }.getOrNull()
                ?: return@thread
            while (true) {
                val socket = runCatching { server.accept() }.getOrNull() ?: continue
                if (socket.peerCredentials.uid !in LiveServer.ALLOWED_UIDS) {
                    socket.close()
                    continue
                }
                thread(name = "agentddp-cdp-session", isDaemon = true) {
                    socket.use { CdpConnection(BufferedInputStream(it.inputStream), it.outputStream, target).run() }
                }
            }
        }
    }
}

internal class CdpTarget(val packageName: String, val title: String) {
    val id = "app"
    val url = "android-app://$packageName"

    fun info() = JSONObject().put("targetId", id).put("type", "page").put("title", title).put("url", url)
        .put("attached", false).put("canAccessOpener", false)
}

/** One HTTP request: a discovery answer and close, or a WebSocket upgrade that then carries CDP messages. */
internal class CdpConnection(
    private val input: InputStream,
    private val output: OutputStream,
    private val target: CdpTarget,
) {
    fun run() {
        val request = HttpRequest.read(input) ?: return
        val host = request.headers["host"] ?: "localhost"
        when {
            request.headers["upgrade"].equals("websocket", ignoreCase = true) -> upgrade(request)
            request.path == "/json/version" -> json(Cdp.version(target).put("webSocketDebuggerUrl", "ws://$host/devtools/browser/${target.id}"))
            request.path == "/json" || request.path == "/json/list" -> json(JSONArray().put(
                JSONObject().put("id", target.id).put("type", "page").put("title", target.title).put("url", target.url)
                    .put("description", "Agent-DDP").put("webSocketDebuggerUrl", "ws://$host/devtools/page/${target.id}")
            ))
            else -> respond("404 Not Found", "text/plain", "Unknown path ${request.path}")
        }
    }

    private fun upgrade(request: HttpRequest) {
        val key = request.headers["sec-websocket-key"] ?: return respond("400 Bad Request", "text/plain", "No Sec-WebSocket-Key")
        if (!Cdp.originAllowed(request.headers["origin"])) {
            return respond("403 Forbidden", "text/plain", "Rejected the WebSocket connection from origin ${request.headers["origin"]}")
        }
        output.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Accept: ${WebSocket.acceptKey(key)}\r\n\r\n").toByteArray())
        output.flush()
        val session = CdpSession(target, browserLevel = request.path.startsWith("/devtools/browser")) { message -> synchronized(output) { WebSocket.writeText(output, message) } }
        try {
            while (true) {
                val frame = runCatching { WebSocket.readFrame(input) }.getOrNull() ?: return
                when (frame.opcode) {
                    WebSocket.TEXT -> session.receive(String(frame.payload))
                    WebSocket.PING -> synchronized(output) { WebSocket.write(output, WebSocket.PONG, frame.payload) }
                    WebSocket.CLOSE -> return synchronized(output) { WebSocket.write(output, WebSocket.CLOSE, frame.payload) }
                }
            }
        } finally {
            session.close()
        }
    }

    private fun json(body: Any) = respond("200 OK", "application/json; charset=UTF-8", body.toString())

    private fun respond(status: String, type: String, body: String) {
        val bytes = body.toByteArray()
        output.write("HTTP/1.1 $status\r\nContent-Type: $type\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray())
        output.write(bytes)
        output.flush()
    }
}

internal class HttpRequest(val path: String, val headers: Map<String, String>) {
    companion object {
        private const val MAX_HEADER_BYTES = 16_384

        /** Reads up to the blank line and no further, so WebSocket frames that follow stay in the stream. */
        fun read(input: InputStream): HttpRequest? {
            val lines = mutableListOf<String>()
            val line = ByteArrayOutputStream()
            var total = 0
            while (total++ < MAX_HEADER_BYTES) {
                val byte = input.read()
                if (byte < 0) return null
                if (byte != '\n'.code) {
                    if (byte != '\r'.code) line.write(byte)
                    continue
                }
                if (line.size() == 0) break
                lines += line.toString()
                line.reset()
            }
            val target = lines.firstOrNull()?.split(" ")?.getOrNull(1) ?: return null
            val headers = lines.drop(1).mapNotNull { header ->
                header.split(":", limit = 2).takeIf { it.size == 2 }?.let { (name, value) -> name.trim().lowercase() to value.trim() }
            }.toMap()
            return HttpRequest(target.substringBefore("?"), headers)
        }
    }
}

/** RFC 6455, only what CDP needs: text, ping, pong and close frames. Clients mask, the server does not. */
internal object WebSocket {
    const val CONTINUATION = 0
    const val TEXT = 1
    const val CLOSE = 8
    const val PING = 9
    const val PONG = 10
    private const val MAX_MESSAGE_BYTES = 64 * 1024 * 1024
    private const val GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

    class Frame(val opcode: Int, val payload: ByteArray)

    fun acceptKey(key: String): String =
        Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-1").digest((key + GUID).toByteArray()))

    /** One message: continuation frames are joined, so the caller always sees whole messages. */
    fun readFrame(input: InputStream): Frame {
        var opcode = -1
        val message = ByteArrayOutputStream()
        while (true) {
            val first = input.readByte()
            val second = input.readByte()
            var length = (second and 0x7F).toLong()
            if (length == 126L) length = input.readNumber(2)
            if (length == 127L) length = input.readNumber(8)
            if (message.size() + length > MAX_MESSAGE_BYTES) throw EOFException("message too large")
            val mask = if (second and 0x80 != 0) ByteArray(4) { input.readByte().toByte() } else null
            val payload = ByteArray(length.toInt()) { input.readByte().toByte() }
            mask?.let { payload.indices.forEach { index -> payload[index] = (payload[index].toInt() xor mask[index % 4].toInt()).toByte() } }
            val frameOpcode = first and 0x0F
            // Control frames may arrive between the pieces of a fragmented message.
            if (frameOpcode >= CLOSE) return Frame(frameOpcode, payload)
            if (frameOpcode != CONTINUATION) opcode = frameOpcode
            message.write(payload)
            if (first and 0x80 != 0) return Frame(opcode, message.toByteArray())
        }
    }

    fun writeText(output: OutputStream, text: String) = write(output, TEXT, text.toByteArray())

    fun write(output: OutputStream, opcode: Int, payload: ByteArray) {
        output.write(0x80 or opcode)
        when {
            payload.size < 126 -> output.write(payload.size)
            payload.size < 65_536 -> {
                output.write(126)
                output.write(payload.size shr 8)
                output.write(payload.size and 0xFF)
            }
            else -> {
                output.write(127)
                (7 downTo 0).forEach { output.write((payload.size.toLong() shr (it * 8)).toInt() and 0xFF) }
            }
        }
        output.write(payload)
        output.flush()
    }

    private fun InputStream.readByte(): Int = read().also { if (it < 0) throw EOFException() }

    private fun InputStream.readNumber(bytes: Int): Long = (0 until bytes).fold(0L) { number, _ -> (number shl 8) or readByte().toLong() }
}

/**
 * Answers CDP messages for one client. The same error codes and messages as Chrome, so clients behave the same.
 * [browserLevel]: connected to `/devtools/browser`, as clients that drive several targets do. A page connection is the page.
 */
internal class CdpSession(
    private val target: CdpTarget,
    private val browserLevel: Boolean = false,
    private val send: (String) -> Unit,
) {
    /** Flat sessions: the app is one page, so every session id reaches the same handlers and only goes back on answers. */
    private val sessionIds = mutableSetOf<String>()
    /** The log watch feeding Network events. */
    private var networkWatch: java.util.UUID? = null
    /** The `sessionId` of the message being answered. */
    private var currentSessionId: Any? = null
    private var appLog: AppLog? = null

    private val methods: Map<String, (JSONObject) -> JSONObject> = mapOf<String, (JSONObject) -> JSONObject>(
        "Browser.getVersion" to { _ -> Cdp.browserVersion(target) },
        "Target.getTargets" to { _ -> JSONObject().put("targetInfos", JSONArray().put(target.info())) },
        "Target.getTargetInfo" to { _ -> JSONObject().put("targetInfo", target.info()) },
        "Target.setDiscoverTargets" to { _ -> JSONObject() },
        "Target.attachToTarget" to { _ -> JSONObject().put("sessionId", attach()) },
        "Target.setAutoAttach" to { params ->
            // Chrome's browser target auto-attaches its pages; a page has no child targets here.
            if (browserLevel && params.optBoolean("autoAttach") && sessionIds.isEmpty()) attach()
            JSONObject()
        },
        "Target.detachFromTarget" to { params -> sessionIds.remove(params.optString("sessionId")); JSONObject() },
        "Schema.getDomains" to { _ -> JSONObject().put("domains", JSONArray(Cdp.DOMAINS.map { JSONObject().put("name", it).put("version", Cdp.PROTOCOL_VERSION) })) },
        "Runtime.runIfWaitingForDebugger" to { _ -> JSONObject() },
        "Accessibility.getFullAXTree" to { _ -> JSONObject().put("nodes", Cdp.axTree(target.title, Dom.read(target.title).elements)) },
        "Page.captureScreenshot" to { params -> Cdp.screenshot(params) },
        "Network.enable" to { _ ->
            stopNetwork()
            val sessionId = currentSessionId
            // Runs on the app's network thread: a closed socket must not throw into the app's request.
            networkWatch = NetworkLog.watch { summary ->
                runCatching {
                    val entry = NetworkLog.entry(summary.optString("id")) ?: return@runCatching
                    Cdp.networkEvents(entry).forEach { (method, params) -> event(method, params, sessionId) }
                }.isSuccess
            }
            JSONObject()
        },
        "Network.disable" to { _ -> stopNetwork(); JSONObject() },
        "Log.enable" to { _ ->
            appLog?.stop()
            val sessionId = currentSessionId
            appLog = AppLog { entry -> runCatching { event("Log.entryAdded", JSONObject().put("entry", entry), sessionId) } }
                .also { it.start() }
            JSONObject()
        },
        "Log.disable" to { _ -> appLog?.stop(); appLog = null; JSONObject() },
        "Log.clear" to { _ -> JSONObject() },
        "Network.getResponseBody" to { params ->
            JSONObject().put("body", Cdp.networkEntry(params).optString("responseBody")).put("base64Encoded", false)
        },
        "Network.getRequestPostData" to { params ->
            JSONObject().put("postData", Cdp.networkEntry(params).optString("requestBody").ifEmpty { null }
                ?: throw IllegalStateException("No post data available for the request"))
        },
        "DOM.getDocument" to { params -> JSONObject().put("root", Dom.read(target.title).document(params.optInt("depth", 1))) },
        "DOM.requestChildNodes" to { _ -> JSONObject() },
        "DOM.describeNode" to { params -> JSONObject().put("node", Dom.read(target.title).node(Dom.nodeId(params))) },
        "DOM.getAttributes" to { params ->
            JSONObject().put("attributes", Dom.read(target.title).node(Dom.nodeId(params)).optJSONArray("attributes") ?: JSONArray())
        },
        "DOM.getOuterHTML" to { params -> JSONObject().put("outerHTML", Dom.read(target.title).outerHTML(Dom.nodeId(params))) },
        "DOM.querySelector" to { params ->
            JSONObject().put("nodeId", Dom.read(target.title).query(params.optString("selector")).firstOrNull() ?: 0)
        },
        "DOM.querySelectorAll" to { params -> JSONObject().put("nodeIds", JSONArray(Dom.read(target.title).query(params.optString("selector")))) },
        "DOM.getBoxModel" to { params -> JSONObject().put("model", Dom.read(target.title).boxModel(Dom.nodeId(params))) },
    ) + Cdp.TOGGLED_DOMAINS.flatMap { listOf("$it.enable", "$it.disable") }.associateWith { { _: JSONObject -> JSONObject() } }

    /** The connection closed: stop feeding it events. */
    fun close() {
        stopNetwork()
        appLog?.stop()
    }

    private fun stopNetwork() {
        networkWatch?.let(NetworkLog::stopWatching)
        networkWatch = null
    }

    private fun event(method: String, params: JSONObject, sessionId: Any?) =
        send(JSONObject().put("method", method).put("params", params).apply { sessionId?.let { put("sessionId", it) } }.toString())

    private fun attach(): String {
        val sessionId = java.util.UUID.randomUUID().toString().replace("-", "").uppercase()
        sessionIds += sessionId
        send(JSONObject().put("method", "Target.attachedToTarget").put("params", JSONObject().put("sessionId", sessionId)
            .put("targetInfo", target.info().put("attached", true)).put("waitingForDebugger", false)).toString())
        return sessionId
    }

    fun receive(text: String) {
        val message = runCatching { JSONObject(text) }.getOrNull()
            ?: return send(Cdp.error(null, -32700, "Message must be a valid JSON").toString())
        val id = message.opt("id") as? Int
            ?: return send(Cdp.error(null, -32600, "Message must have integer 'id' property").toString())
        val method = message.optString("method")
        currentSessionId = message.opt("sessionId")
        val handler = methods[method] ?: return send(Cdp.error(id, -32601, "'$method' wasn't found").toString())
        val answer = runCatching { JSONObject().put("id", id).put("result", handler(message.optJSONObject("params") ?: JSONObject())) }
            .getOrElse { Cdp.error(id, -32000, it.message ?: it.toString()) }
        message.opt("sessionId")?.let { answer.put("sessionId", it) }
        send(answer.toString())
    }
}

/**
 * The DOM over the captured elements: `#document > html > body`, then one node per element in reading order, the same
 * order as the accessibility tree. The tag is the element's role (`text` for StaticText), and every node has the same
 * attributes on both platforms: `role`, `id`, `label`. Frames are in dp, Android's CSS pixel. Node ids are positions in
 * one read, so they hold only until the screen changes.
 */
// ponytail: flat, no view hierarchy and no text nodes. Add the hierarchy when a client needs ancestors.
internal class Dom(private val title: String, val elements: JSONArray, private val density: Float = 1f) {
    companion object {
        const val DOCUMENT_ID = 1
        const val HTML_ID = 2
        const val BODY_ID = 3
        const val FIRST_ELEMENT_ID = 4

        fun read(title: String): Dom = onMain {
            val density = FrameRecorder.resumedActivity?.resources?.displayMetrics?.density ?: 1f
            Dom(title, FrameCapture.frame(0, 0.0).getJSONArray("elements"), density)
        }

        /** `nodeId` or `backendNodeId`, which are the same number here. */
        fun nodeId(params: JSONObject): Int = (params.opt("nodeId") ?: params.opt("backendNodeId")) as? Int
            ?: throw IllegalArgumentException("Either nodeId, backendNodeId or objectId must be specified")
    }

    private val elementIds = FIRST_ELEMENT_ID until FIRST_ELEMENT_ID + elements.length()

    /** [depth] -1 is the whole tree, as in Chrome. */
    fun document(depth: Int): JSONObject = node(DOCUMENT_ID, depth)

    fun node(id: Int): JSONObject {
        if (id < DOCUMENT_ID || id > elementIds.last.coerceAtLeast(BODY_ID)) throw IllegalArgumentException("Could not find node with given id")
        return node(id, depth = 0)
    }

    private fun node(id: Int, depth: Int): JSONObject {
        val childIds = when (id) {
            DOCUMENT_ID -> listOf(HTML_ID)
            HTML_ID -> listOf(BODY_ID)
            BODY_ID -> elementIds.toList()
            else -> emptyList()
        }
        val node = JSONObject().put("nodeId", id).put("backendNodeId", id).put("childNodeCount", childIds.size)
        if (id == DOCUMENT_ID) {
            node.put("nodeType", 9).put("nodeName", "#document").put("localName", "").put("nodeValue", "")
                .put("documentURL", "").put("baseURL", "")
        } else {
            val tag = tag(id)
            node.put("nodeType", 1).put("nodeName", tag.uppercase()).put("localName", tag).put("nodeValue", "")
                .put("attributes", JSONArray(attributes(id).flatMap { listOf(it.first, it.second) }))
                .put("parentId", when (id) { HTML_ID -> DOCUMENT_ID; BODY_ID -> HTML_ID; else -> BODY_ID })
        }
        if (depth != 0) node.put("children", JSONArray(childIds.map { node(it, depth - 1) }))
        return node
    }

    private fun element(id: Int): JSONObject? = if (id in elementIds) elements.getJSONObject(id - FIRST_ELEMENT_ID) else null

    private fun tag(id: Int): String = when (id) {
        HTML_ID -> "html"
        BODY_ID -> "body"
        else -> element(id)?.optString("role", "generic").let { if (it == "StaticText") "text" else it ?: "generic" }
    }

    private fun attributes(id: Int): List<Pair<String, String>> {
        val element = element(id) ?: return if (id == HTML_ID) listOf("title" to title) else emptyList()
        return listOf("role", "id", "label").mapNotNull { name -> element.optString(name).takeIf { element.has(name) }?.let { name to it } }
    }

    fun outerHTML(id: Int): String {
        node(id)
        val inner = when (id) {
            DOCUMENT_ID -> outerHTML(HTML_ID)
            HTML_ID -> outerHTML(BODY_ID)
            BODY_ID -> elementIds.joinToString("") { outerHTML(it) }
            else -> escape(element(id)?.optString("label").orEmpty())
        }
        if (id == DOCUMENT_ID) return inner
        val tag = tag(id)
        return "<$tag${attributes(id).joinToString("") { " ${it.first}=\"${escape(it.second)}\"" }}>$inner</$tag>"
    }

    private fun escape(text: String) = text.replace("&", "&amp;").replace("<", "&lt;").replace("\"", "&quot;")

    /** `DOM.getBoxModel`: no padding, border or margin, so the four quads are the frame. */
    fun boxModel(id: Int): JSONObject {
        node(id)
        val frame = element(id)?.optJSONArray("frame") ?: throw IllegalStateException("Could not compute box model.")
        val (x, y, width, height) = (0 until 4).map { round2(frame.getDouble(it) / density) }
        val quad = JSONArray(listOf(x, y, x + width, y, x + width, y + height, x, y + height).map(::round2))
        return JSONObject().put("content", quad).put("padding", quad).put("border", quad).put("margin", quad)
            .put("width", width).put("height", height)
    }

    /**
     * Selector lists of compound selectors: `button`, `#saveButton`, `[label='Save']`, `button[label^='Sa']`, `*`.
     * Attribute operators `=`, `^=`, `$=`, `*=`. Combinators throw, since the tree is flat.
     */
    fun query(selector: String): List<Int> {
        val compounds = selector.split(",").map(::SimpleSelector)
        return elementIds.filter { id ->
            val attributes = attributes(id).toMap()
            compounds.any { it.matches(tag(id), attributes) }
        }
    }
}

internal class SimpleSelector(text: String) {
    private val tag: String?
    private val conditions: List<Triple<String, String, String?>>

    init {
        val match = COMPOUND.matchEntire(text.trim())?.takeIf { text.isNotBlank() } ?: throw IllegalArgumentException("DOM Error while querying")
        tag = match.groupValues[1].lowercase().takeUnless { it.isEmpty() || it == "*" }
        conditions = PART.findAll(match.groupValues[2]).map { part ->
            val (id, name, operation, value) = part.destructured
            if (id.isNotEmpty()) Triple("id", "=", id)
            else Triple(name, operation, value.takeIf { it.isNotEmpty() }?.removeSurrounding("'")?.removeSurrounding("\""))
        }.toList()
    }

    fun matches(tag: String, attributes: Map<String, String>): Boolean {
        if (this.tag != null && this.tag != tag) return false
        return conditions.all { (name, operation, value) ->
            val actual = attributes[name] ?: return@all false
            when {
                value == null -> true
                operation == "^=" -> actual.startsWith(value)
                operation == "$=" -> actual.endsWith(value)
                operation == "*=" -> value in actual
                else -> actual == value
            }
        }
    }

    private companion object {
        val COMPOUND = Regex("""([a-zA-Z*][\w-]*)?((?:#[\w-]+|\[\s*[\w-]+\s*(?:[\^$*]?=\s*(?:'[^']*'|"[^"]*"|[^\]\s]+))?\s*])*)""")
        val PART = Regex("""#([\w-]+)|\[\s*([\w-]+)\s*(?:([\^$*]?=)\s*('[^']*'|"[^"]*"|[^\]\s]+))?\s*]""")
    }
}

internal object Cdp {
    const val PROTOCOL_VERSION = "1.3"
    val DOMAINS = listOf("Accessibility", "Browser", "DOM", "Log", "Network", "Page", "Runtime", "Schema", "Target")
    /** Domains whose enable and disable only switch events on and off. */
    val TOGGLED_DOMAINS = listOf("Accessibility", "DOM", "Page", "Runtime")

    private const val PRODUCT = "Agent-DDP/${FrameCapture.PROTOCOL}"

    /** `/json/version`. */
    fun version(target: CdpTarget) = JSONObject()
        .put("Browser", PRODUCT).put("Protocol-Version", PROTOCOL_VERSION)
        .put("User-Agent", userAgent(target)).put("Android-Package", target.packageName)

    /** `Browser.getVersion`. */
    fun browserVersion(target: CdpTarget) = JSONObject()
        .put("protocolVersion", PROTOCOL_VERSION).put("product", PRODUCT).put("revision", "")
        .put("userAgent", userAgent(target)).put("jsVersion", "")

    private fun userAgent(target: CdpTarget) = "Agent-DDP Android ${target.packageName}"

    /**
     * `Accessibility.getFullAXTree`: a root like Chrome's RootWebArea, then the elements in reading order.
     * Ids are positions in this read, so they hold only until the screen changes.
     */
    fun axTree(title: String, elements: JSONArray): JSONArray {
        val children = (0 until elements.length()).map { index ->
            val element = elements.getJSONObject(index)
            axNode(index + 2, element.optString("role", "generic"), element.optString("label", element.optString("id")))
                .put("parentId", "1").put("backendDOMNodeId", Dom.FIRST_ELEMENT_ID + index)
        }
        return JSONArray().put(axNode(1, "RootWebArea", title).put("childIds", JSONArray(children.map { it.getString("nodeId") })))
            .apply { children.forEach(::put) }
    }

    private fun axNode(id: Int, role: String, name: String) = JSONObject().put("nodeId", id.toString())
        .put("backendDOMNodeId", id).put("ignored", false).put("childIds", JSONArray())
        .put("role", JSONObject().put("type", "role").put("value", role))
        .put("name", JSONObject().put("type", "computedString").put("value", name))

    /**
     * One finished request from the network log as Chrome's events, in Chrome's order. The log only hears of a request
     * when it ends, so all of them arrive together; the timestamps are when it started and ended (seconds since 1970).
     */
    fun networkEvents(entry: JSONObject): List<Pair<String, JSONObject>> {
        val id = entry.optString("id")
        val started = entry.optDouble("startedAt") / 1000
        val ended = started + entry.optDouble("ms") / 1000
        val url = entry.optString("url")
        val request = JSONObject().put("url", url).put("method", entry.optString("method", "GET"))
            .put("headers", entry.optJSONObject("requestHeaders") ?: JSONObject()).put("hasPostData", entry.has("requestBody"))
        entry.opt("requestBody")?.let { request.put("postData", it) }
        val events = mutableListOf("Network.requestWillBeSent" to JSONObject().put("requestId", id).put("loaderId", "")
            .put("documentURL", url).put("request", request).put("timestamp", started).put("wallTime", started)
            .put("initiator", JSONObject().put("type", "other")).put("type", "Fetch").put("redirectHasExtraInfo", false))
        if (entry.has("status")) {
            val headers = entry.optJSONObject("responseHeaders") ?: JSONObject()
            val contentType = headers.keys().asSequence().firstOrNull { it.equals("content-type", ignoreCase = true) }
                ?.let(headers::optString).orEmpty()
            events += "Network.responseReceived" to JSONObject().put("requestId", id).put("loaderId", "").put("timestamp", ended)
                .put("type", "Fetch").put("hasExtraInfo", false).put("response", JSONObject().put("url", url)
                    .put("status", entry.getInt("status")).put("statusText", "").put("headers", headers)
                    .put("mimeType", contentType.substringBefore(";").trim()).put("connectionReused", false).put("connectionId", 0)
                    .put("encodedDataLength", entry.optLong("responseBytes"))
                    .put("securityState", if (url.startsWith("https")) "secure" else "insecure"))
        }
        events += if (entry.has("error")) {
            "Network.loadingFailed" to JSONObject().put("requestId", id).put("timestamp", ended).put("type", "Fetch")
                .put("errorText", entry.optString("error")).put("canceled", false)
        } else {
            "Network.loadingFinished" to JSONObject().put("requestId", id).put("timestamp", ended)
                .put("encodedDataLength", entry.optLong("responseBytes"))
        }
        return events
    }

    fun networkEntry(params: JSONObject): JSONObject = NetworkLog.entry(params.optString("requestId"))
        ?: throw IllegalArgumentException("No resource with given identifier found")

    /**
     * `Page.captureScreenshot` in device pixels, like Chrome; `clip` is in dp, Android's CSS pixel. Draws every app
     * window like a frame capture does, so dialogs and menus show. SurfaceView and TextureView content does not.
     */
    fun screenshot(params: JSONObject): JSONObject {
        val format = params.optString("format", "png")
        val compress = when (format) {
            "png" -> Bitmap.CompressFormat.PNG
            "jpeg" -> Bitmap.CompressFormat.JPEG
            else -> throw IllegalArgumentException("Unsupported image format: $format")
        }
        val bitmap = onMain {
            val decor = FrameRecorder.resumedActivity?.window?.decorView ?: throw IllegalStateException("No window to capture")
            val density = decor.resources.displayMetrics.density
            val clip = params.optJSONObject("clip")
            val scale = clip?.optDouble("scale", 1.0)?.toFloat() ?: 1f
            val left = (clip?.optDouble("x") ?: 0.0).toFloat() * density
            val top = (clip?.optDouble("y") ?: 0.0).toFloat() * density
            val width = clip?.let { it.optDouble("width").toFloat() * density } ?: decor.width.toFloat()
            val height = clip?.let { it.optDouble("height").toFloat() * density } ?: decor.height.toFloat()
            val bitmap = Bitmap.createBitmap((width * scale).toInt().coerceAtLeast(1), (height * scale).toInt().coerceAtLeast(1), Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap).apply { scale(scale, scale); translate(-left, -top) }
            val origin = IntArray(2).also(decor::getLocationOnScreen)
            FrameCapture.windowRoots(decor).forEach { root ->
                val onScreen = IntArray(2).also(root::getLocationOnScreen)
                canvas.save()
                canvas.translate((onScreen[0] - origin[0]).toFloat(), (onScreen[1] - origin[1]).toFloat())
                root.draw(canvas)
                canvas.restore()
            }
            bitmap
        }
        val bytes = ByteArrayOutputStream().also { bitmap.compress(compress, params.optInt("quality", 80), it) }.toByteArray()
        bitmap.recycle()
        return JSONObject().put("data", Base64.getEncoder().encodeToString(bytes))
    }

    /** Like Chrome: no Origin (a tool) or DevTools itself. A web page always sends its own origin. */
    fun originAllowed(origin: String?) = origin == null || origin.startsWith("devtools://") || origin.startsWith("chrome-devtools://")

    fun error(id: Int?, code: Int, message: String) = JSONObject()
        .apply { if (id != null) put("id", id) }
        .put("error", JSONObject().put("code", code).put("message", message))
}
