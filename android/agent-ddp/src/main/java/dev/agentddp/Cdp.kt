package dev.agentddp

import android.app.Application
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
            request.path == "/json/version" -> json(Cdp.version(target).put("webSocketDebuggerUrl", "ws://$host/devtools/page/${target.id}"))
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
        val session = CdpSession(target) { message -> synchronized(output) { WebSocket.writeText(output, message) } }
        while (true) {
            val frame = runCatching { WebSocket.readFrame(input) }.getOrNull() ?: return
            when (frame.opcode) {
                WebSocket.TEXT -> session.receive(String(frame.payload))
                WebSocket.PING -> synchronized(output) { WebSocket.write(output, WebSocket.PONG, frame.payload) }
                WebSocket.CLOSE -> return synchronized(output) { WebSocket.write(output, WebSocket.CLOSE, frame.payload) }
            }
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

/** Answers CDP messages for one client. The same error codes and messages as Chrome, so clients behave the same. */
internal class CdpSession(private val target: CdpTarget, private val send: (String) -> Unit) {
    private val methods: Map<String, (JSONObject) -> JSONObject> = mapOf(
        "Browser.getVersion" to { _ -> Cdp.browserVersion(target) },
        "Target.getTargets" to { _ -> JSONObject().put("targetInfos", JSONArray().put(target.info())) },
        "Target.getTargetInfo" to { _ -> JSONObject().put("targetInfo", target.info()) },
        "Target.setDiscoverTargets" to { _ -> JSONObject() },
        "Schema.getDomains" to { _ -> JSONObject().put("domains", JSONArray(Cdp.DOMAINS.map { JSONObject().put("name", it).put("version", "1.3") })) },
    )

    fun receive(text: String) {
        val message = runCatching { JSONObject(text) }.getOrNull()
            ?: return send(Cdp.error(null, -32700, "Message must be a valid JSON").toString())
        val id = message.opt("id") as? Int
            ?: return send(Cdp.error(null, -32600, "Message must have integer 'id' property").toString())
        val method = message.optString("method")
        val handler = methods[method] ?: return send(Cdp.error(id, -32601, "'$method' wasn't found").toString())
        val answer = runCatching { JSONObject().put("id", id).put("result", handler(message.optJSONObject("params") ?: JSONObject())) }
            .getOrElse { Cdp.error(id, -32000, it.message ?: it.toString()) }
        message.opt("sessionId")?.let { answer.put("sessionId", it) }
        send(answer.toString())
    }
}

internal object Cdp {
    const val PROTOCOL_VERSION = "1.3"
    val DOMAINS = listOf("Browser", "Schema", "Target")

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

    /** Like Chrome: no Origin (a tool) or DevTools itself. A web page always sends its own origin. */
    fun originAllowed(origin: String?) = origin == null || origin.startsWith("devtools://") || origin.startsWith("chrome-devtools://")

    fun error(id: Int?, code: Int, message: String) = JSONObject()
        .apply { if (id != null) put("id", id) }
        .put("error", JSONObject().put("code", code).put("message", message))
}
