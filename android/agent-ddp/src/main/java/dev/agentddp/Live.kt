package dev.agentddp

import android.net.LocalServerSocket
import android.net.LocalSocket
import android.os.Handler
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.unit.Dp
import org.json.JSONObject
import java.util.concurrent.FutureTask
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * The live endpoint `ddp connect` attaches to through `adb forward tcp:0 localabstract:agentddp.<package>`.
 * An abstract socket needs no INTERNET permission and is not reachable from the network.
 */
internal object LiveServer {
    // Abstract sockets are open to every app on the device: only adb (the shell user) and root may talk.
    private const val SHELL_UID = 2000
    private const val ROOT_UID = 0

    fun start(packageName: String) {
        thread(name = "agentddp-live", isDaemon = true) {
            val server = runCatching { LocalServerSocket("agentddp.$packageName") }.getOrNull() ?: return@thread
            while (true) {
                val socket = runCatching { server.accept() }.getOrNull() ?: continue
                if (socket.peerCredentials.uid !in setOf(SHELL_UID, ROOT_UID)) {
                    socket.close()
                    continue
                }
                thread(name = "agentddp-session", isDaemon = true) { LiveSession(socket).run() }
            }
        }
    }
}

/**
 * One client: a token line (ignored here, the uid check is stronger), one command, then JSON lines back
 * that end with {"type":"end"}. Streams run until the client closes. Captures run on the main thread;
 * this thread does the socket writes so a slow client never blocks the UI.
 */
private class LiveSession(private val socket: LocalSocket) {
    private val main = Handler(Looper.getMainLooper())
    private val outbox = LinkedBlockingQueue<String>()
    @Volatile private var open = true

    fun run() = socket.use {
        val reader = socket.inputStream.bufferedReader()
        reader.readLine() ?: return
        val command = runCatching { JSONObject(reader.readLine() ?: return) }.getOrElse { return fail("not JSON") }
        when (command.optString("cmd")) {
            "snapshot" -> {
                onMain { listOfNotNull(FrameCapture.header(1), FrameCapture.frame(0, 0.0)) }.forEach(::write)
                end()
            }
            "stream" -> stream(command.optInt("frames", 0))
            "eval" -> {
                write(onMain { LiveEval.evaluate(command.optString("id"), command.optString("path")) })
                end()
            }
            "time" -> {
                val value = if (command.has("value")) command.getDouble("value") else null
                write(onMain { TimeControl.handle(command.optString("action"), value) })
                end()
            }
            "net" -> {
                NetworkLog.summaries().forEach(::write)
                end()
            }
            "net-get" -> {
                write(NetworkLog.entry(command.optString("id"))
                    ?: error("no request with that id; see `ddp net list`"))
                end()
            }
            "net-watch" -> {
                val key = NetworkLog.watch { summary -> open.also { if (it) outbox.put(summary.toString()) } }
                try { drainUntilClosed() } finally { NetworkLog.stopWatching(key) }
            }
            else -> fail("unknown command ${command.optString("cmd")}")
        }
    }

    private fun stream(frames: Int) {
        val done = "\u0000done"
        var frameStream: FrameStream? = null
        main.post {
            frameStream = FrameStream(frames, send = { line -> open.also { if (it) outbox.put(line.toString()) } },
                finished = { outbox.put(done) }).also { it.start() }
        }
        try {
            while (open) {
                val line = outbox.take()
                if (line == done) break
                writeRaw(line)
            }
        } finally {
            open = false
            main.post { frameStream?.stop() }
        }
    }

    private fun drainUntilClosed() {
        while (open) outbox.poll(1, TimeUnit.SECONDS)?.let(::writeRaw) ?: writeRaw("")
    }

    private fun <T> onMain(block: () -> T): T = FutureTask(block).also(main::post).get(5, TimeUnit.SECONDS)

    private fun write(line: JSONObject) = writeRaw(line.toString())

    /** An empty line is a keep-alive: it finds out a closed client while nothing else is sent. */
    private fun writeRaw(line: String) {
        if (!open) return
        runCatching {
            socket.outputStream.write((line + "\n").toByteArray())
            socket.outputStream.flush()
        }.onFailure { open = false }
    }

    private fun end() = write(JSONObject().put("type", "end"))

    private fun error(message: String) = JSONObject().put("type", "error").put("error", message)

    private fun fail(message: String) {
        write(error(message))
        end()
    }
}

/**
 * Read-only property reads for `ddp eval <element> <path>`: `<modifier>.<property>` from the Compose
 * modifier inspector (border.width, graphicsLayer.alpha, background.color) or `semantics.<Key>`
 * (semantics.Selected, semantics.StateDescription) on the semantics node with that test tag or text.
 */
private object LiveEval {
    fun evaluate(name: String, path: String): JSONObject {
        val (scope, property) = path.split(".", limit = 2).takeIf { it.size == 2 }
            ?: return error("path is <modifier>.<property> or semantics.<Key>, e.g. border.width")
        val root = FrameRecorder.resumedActivity?.window?.decorView ?: return error("no activity in the foreground")
        val node = composeRoots(root).firstNotNullOfOrNull { find(it.semanticsOwner.unmergedRootSemanticsNode, name) }
            ?: return error("no Compose element '$name' (tag it with Modifier.testTag)")
        val value = if (scope == "semantics") {
            node.config.firstOrNull { it.key.name == property }?.value
                ?: return error("no semantics '$property'; has: ${node.config.joinToString { it.key.name }}")
        } else {
            val modifiers = node.layoutInfo.getModifierInfo().mapNotNull { inspect(it.modifier) }
            val properties = modifiers.firstOrNull { it.first == scope }?.second
                ?: return error("no modifier '$scope'; has: ${modifiers.joinToString { it.first }}")
            if (property !in properties) return error("'$scope' has: ${properties.keys.joinToString()}")
            properties[property]
        }
        return JSONObject().put("type", "value").put("path", path).put("value", jsonValue(value))
    }

    private fun composeRoots(view: View): List<ViewRootForTest> = when (view) {
        is ViewRootForTest -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { composeRoots(view.getChildAt(it)) }
        else -> emptyList()
    }

    private fun find(node: SemanticsNode, name: String): SemanticsNode? {
        val tag = node.config.getOrNull(SemanticsProperties.TestTag)
        val text = node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }
        if (tag == name || text == name) return node
        return node.children.firstNotNullOfOrNull { find(it, name) }
    }

    private fun jsonValue(value: Any?): Any = when (value) {
        null -> JSONObject.NULL
        is Number, is Boolean, is String -> value
        is Dp -> value.value
        is Color -> hex(value.toArgb()) ?: "transparent"
        else -> colorOf(value) ?: value.toString()
    }

    private fun error(message: String) = JSONObject().put("type", "error").put("error", message)
}
