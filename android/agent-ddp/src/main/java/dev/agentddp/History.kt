package dev.agentddp

import android.app.Activity
import android.app.Application
import android.os.SystemClock
import android.view.Choreographer
import android.view.MotionEvent
import android.view.ViewConfiguration
import android.view.ViewTreeObserver
import android.view.Window
import org.json.JSONObject
import java.io.File
import java.io.Writer
import java.util.WeakHashMap
import java.util.concurrent.Executors
import java.util.zip.GZIPOutputStream
import kotlin.math.hypot

/**
 * The flight recorder behind `ddp history`: every frame that differs from the one before, plus taps and screen
 * changes, for the last [KEEP_MS]. A still screen costs nothing, because a capture runs only after a window drew.
 * Files are recordings of their own (header, frames, events), one per [SEGMENT_MS], gzipped when closed, so the
 * history of a run that crashed is still there on the next one.
 */
internal object History {
    private const val SEGMENT_MS = 60_000L
    private const val KEEP_MS = 15 * 60_000L
    /** While only busy elements change, capture at most this often. Anything else changing goes back to every frame. */
    private const val BUSY_INTERVAL_MS = 250L

    private lateinit var directory: File
    private val writer = Executors.newSingleThreadExecutor { Thread(it, "agentddp-history").apply { isDaemon = true } }

    // Main thread only.
    private val watchedWindows = WeakHashMap<Window, Unit>()
    private var captureScheduled = false
    // The quiet gap must be longer than BUSY_INTERVAL_MS: while throttled, a busy element is only seen that often.
    private val tracker = ChangeTracker(busyAfterMs = 2_000, quietMs = 2 * BUSY_INTERVAL_MS)
    private var lastCaptureAt = 0L
    private val capture = Choreographer.FrameCallback { captureNow() }

    // Writer thread only.
    private var segment: Segment? = null

    private var scale = 1.0

    /** For `history.files` and `history.read`: the files as they are on disk, the open one included. */
    fun files(): List<String> = directory.list().orEmpty().filter { it.endsWith(".jsonl") || it.endsWith(".jsonl.gz") }.sorted()

    fun read(name: String): ByteArray {
        if (name !in files()) throw IllegalArgumentException("No history file '$name'; history.files lists them")
        return File(directory, name).readBytes()
    }

    fun start(application: Application) {
        scale = application.resources.displayMetrics.density.toDouble()
        directory = File(application.filesDir, "agent-ddp/history").apply { mkdirs() }
        writer.execute { prune(System.currentTimeMillis()) }
        event("launch")
    }

    fun onResumed(activity: Activity) {
        event("screen", JSONObject().put("name", activity.javaClass.simpleName))
        val window = activity.window ?: return
        if (window in watchedWindows) return
        watchedWindows[window] = Unit
        window.callback = TouchWatcher(window.callback, ViewConfiguration.get(activity).scaledTouchSlop.toFloat())
        // ponytail: only the activity window wakes the recorder; a dialog or popup that animates on its own
        // (the activity under it still) is caught at its next activity draw. Add their roots' listeners if that bites.
        window.decorView.viewTreeObserver.addOnDrawListener(ViewTreeObserver.OnDrawListener(::onDraw))
    }

    /** A frame a recording already captured and sent: kept here too, so recording does not pay twice. */
    fun offer(frame: JSONObject) {
        lastCaptureAt = SystemClock.uptimeMillis()
        keep(frame)
    }

    /** [wall]: when it happened; a touch is stamped when it went down, before what it caused. */
    fun event(kind: String, fields: JSONObject = JSONObject(), wall: Long = System.currentTimeMillis()) {
        writer.execute { segmentAt(wall).write(fields.put("type", "event").put("kind", kind), wall) }
    }

    private fun onDraw() {
        if (captureScheduled || FrameStream.active > 0) return
        captureScheduled = true
        val wait = if (tracker.onlyBusyChanging) BUSY_INTERVAL_MS - (SystemClock.uptimeMillis() - lastCaptureAt) else 0
        Choreographer.getInstance().postFrameCallbackDelayed(capture, wait.coerceAtLeast(0))
    }

    private fun captureNow() {
        captureScheduled = false
        if (FrameRecorder.resumedActivity == null || FrameStream.active > 0) return
        lastCaptureAt = SystemClock.uptimeMillis()
        val started = SystemClock.elapsedRealtimeNanos()
        val frame = FrameCapture.frame(0, 0.0)
        keep(frame, (SystemClock.elapsedRealtimeNanos() - started) / 1e6)
    }

    private fun keep(frame: JSONObject, captureMs: Double? = null) {
        if (tracker.changed(signatures(frame), SystemClock.uptimeMillis()).isEmpty()) return
        val wall = System.currentTimeMillis()
        writer.execute { segmentAt(wall).writeFrame(frame, wall, captureMs) }
    }

    /** Per element (named like the CLI names them), its JSON and the nodes it drew; nodes nobody owns under "". */
    private fun signatures(frame: JSONObject): Map<String, String> {
        val elements = frame.getJSONArray("elements")
        val seen = mutableMapOf<String, Int>()
        val names = List(elements.length()) { index ->
            val element = elements.getJSONObject(index)
            val name = element.optString("id").ifEmpty { element.optString("label") }
            val count = seen.merge(name, 1, Int::plus)!!
            if (count == 1) name else "$name [$count]"
        }
        val parts = List(names.size + 1) { index -> StringBuilder(elements.optJSONObject(index)?.toString() ?: "") }
        val nodes = frame.getJSONArray("nodes")
        for (index in 0 until nodes.length()) {
            val node = nodes.getJSONObject(index)
            parts[node.optInt("own", -1).takeIf { it in names.indices } ?: names.size].append(node.toString())
        }
        return (names + "").zip(parts.map(StringBuilder::toString)).toMap()
    }

    private fun segmentAt(wall: Long): Segment {
        val current = segment
        if (current != null && wall - current.startedAt < SEGMENT_MS) return current
        current?.close()
        prune(wall)
        return Segment(directory, wall, scale).also { segment = it }
    }

    private fun prune(now: Long) {
        directory.listFiles()?.filter { (it.name.substringBefore('.').toLongOrNull() ?: 0) < now - KEEP_MS - SEGMENT_MS }
            ?.forEach { it.delete() }
    }
}

/** One file of history: `<startedAt>.jsonl` while open, `<startedAt>.jsonl.gz` once closed. */
private class Segment(private val directory: File, val startedAt: Long, scale: Double) {
    private val file = File(directory, "$startedAt.jsonl")
    private val out: Writer = file.bufferedWriter()
    private var frames = 0
    private var captures = 0
    private var totalCaptureMs = 0.0
    private var slowestCaptureMs = 0.0

    init {
        line(JSONObject().put("type", "header").put("protocol", FrameCapture.PROTOCOL).put("platform", "android").put("unit", "px").put("scale", scale)
            .put("frames", 0).put("startedAt", startedAt).put("history", true))
    }

    fun writeFrame(frame: JSONObject, wall: Long, captureMs: Double?) {
        captureMs?.let {
            captures++
            totalCaptureMs += it
            slowestCaptureMs = maxOf(slowestCaptureMs, it)
        }
        // A streamed frame is still waiting to be sent: number and stamp a copy, not the client's frame.
        val copy = JSONObject(frame, frame.keys().asSequence().toList().toTypedArray())
        write(copy.put("i", frames++), wall)
    }

    fun write(line: JSONObject, wall: Long) = line(line.put("t", (wall - startedAt).toDouble()))

    fun close() {
        line(JSONObject().put("type", "end").put("frames", frames)
            .put("averageCaptureMs", round2(totalCaptureMs / maxOf(captures, 1))).put("slowestCaptureMs", round2(slowestCaptureMs)))
        out.close()
        GZIPOutputStream(File(directory, "${file.name}.gz").outputStream()).use { file.inputStream().copyTo(it) }
        file.delete()
    }

    // Flushed per line so `ddp history` sees the open segment up to now.
    private fun line(json: JSONObject) {
        out.write(json.toString())
        out.write("\n")
        out.flush()
    }
}

/** Taps and swipes in the activity window's coordinates (the recording's), the ones adb sends included. */
private class TouchWatcher(private val inner: Window.Callback, private val slop: Float) : Window.Callback by inner {
    private var downX = 0f
    private var downY = 0f

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.x
                downY = event.y
            }
            MotionEvent.ACTION_UP -> {
                val moved = hypot(event.x - downX, event.y - downY) > slop
                val downWall = System.currentTimeMillis() - (SystemClock.uptimeMillis() - event.downTime)
                History.event(if (moved) "swipe" else "tap", JSONObject().put("x", round2(downX.toDouble()))
                    .put("y", round2(downY.toDouble())).apply {
                        if (moved) put("toX", round2(event.x.toDouble())).put("toY", round2(event.y.toDouble()))
                    }, downWall)
            }
        }
        return inner.dispatchTouchEvent(event)
    }
}
