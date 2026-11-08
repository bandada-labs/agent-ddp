package dev.agentddp

import android.app.Application
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.PointF
import android.graphics.RectF
import android.net.LocalServerSocket
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.util.Base64
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * The control endpoint (ADR 0008): JSON-RPC over a WebSocket on the abstract socket `agentddp.<package>.control`, with
 * method names taken from the app, not from a browser. Only adb (shell) and root may connect.
 */
internal object ControlServer {
    // Abstract sockets are open to every app on the device: only adb (the shell user) and root may talk.
    private const val SHELL_UID = 2000
    private const val ROOT_UID = 0
    private val ALLOWED_UIDS = setOf(SHELL_UID, ROOT_UID)

    fun start(application: Application) {
        Preferences.application = application
        val app = ControlApp(
            packageName = application.packageName,
            name = application.applicationInfo.loadLabel(application.packageManager).toString(),
        )
        thread(name = "agentddp-control", isDaemon = true) {
            val server = runCatching { LocalServerSocket("agentddp.${app.packageName}.control") }.getOrNull() ?: return@thread
            while (true) {
                val socket = runCatching { server.accept() }.getOrNull() ?: continue
                if (socket.peerCredentials.uid !in ALLOWED_UIDS) {
                    socket.close()
                    continue
                }
                thread(name = "agentddp-control-session", isDaemon = true) {
                    // Nothing the library does may end the app: an error here only ends this connection.
                    socket.use {
                        runCatching { ControlConnection(BufferedInputStream(it.inputStream), it.outputStream, app).run() }
                            .onFailure { error -> Log.e("agentddp", "control session ended", error) }
                    }
                }
            }
        }
    }
}

internal class ControlApp(val packageName: String, val name: String)

/** One connection: a WebSocket upgrade, then messages until the client closes. */
internal class ControlConnection(
    private val input: InputStream,
    private val output: OutputStream,
    private val app: ControlApp,
) {
    fun run() {
        val request = HttpRequest.read(input) ?: return
        if (!request.headers["upgrade"].equals("websocket", ignoreCase = true)) return respond("426 Upgrade Required", "Use a WebSocket")
        val key = request.headers["sec-websocket-key"] ?: return respond("400 Bad Request", "No Sec-WebSocket-Key")
        // A web page always sends its origin; tools do not.
        if (request.headers["origin"] != null) return respond("403 Forbidden", "Web pages may not connect")
        output.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Accept: ${WebSocket.acceptKey(key)}\r\n\r\n").toByteArray())
        output.flush()
        val session = ControlSession(app) { message -> synchronized(output) { WebSocket.writeText(output, message) } }
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

    private fun respond(status: String, body: String) {
        val bytes = body.toByteArray()
        output.write("HTTP/1.1 $status\r\nContent-Type: text/plain\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray())
        output.write(bytes)
        output.flush()
    }
}

/** Answers the messages of one client. Every handler calls a native module and only converts JSON. */
internal class ControlSession(private val app: ControlApp, private val send: (String) -> Unit) {
    private var networkWatch: java.util.UUID? = null
    private var appLog: AppLog? = null
    private var stopEvents: (() -> Unit)? = null
    // Main thread only.
    private var screenStream: FrameStream? = null

    private val methods: Map<String, (JSONObject) -> Any> = mapOf(
        "app.info" to { _ -> Control.info(app) },
        "app.commands" to { _ -> JSONObject().put("commands", JSONArray(AgentDdpCommands.names())) },
        "app.run" to AgentDdpCommands::run,
        // Which of these names the app was started with, as the platform gave them: the running activity's intent extras
        // (ddp launch --env). Never the values.
        "app.launch" to { params ->
            val names = params.optJSONArray("names")?.let { array -> List(array.length()) { array.getString(it) } }.orEmpty()
            val extras = onMain { FrameRecorder.resumedActivity?.intent?.extras?.keySet().orEmpty() }
            JSONObject().put("received", JSONArray(names.filter { it in extras }))
        },
        "perf.metrics" to Perf::measure,
        "media.list" to { _ -> Media.list() },
        // Compose animates on its own frame clock and the animator registry is hidden: a list would miss most motion.
        "screen.animations" to { _ ->
            throw IllegalArgumentException("Android has no public list of running animations: ddp record shows what moves")
        },
        "screen.highlight" to Highlight::show,
        "storage.files" to Storage::files,
        "storage.read" to Storage::read,
        "storage.remove" to Storage::remove,
        "storage.query" to Storage::query,
        "storage.keys" to { _ -> Storage.keys() },
        "state.list" to { _ -> AgentDdpState.list() },
        "state.get" to AgentDdpState::get,
        "state.set" to AgentDdpState::set,
        "screen.elements" to { _ -> JSONObject().put("elements", onMain { FrameCapture.frame(0, 0.0).getJSONArray("elements") }) },
        "screen.capture" to { params -> Control.capture(params) },
        "app.hooks" to { _ -> JSONObject().put("hooks", onMain { Hooks.report() }) },
        "screen.pixels" to { params ->
            val rects = params.optJSONArray("rects") ?: JSONArray()
            Pixels.regions((0 until rects.length()).mapNotNull { i ->
                rects.optJSONArray(i)?.takeIf { it.length() == 4 }?.let { r ->
                    RectF(r.getDouble(0).toFloat(), r.getDouble(1).toFloat(), (r.getDouble(0) + r.getDouble(2)).toFloat(), (r.getDouble(1) + r.getDouble(3)).toFloat())
                }
            })
        },
        "screen.hit" to { params ->
            val touch = params.optDouble("touch", 0.0).toFloat()
            onMain {
                // Frames are in the activity window's pixels; a screenshot is the whole display's.
                val origin = FrameRecorder.resumedActivity?.window?.decorView?.let(FrameCapture::windowOrigin) ?: IntArray(2)
                JSONObject().put("elements", Hit.check(params.optString("id").ifEmpty { null }, touch))
                    .put("origin", JSONArray(origin.toList()))
            }
        },
        "screen.frame" to { _ -> onMain { FrameCapture.frame(0, 0.0) } },
        "screen.watch" to { params -> watchScreen(params.optInt("frames", 0)); JSONObject() },
        "screen.unwatch" to { _ -> onMain { screenStream?.stop() }; JSONObject() },
        "compose.recompositions.start" to Recompositions::start,
        "compose.recompositions.stop" to { _ -> Recompositions.stop() },
        "screen.source" to { params -> onMain { SourceLocations.of(params.getString("id")) } },
        "screen.value" to { params -> Control.answer(onMain { LiveEval.evaluate(params.getString("id"), params.getString("path")) }) },
        // Compose draws what the app's state says, so a value set from outside would not be the app's; ddp does not.
        "screen.setValue" to { _ ->
            throw IllegalArgumentException("Compose takes its values from the app's state: change that state (ddp state set)")
        },
        "time.pause" to time("pause"),
        "time.resume" to time("resume"),
        "time.step" to time("step", "frames"),
        "time.rate" to time("rate", "rate"),
        "time.seek" to time("seek", "ms"),
        "time.status" to time("status"),
        "history.files" to { _ -> JSONObject().put("files", JSONArray(History.files())) },
        "history.read" to { params -> JSONObject().put("data", Base64.getEncoder().encodeToString(History.read(params.getString("file")))) },
        "input.touch" to { params ->
            val point = PointF(params.optDouble("x", 0.0).toFloat(), params.optDouble("y", 0.0).toFloat())
            when (params.optString("phase")) {
                "down", "move" -> onMain { Input.touch(point) }
                "up" -> onMain { Input.touch(null) }
                else -> throw IllegalArgumentException("phase is down, move or up")
            }
            JSONObject()
        },
        "input.type" to { params -> onMain { Input.type(params.getString("text")) }; JSONObject() },
        "input.key" to { params -> onMain { Input.press(params.getString("key")) }; JSONObject() },
        "net.list" to { _ -> JSONObject().put("requests", JSONArray(NetworkLog.summaries())) },
        "net.get" to { params -> NetworkLog.entry(params.getString("id")) ?: throw IllegalArgumentException("No request ${params.getString("id")}") },
        "net.watch" to { _ ->
            stopNetwork()
            // Runs on the app's network thread: a closed socket must not throw into the app's request.
            networkWatch = NetworkLog.watch { summary -> runCatching { event("net.request", summary) }.isSuccess }
            JSONObject()
        },
        "net.unwatch" to { _ -> stopNetwork(); JSONObject() },
        "net.intercept" to { params ->
            // Runs on the app's network thread, as above.
            Fetch.intercept(this, params) { paused -> runCatching { event("net.paused", paused) } }
            JSONObject()
        },
        "net.release" to { _ -> Fetch.release(this); JSONObject() },
        // The default java.net cookie store, when the app set one (OkHttp's JavaNetCookieJar uses it): names and
        // domains, never the values. A client with its own CookieJar keeps its cookies to itself.
        "net.cookies" to { _ ->
            val store = (java.net.CookieHandler.getDefault() as? java.net.CookieManager)?.cookieStore
            JSONObject().put("cookies", JSONArray(store?.cookies.orEmpty().map { cookie ->
                JSONObject().put("name", cookie.name).put("domain", cookie.domain ?: JSONObject.NULL).put("path", cookie.path ?: JSONObject.NULL)
                    .put("secure", cookie.secure).put("expires", if (cookie.maxAge < 0) JSONObject.NULL else cookie.maxAge)
            })).apply { if (store == null) put("unread", "the app set no java.net CookieManager; its HTTP client keeps its own cookies") }
        },
        "net.clearCookies" to { _ ->
            val store = (java.net.CookieHandler.getDefault() as? java.net.CookieManager)?.cookieStore
                ?: throw IllegalStateException("The app set no java.net CookieManager; its HTTP client keeps its own cookies")
            val count = store.cookies.size
            store.removeAll()
            JSONObject().put("removed", count)
        },
        "net.continue" to Fetch::proceed,
        "net.respond" to Fetch::respond,
        "net.fail" to Fetch::fail,
        "net.send" to { params ->
            // AppClient loads OkHttp classes, which an app without OkHttp does not have.
            try { AppClient.send(params) } catch (missing: NoClassDefFoundError) { throw IllegalStateException("net.send needs OkHttp in the app") }
        },
        "log.watch" to { _ ->
            appLog?.stop()
            appLog = AppLog { entry -> runCatching { event("log.entry", entry) } }.also { it.start() }
            JSONObject()
        },
        "log.unwatch" to { _ -> appLog?.stop(); appLog = null; JSONObject() },
        "events.list" to { _ -> AgentDdpEvents.list() },
        "events.watch" to { _ ->
            stopEvents?.invoke()
            stopEvents = AgentDdpEvents.watch { entry -> runCatching { event("events.entry", entry) } }
            JSONObject()
        },
        "events.unwatch" to { _ -> stopEvents?.invoke(); stopEvents = null; JSONObject() },
        "prefs.files" to { _ -> JSONObject().put("files", JSONArray(Preferences.files())) },
        "prefs.read" to { params -> JSONObject().put("values", JSONObject(Preferences.read(params.file()).toMap())) },
        "prefs.write" to { params -> Preferences.write(params.file(), params.getString("key"), params.getString("value")); JSONObject() },
        "prefs.remove" to { params -> Preferences.remove(params.file(), params.getString("key")); JSONObject() },
        "prefs.clear" to { params -> Preferences.clear(params.file()); JSONObject() },
    )

    /** The connection closed: stop feeding it events and let paused requests go on. */
    fun close() {
        runCatching { onMain { screenStream?.stop() } }
        stopNetwork()
        Fetch.release(this)
        appLog?.stop()
        stopEvents?.invoke()
    }

    private fun stopNetwork() {
        networkWatch?.let(NetworkLog::stopWatching)
        networkWatch = null
    }

    // optString reads a JSON null as the text "null".
    private fun JSONObject.file(): String? = (opt("file") as? String)?.ifEmpty { null }

    private fun time(action: String, key: String? = null): (JSONObject) -> Any = { params ->
        Control.answer(onMain { TimeControl.handle(action, key?.let { params.optDouble(it) }?.takeUnless(Double::isNaN)) })
    }

    /**
     * `screen.stream` events: the header, frame and end lines of a recording. They go out from a thread of their own,
     * so the main thread never waits on a slow client; a client that falls [FRAME_BACKLOG] frames behind ends the stream.
     */
    private fun watchScreen(frameLimit: Int) {
        val outbox = ArrayBlockingQueue<JSONObject>(FRAME_BACKLOG)
        val finished = AtomicBoolean(false)
        onMain {
            FrameRecorder.resumedActivity ?: throw IllegalStateException("No activity in the foreground")
            screenStream?.stop()
            screenStream = FrameStream(frameLimit, send = outbox::offer, finished = { finished.set(true) }).also { it.start() }
        }
        thread(name = "agentddp-screen", isDaemon = true) {
            while (!finished.get() || outbox.isNotEmpty()) {
                val line = outbox.poll(200, TimeUnit.MILLISECONDS) ?: continue
                if (runCatching { event("screen.stream", line) }.isFailure) {
                    runCatching { onMain { screenStream?.stop() } }
                    return@thread
                }
            }
        }
    }

    private fun event(method: String, params: JSONObject) = send(JSONObject().put("method", method).put("params", params).toString())

    fun receive(text: String) {
        val message = runCatching { JSONObject(text) }.getOrNull() ?: return send(Control.error(null, -32700, "Not JSON").toString())
        val id = message.opt("id") as? Int ?: return send(Control.error(null, -32600, "No integer id").toString())
        val method = message.optString("method")
        val handler = methods[method] ?: return send(Control.error(id, -32601, "Unknown method '$method'").toString())
        val params = message.optJSONObject("params") ?: JSONObject()
        if (method !in WAITING) return answer(id, handler, params)
        // A closed socket must not end the thread with an exception nobody catches.
        thread(name = "agentddp-$method", isDaemon = true) { runCatching { answer(id, handler, params) } }
    }

    private fun answer(id: Int, handler: (JSONObject) -> Any, params: JSONObject) {
        val answer = runCatching { JSONObject().put("id", id).put("result", handler(params)) }
            .getOrElse { Control.error(id, -32000, it.message ?: it.toString()) }
        send(answer.toString())
    }

    private companion object {
        /** These wait on the app's code or the network, off the reading thread, so a request they send can still be paused and continued. */
        val WAITING = setOf("app.run", "net.send", "perf.metrics")
        const val FRAME_BACKLOG = 64
    }
}

internal object Control {
    /** An answer line (`type` value or error) as a result: its `type` goes, and an error line throws. */
    fun answer(line: JSONObject): JSONObject {
        if (line.optString("type") == "error") throw IllegalArgumentException(line.optString("error"))
        return line.apply { remove("type") }
    }

    fun info(app: ControlApp): JSONObject {
        val density = onMain { FrameRecorder.resumedActivity?.resources?.displayMetrics?.density } ?: 1f
        // The activity window's size, the space frames are in.
        val window = onMain { FrameRecorder.resumedActivity?.window?.decorView?.let { listOf(it.width, it.height) } }
        return JSONObject().put("protocol", FrameCapture.PROTOCOL).put("platform", "android").put("app", app.packageName).put("name", app.name)
            .put("unit", "px").put("scale", density.toDouble()).apply { window?.let { put("screen", JSONArray(it)) } }
    }

    /**
     * The app's windows as one image in pixels, dialogs and menus included, like a frame capture. `clip` is
     * `[x, y, width, height]` in pixels. SurfaceView and TextureView content is not drawn.
     */
    fun capture(params: JSONObject): JSONObject {
        val format = params.optString("format", "png")
        val compress = when (format) {
            "png" -> Bitmap.CompressFormat.PNG
            "jpeg" -> Bitmap.CompressFormat.JPEG
            else -> throw IllegalArgumentException("format is png or jpeg")
        }
        val clip = params.optJSONArray("clip")
        val bitmap = onMain {
            val decor = FrameRecorder.resumedActivity?.window?.decorView ?: throw IllegalStateException("No window to capture")
            val (left, top, width, height) = clip?.let { array -> (0 until 4).map { array.getDouble(it).toFloat() } }
                ?: listOf(0f, 0f, decor.width.toFloat(), decor.height.toFloat())
            val bitmap = Bitmap.createBitmap(width.toInt().coerceAtLeast(1), height.toInt().coerceAtLeast(1), Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap).apply { translate(-left, -top) }
            // A root draws its views where they are laid out; on screen they sit where getLocationOnScreen says, pan included.
            val origin = FrameCapture.windowOrigin(decor)
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

    fun error(id: Int?, code: Int, message: String) = JSONObject()
        .apply { if (id != null) put("id", id) }
        .put("error", JSONObject().put("code", code).put("message", message))
}
