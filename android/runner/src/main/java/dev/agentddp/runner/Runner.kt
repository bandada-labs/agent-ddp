package dev.agentddp.runner

import android.accessibilityservice.AccessibilityService
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
 * platform's accessibility tree. It runs until `runner.stop`, or [IDLE_MS] without a request:
 * `am instrument dev.agentddp.runner/.Runner`.
 */
class Runner : Instrumentation() {
    // Lazy: sessions run on their own threads and must see the same instance. Without the flag, holding UiAutomation
    // turns TalkBack and every other accessibility service off until the runner ends.
    private val automation: UiAutomation by lazy {
        getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES).also { connected ->
            // Without these the tree leaves out views marked unimportant, resource ids and other windows.
            connected.serviceInfo = connected.serviceInfo.apply {
                flags = flags or AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS or
                    AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS or AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
            }
        }
    }
    private val lock = Any()
    @Volatile private var stopped = false
    @Volatile private var lastRequest = SystemClock.elapsedRealtime()

    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        start()
    }

    override fun onStart() {
        val server = LocalServerSocket(SOCKET_NAME)
        // Only one process may hold UiAutomation: while an idle runner keeps it, the app's own UI tests fail with
        // "UiAutomationService already registered". ddp starts the runner again on its next command.
        thread(name = "ddp-runner-idle", isDaemon = true) {
            while (SystemClock.elapsedRealtime() - lastRequest < IDLE_MS) SystemClock.sleep(IDLE_CHECK_MS)
            stopped = true
            finish(0, Bundle())
        }
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
        lastRequest = SystemClock.elapsedRealtime()
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
                .put("runner", VERSION)
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
            drag(fromX, fromY, toX, toY, params.optLong("ms", 300), params.optLong("hold", 0), params.optLong("still", 0))
            JSONObject()
        }
        "input.type" -> {
            val text = params.optString("text").ifEmpty { throw RunnerError.missing("text") }
            val events = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents(text.toCharArray())
            // Waiting for each key took about 240 ms a character in a Compose field; the input queue keeps their
            // order, so only the last one waits.
            if (events != null) events.forEachIndexed { index, event -> automation.injectInputEvent(event, index == events.lastIndex) }
            else insert(text)
            JSONObject()
        }
        "input.key" -> {
            val key = params.optString("key")
            val global = GLOBAL_ACTIONS[key]
            if (global != null) {
                if (!automation.performGlobalAction(global)) throw RunnerError("the system refused $key", -32000)
            } else {
                val code = KEYS[key] ?: throw RunnerError("key must be one of ${KEYS.keys + GLOBAL_ACTIONS.keys}", -32602)
                press(code)
            }
            JSONObject()
        }
        // Select all, then delete: the keys a person with a keyboard presses (iOS does the same). Setting the text needs
        // the field's node, and just after a tap on a focused Compose field input focus is on a plain View: finding the
        // field then took over 1 s.
        "input.clear" -> {
            automation.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: throw RunnerError("no field has the focus", -32000)
            press(KeyEvent.KEYCODE_A, KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON)
            press(KeyEvent.KEYCODE_DEL)
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

    private fun drag(fromX: Float, fromY: Float, toX: Float, toY: Float, ms: Long, hold: Long, still: Long) {
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
        // Held still before letting go: the velocity the view reads is zero, and a scroll does not fling.
        repeat((still / FRAME_MS).toInt()) {
            SystemClock.sleep(FRAME_MS)
            touch(MotionEvent.ACTION_MOVE, toX, toY, down)
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
    /** Down and up, stamped now: the system drops a key event with no time (KeyEvent(action, code)) as stale. */
    private fun press(code: Int, meta: Int = 0) {
        val now = SystemClock.uptimeMillis()
        listOf(KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP).forEach {
            automation.injectInputEvent(KeyEvent(now, now, it, code, 0, meta), true)
        }
    }

    private fun root(): AccessibilityNodeInfo? {
        val deadline = SystemClock.uptimeMillis() + ROOT_WAIT_MS
        while (true) {
            automation.rootInActiveWindow?.let { return it }
            if (SystemClock.uptimeMillis() >= deadline) return null
            runCatching { automation.waitForIdle(50, 200) }
        }
    }

    private fun find(label: String): AccessibilityNodeInfo? =
        firstNode { label(it) == label || it.viewIdResourceName?.substringAfter(":id/") == label }

    private fun focusedField(): AccessibilityNodeInfo? =
        automation.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.takeIf { it.isEditable }
            ?: firstNode { it.isFocused && it.isEditable }

    private fun firstNode(wanted: (AccessibilityNodeInfo) -> Boolean): AccessibilityNodeInfo? {
        val root = root() ?: return null
        val queue = ArrayDeque(listOf(root))
        while (queue.isNotEmpty()) {
            val node = queue.removeFirst()
            if (wanted(node)) return node
            (0 until node.childCount).mapNotNullTo(queue) { node.getChild(it) }
        }
        return null
    }

    // Text the keyboard map has no keys for (ñ, é, an emoji) goes into the focused field at its cursor, set as an
    // accessibility service sets it (UiAutomator's setText): the app gets one change, not key presses. Just after a
    // tap a Compose field's input focus is on a plain View, so the focused editable node is looked for in the tree.
    private fun insert(text: String) {
        // A field tapped just before takes the focus a moment later.
        val deadline = SystemClock.uptimeMillis() + FOCUS_WAIT_MS
        var field = focusedField()
        while (field == null && SystemClock.uptimeMillis() < deadline) {
            SystemClock.sleep(50)
            field = focusedField()
        }
        field ?: throw RunnerError("no field has the focus", -32000)
        val current = if (field.isShowingHintText) "" else field.text?.toString().orEmpty()
        if (field.isPassword && current.isNotEmpty()) {
            throw RunnerError("a password field's text is hidden, so only keys can add to it: empty it first", -32000)
        }
        val start = field.textSelectionStart.takeIf { it in 0..current.length } ?: current.length
        val end = field.textSelectionEnd.takeIf { it in start..current.length } ?: start
        val replaced = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, current.replaceRange(start, end, text))
        }
        if (!field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, replaced)) {
            throw RunnerError("the field refused the text", -32000)
        }
        val cursor = start + text.length
        field.refresh()
        field.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, cursor)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, cursor)
        })
    }

    // Only what an agent can act on or read: nodes with a label, an id, or that take a tap. A node the app still
    // holds but the user cannot see (scrolled away, panned off by the keyboard) is kept with `shown: false`, so the
    // CLI can tell "out of view" from "gone".
    /** [parent]: where the nearest listed node above this one is in [elements]; the CLI turns it into a key. */
    private fun collect(node: AccessibilityNodeInfo, elements: JSONArray, parent: Int? = null) {
        val label = label(node)
        val id = node.viewIdResourceName?.substringAfter(":id/")
        var aboveChildren = parent
        if (label != null || id != null || node.isClickable) {
            aboveChildren = elements.length()
            elements.put(JSONObject().apply {
                parent?.let { put("parent", it) }
                if (!node.isVisibleToUser) put("shown", false)
                put("type", node.className?.toString()?.substringAfterLast('.'))
                label?.let { put("label", it) }
                id?.let { put("id", it) }
                typed(node)?.let { put("value", it) }
                put("frame", frame(node))
                if (node.isClickable) put("tap", true)
                if (!node.isEnabled) put("enabled", false)
                if (node.isCheckable) put("checked", node.isChecked)
                if (node.isSelected) put("selected", true)
                if (node.isFocused) put("focused", true)
                if (node.isPassword) put("password", true)
                node.error?.toString()?.takeIf { it.isNotBlank() }?.let { put("error", it) }
                // A node that can collapse is expanded, and the other way round.
                val actions = node.actionList
                when {
                    AccessibilityNodeInfo.AccessibilityAction.ACTION_COLLAPSE in actions -> put("expanded", true)
                    AccessibilityNodeInfo.AccessibilityAction.ACTION_EXPAND in actions -> put("expanded", false)
                }
            })
        }
        (0 until node.childCount).forEach { index -> node.getChild(index)?.let { collect(it, elements, aboveChildren) } }
    }

    // A field is named by its hint (an empty field shows it as its text) and holds what was typed as its value.
    private fun label(node: AccessibilityNodeInfo): String? =
        (if (node.isEditable) node.hintText ?: node.contentDescription else node.text ?: node.contentDescription)
            ?.toString()?.takeIf { it.isNotBlank() }

    private fun typed(node: AccessibilityNodeInfo): String? =
        node.text?.toString()?.takeIf { node.isEditable && !node.isShowingHintText && it.isNotEmpty() }

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
        /** Raised when what the runner answers changes; the CLI replaces a runner with another number. */
        const val VERSION = 6
        const val FOCUS_WAIT_MS = 1000L
        const val IDLE_MS = 5 * 60_000L
        const val IDLE_CHECK_MS = 10_000L
        val ALLOWED_UIDS = setOf(0, 2000)
        val KEYS = mapOf(
            "Enter" to KeyEvent.KEYCODE_ENTER,
            "Tab" to KeyEvent.KEYCODE_TAB,
            "Backspace" to KeyEvent.KEYCODE_DEL,
            "Back" to KeyEvent.KEYCODE_BACK,
            "Escape" to KeyEvent.KEYCODE_ESCAPE,
            "Up" to KeyEvent.KEYCODE_DPAD_UP,
            "Down" to KeyEvent.KEYCODE_DPAD_DOWN,
            "Left" to KeyEvent.KEYCODE_DPAD_LEFT,
            "Right" to KeyEvent.KEYCODE_DPAD_RIGHT,
        )
        val GLOBAL_ACTIONS = mapOf(
            "Home" to AccessibilityService.GLOBAL_ACTION_HOME,
            "Recents" to AccessibilityService.GLOBAL_ACTION_RECENTS,
            "Notifications" to AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS,
        )
    }
}
