package dev.agentddp.runner

import android.accessibilityservice.AccessibilityServiceInfo
import android.app.Instrumentation
import android.app.UiAutomation
import android.content.res.Resources
import android.graphics.Rect
import android.net.LocalServerSocket
import android.os.Bundle
import android.os.SystemClock
import android.view.InputDevice
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import dev.agentddp.HttpRequest
import dev.agentddp.WebSocket
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.InputStream
import java.io.OutputStream
import kotlin.concurrent.thread

/**
 * Serves the control protocol (JSON-RPC over a WebSocket) on the abstract socket `agentddp.runner.control` for an app
 * that does not link AgentDDP. Touches and keys go through UiAutomation, as the system's own input; the tree is the
 * platform's accessibility tree. It runs until `runner.stop`: `am instrument dev.agentddp.runner/.Runner`.
 */
class Runner : Instrumentation() {
    // Lazy: sessions run on their own threads and must see the same instance.
    private val automation: UiAutomation by lazy {
        uiAutomation.also { connected ->
            // Without these the tree leaves out views marked unimportant, resource ids and other windows.
            connected.serviceInfo = connected.serviceInfo.apply {
                flags = flags or AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS or
                    AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS or AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
            }
        }
    }
    private val lock = Any()
    @Volatile private var stopped = false

    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        start()
    }

    override fun onStart() {
        val server = LocalServerSocket(SOCKET_NAME)
        while (!stopped) {
            val socket = runCatching { server.accept() }.getOrNull() ?: continue
            // Abstract sockets are open to every app on the device: only adb (the shell user) and root may talk.
            if (socket.peerCredentials.uid !in ALLOWED_UIDS) {
                socket.close()
                continue
            }
            thread(name = "ddp-runner-session", isDaemon = true) {
                socket.use { serve(BufferedInputStream(it.inputStream), it.outputStream) }
            }
        }
        finish(0, Bundle())
    }

    private fun serve(input: InputStream, output: OutputStream) {
        val request = HttpRequest.read(input) ?: return
        val key = request.headers["sec-websocket-key"] ?: return
        if (request.headers["origin"] != null) return
        output.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Accept: ${WebSocket.acceptKey(key)}\r\n\r\n").toByteArray())
        output.flush()
        while (!stopped) {
            val frame = runCatching { WebSocket.readFrame(input) }.getOrNull() ?: return
            when (frame.opcode) {
                WebSocket.TEXT -> WebSocket.writeText(output, answer(String(frame.payload)).toString())
                WebSocket.PING -> WebSocket.write(output, WebSocket.PONG, frame.payload)
                WebSocket.CLOSE -> return WebSocket.write(output, WebSocket.CLOSE, frame.payload)
            }
        }
    }

    private fun answer(text: String): JSONObject {
        val request = runCatching { JSONObject(text) }.getOrNull()
            ?: return JSONObject().put("error", JSONObject().put("code", -32700).put("message", "not JSON"))
        val reply = JSONObject().put("id", request.opt("id"))
        val started = SystemClock.elapsedRealtimeNanos()
        return try {
            // One action at a time: two agents' touches must not interleave.
            val result = synchronized(lock) { run(request.optString("method"), request.optJSONObject("params") ?: JSONObject()) }
            reply.put("result", result.put("ms", (SystemClock.elapsedRealtimeNanos() - started) / 1e6))
        } catch (error: RunnerError) {
            reply.put("error", JSONObject().put("code", error.code).put("message", error.message))
        } catch (error: Exception) {
            reply.put("error", JSONObject().put("code", -32000).put("message", error.toString()))
        }
    }

    private fun run(method: String, params: JSONObject): JSONObject = when (method) {
        "app.info" -> {
            val metrics = Resources.getSystem().displayMetrics
            JSONObject().put("platform", "android").put("unit", "px").put("scale", metrics.density.toDouble())
                .put("screen", JSONArray().put(metrics.widthPixels).put(metrics.heightPixels))
                .put("app", automation.rootInActiveWindow?.packageName?.toString())
        }
        "input.tap" -> point(params).let { (x, y) -> tap(x, y); JSONObject() }
        "input.doubleTap" -> point(params).let { (x, y) ->
            tap(x, y)
            // Above the platform's minimum gap between the taps of a double tap (40 ms).
            SystemClock.sleep(60)
            tap(x, y)
            JSONObject()
        }
        "input.longPress" -> point(params).let { (x, y) ->
            val down = SystemClock.uptimeMillis()
            touch(MotionEvent.ACTION_DOWN, x, y, down)
            SystemClock.sleep(params.optLong("ms", 600))
            touch(MotionEvent.ACTION_UP, x, y, down)
            JSONObject()
        }
        "input.drag" -> {
            val (fromX, fromY) = point(params.optJSONObject("from") ?: throw RunnerError.missing("from"))
            val (toX, toY) = point(params.optJSONObject("to") ?: throw RunnerError.missing("to"))
            drag(fromX, fromY, toX, toY, params.optLong("ms", 300), params.optLong("hold", 0))
            JSONObject()
        }
        "input.type" -> {
            val text = params.optString("text").ifEmpty { throw RunnerError.missing("text") }
            val events = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents(text.toCharArray())
                ?: throw RunnerError("the keyboard map has no keys for this text", -32602)
            events.forEach { automation.injectInputEvent(it, true) }
            JSONObject()
        }
        "input.key" -> {
            val code = KEYS[params.optString("key")] ?: throw RunnerError("key must be one of ${KEYS.keys}", -32602)
            listOf(KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP).forEach { automation.injectInputEvent(KeyEvent(it, code), true) }
            JSONObject()
        }
        "screen.find" -> {
            val label = params.optString("label").ifEmpty { throw RunnerError.missing("label") }
            val found = find(label)
            JSONObject().put("found", found != null).put("frame", found?.let { frame(it) } ?: JSONObject.NULL)
        }
        "screen.elements" -> JSONObject().put("elements", JSONArray().also { elements ->
            root()?.let { collect(it, elements) }
        })
        "runner.stop" -> {
            stopped = true
            JSONObject()
        }
        else -> throw RunnerError("unknown method $method", -32601)
    }

    // A point is x, y in screen pixels, or `label` (text, content description or resource id) for its center.
    private fun point(params: JSONObject): Pair<Float, Float> {
        if (params.has("label")) {
            val label = params.getString("label")
            val bounds = Rect().also { (find(label) ?: throw RunnerError("no element $label", -32000)).getBoundsInScreen(it) }
            return bounds.exactCenterX() to bounds.exactCenterY()
        }
        if (!params.has("x") || !params.has("y")) throw RunnerError.missing("x, y or label")
        return params.getDouble("x").toFloat() to params.getDouble("y").toFloat()
    }

    private fun tap(x: Float, y: Float) {
        val down = SystemClock.uptimeMillis()
        touch(MotionEvent.ACTION_DOWN, x, y, down)
        touch(MotionEvent.ACTION_UP, x, y, down)
    }

    private fun drag(fromX: Float, fromY: Float, toX: Float, toY: Float, ms: Long, hold: Long) {
        val down = SystemClock.uptimeMillis()
        touch(MotionEvent.ACTION_DOWN, fromX, fromY, down)
        // Held still first, the app sees a long press that picks the item up.
        SystemClock.sleep(hold)
        // One move per display frame, so scroll and fling see the speed of a real finger.
        val steps = (ms / FRAME_MS).coerceAtLeast(1)
        for (step in 1..steps) {
            val fraction = step.toFloat() / steps
            SystemClock.sleep(FRAME_MS)
            touch(MotionEvent.ACTION_MOVE, fromX + (toX - fromX) * fraction, fromY + (toY - fromY) * fraction, down)
        }
        touch(MotionEvent.ACTION_UP, toX, toY, down)
    }

    private fun touch(action: Int, x: Float, y: Float, downTime: Long) {
        val event = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), action, x, y, 0)
        event.source = InputDevice.SOURCE_TOUCHSCREEN
        // Down and up wait until the system dispatched them, so the next call sees their effect start. A move waiting
        // too would stretch a drag far past its time.
        val waitForDispatch = action != MotionEvent.ACTION_MOVE
        if (!automation.injectInputEvent(event, waitForDispatch)) throw RunnerError("the system refused the touch", -32000)
        event.recycle()
    }

    // Right after the runner starts (its flags just set) or a window change, the active window may not be known yet:
    // an empty answer then would read as an empty screen. Wait for it, as long as a window change takes.
    private fun root(): AccessibilityNodeInfo? {
        val deadline = SystemClock.uptimeMillis() + ROOT_WAIT_MS
        while (true) {
            automation.rootInActiveWindow?.let { return it }
            if (SystemClock.uptimeMillis() >= deadline) return null
            runCatching { automation.waitForIdle(50, 200) }
        }
    }

    private fun find(label: String): AccessibilityNodeInfo? {
        val root = root() ?: return null
        val queue = ArrayDeque(listOf(root))
        while (queue.isNotEmpty()) {
            val node = queue.removeFirst()
            if (label(node) == label || node.viewIdResourceName?.substringAfter(":id/") == label) return node
            (0 until node.childCount).mapNotNullTo(queue) { node.getChild(it) }
        }
        return null
    }

    // Only what an agent can act on or read: nodes with a label, an id, or that take a tap.
    private fun collect(node: AccessibilityNodeInfo, elements: JSONArray) {
        val label = label(node)
        val id = node.viewIdResourceName?.substringAfter(":id/")
        if (node.isVisibleToUser && (label != null || id != null || node.isClickable)) {
            elements.put(JSONObject().apply {
                put("type", node.className?.toString()?.substringAfterLast('.'))
                label?.let { put("label", it) }
                id?.let { put("id", it) }
                put("frame", frame(node))
                if (node.isClickable) put("tap", true)
                if (!node.isEnabled) put("enabled", false)
            })
        }
        (0 until node.childCount).forEach { index -> node.getChild(index)?.let { collect(it, elements) } }
    }

    private fun label(node: AccessibilityNodeInfo): String? =
        (node.text ?: node.contentDescription)?.toString()?.takeIf { it.isNotBlank() }

    private fun frame(node: AccessibilityNodeInfo): JSONArray {
        val bounds = Rect().also { node.getBoundsInScreen(it) }
        return JSONArray().put(bounds.left).put(bounds.top).put(bounds.width()).put(bounds.height())
    }

    private class RunnerError(message: String, val code: Int) : Exception(message) {
        companion object {
            fun missing(name: String) = RunnerError("missing $name", -32602)
        }
    }

    private companion object {
        const val SOCKET_NAME = "agentddp.runner.control"
        const val FRAME_MS = 16L
        const val ROOT_WAIT_MS = 2000L
        val ALLOWED_UIDS = setOf(0, 2000)
        val KEYS = mapOf(
            "Enter" to KeyEvent.KEYCODE_ENTER,
            "Tab" to KeyEvent.KEYCODE_TAB,
            "Backspace" to KeyEvent.KEYCODE_DEL,
            "Back" to KeyEvent.KEYCODE_BACK,
        )
    }
}
