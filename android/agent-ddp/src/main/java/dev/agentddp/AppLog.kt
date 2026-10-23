package dev.agentddp

import org.json.JSONObject
import kotlin.concurrent.thread

/**
 * The app's own logcat lines for CDP `Log.entryAdded`, read from a `logcat --pid` child process while someone listens.
 * An app may read its own lines without READ_LOGS.
 */
internal class AppLog(private val send: (JSONObject) -> Unit) {
    @Volatile private var process: Process? = null

    /** Sends the last entries, as Chrome replays its buffer on `Log.enable`, then each new one. */
    fun start() {
        val started = runCatching {
            ProcessBuilder("logcat", "-v", "epoch", "-T", REPLAYED.toString(), "--pid=${android.os.Process.myPid()}")
                .redirectErrorStream(true).start()
        }.getOrNull() ?: return
        process = started
        thread(name = "agentddp-log", isDaemon = true) {
            runCatching { started.inputStream.bufferedReader().forEachLine { line -> logEntry(line)?.let(send) } }
        }
    }

    fun stop() {
        process?.destroy()
        process = null
    }

    companion object {
        private const val REPLAYED = 100
        // "1729700000.123  8684  8700 I Tag: message"
        private val LINE = Regex("""^\s*(\d+\.\d+)\s+\d+\s+\d+\s+([VDIWEFA])\s+(.*)$""")

        /** Chrome's `Log.LogEntry`, or null for logcat's own lines ("--------- beginning of main"). */
        fun logEntry(line: String): JSONObject? {
            val (seconds, priority, text) = LINE.find(line)?.destructured ?: return null
            val level = when (priority) {
                "V", "D" -> "verbose"
                "I" -> "info"
                "W" -> "warning"
                else -> "error"
            }
            return JSONObject().put("source", "other").put("level", level).put("text", text.trim())
                .put("timestamp", Math.round(seconds.toDouble() * 1000).toDouble())
        }
    }
}
