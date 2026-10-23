package dev.agentddp

import android.Manifest
import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.PointF
import android.graphics.RectF
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.Choreographer
import android.view.View
import androidx.compose.ui.platform.isDebugInspectorInfoEnabled
import androidx.compose.ui.semantics.getOrNull
import org.json.JSONObject
import java.io.File

/** Starts with the app (debugImplementation only), so release builds never contain it. */
class AgentDdpStarter : ContentProvider() {
    override fun onCreate(): Boolean {
        // Makes Compose modifiers expose their values (border width, colors) for reading.
        isDebugInspectorInfoEnabled = true
        TimeControl.installComposeClock()
        val application = context!!.applicationContext as Application
        FrameRecorder.start(application)
        History.start(application)
        LiveServer.start(application.packageName)
        CdpServer.start(application)
        return true
    }

    override fun query(uri: Uri, p: Array<String>?, s: String?, a: Array<String>?, o: String?) = null
    override fun getType(uri: Uri) = null
    override fun insert(uri: Uri, values: ContentValues?) = null
    override fun delete(uri: Uri, s: String?, a: Array<String>?) = 0
    override fun update(uri: Uri, v: ContentValues?, s: String?, a: Array<String>?) = 0
}

/** Starts a recording on the `adb shell am broadcast` request (offline mode: the file in the app's files). */
internal object FrameRecorder {
    private const val REQUEST_ACTION = "dev.agentddp.RECORD"
    private const val DEFAULT_FRAME_COUNT = 120
    private const val MAX_FRAME_COUNT = 600

    private lateinit var outputDirectory: File
    var resumedActivity: Activity? = null
        private set
    private var stream: FrameStream? = null
    private var writer: java.io.Writer? = null

    fun start(application: Application) {
        outputDirectory = File(application.filesDir, "agent-ddp").apply { mkdirs() }
        application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) {
                resumedActivity = activity
                History.onResumed(activity)
            }
            override fun onActivityPaused(activity: Activity) {
                if (resumedActivity === activity) resumedActivity = null
            }
            override fun onActivityCreated(activity: Activity, state: Bundle?) {}
            override fun onActivityStarted(activity: Activity) {}
            override fun onActivityStopped(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) {}
            override fun onActivityDestroyed(activity: Activity) {}
        })
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                beginRecording(intent.getIntExtra("frames", DEFAULT_FRAME_COUNT).coerceIn(1, MAX_FRAME_COUNT))
            }
        }
        // Exported so `adb shell am broadcast` reaches it, but only senders holding DUMP (the shell,
        // not other apps) can start a recording.
        val filter = IntentFilter(REQUEST_ACTION)
        if (Build.VERSION.SDK_INT >= 33) {
            application.registerReceiver(receiver, filter, Manifest.permission.DUMP, null, Context.RECEIVER_EXPORTED)
        } else {
            application.registerReceiver(receiver, filter, Manifest.permission.DUMP, null)
        }
    }

    private fun beginRecording(frameCount: Int) {
        if (stream != null || resumedActivity == null) return
        val output = File(outputDirectory, "recording.partial").bufferedWriter()
        writer = output
        stream = FrameStream(frameCount, send = { line -> output.write(line.toString()); output.write("\n"); true },
            finished = ::finishRecording).also { it.start() }
    }

    private fun finishRecording() {
        writer?.close()
        writer = null
        stream = null
        File(outputDirectory, "recording.partial").renameTo(File(outputDirectory, "recording.jsonl"))
    }
}

/**
 * Captures display frames and hands each line (header, frames, end) to [send], for a file or a live client.
 * [frameLimit] 0 runs until [send] returns false or [stop] is called. Main thread only.
 */
internal class FrameStream(
    private val frameLimit: Int,
    private val send: (JSONObject) -> Boolean,
    private val finished: () -> Unit = {},
) : Choreographer.FrameCallback {
    companion object {
        /** Streams capturing now. History leaves the capturing to them meanwhile. */
        var active = 0
            private set
    }

    private var running = false
    private var frameIndex = 0
    private var firstFrameNanos = 0L
    private var slowestCaptureMs = 0.0
    private var totalCaptureMs = 0.0

    fun start() {
        val header = FrameCapture.header(frameLimit)
        if (header == null || !send(header)) return finished()
        running = true
        active++
        Choreographer.getInstance().postFrameCallback(this)
    }

    fun stop() {
        if (!running) return
        running = false
        active--
        Choreographer.getInstance().removeFrameCallback(this)
        send(JSONObject().put("type", "end").put("frames", frameIndex).put("slowestCaptureMs", round2(slowestCaptureMs))
            .put("averageCaptureMs", round2(totalCaptureMs / maxOf(frameIndex, 1))))
        finished()
    }

    override fun doFrame(frameTimeNanos: Long) {
        if (!running) return
        if (frameIndex == 0) firstFrameNanos = frameTimeNanos
        val captureStart = SystemClock.elapsedRealtimeNanos()
        val line = FrameCapture.frame(frameIndex, (frameTimeNanos - firstFrameNanos) / 1e6)
        val captureMs = (SystemClock.elapsedRealtimeNanos() - captureStart) / 1e6
        slowestCaptureMs = maxOf(slowestCaptureMs, captureMs)
        totalCaptureMs += captureMs
        frameIndex++
        val sent = send(line)
        History.offer(line)
        if (!sent || (frameLimit > 0 && frameIndex >= frameLimit)) stop()
        else Choreographer.getInstance().postFrameCallback(this)
    }
}

internal object FrameCapture {
    /** Null when no activity is in the foreground. */
    /** The DDP protocol version (docs/protocol.md). Raise it only when a field changes meaning or goes away. */
    const val PROTOCOL = 1

    fun header(frames: Int): JSONObject? {
        val activity = FrameRecorder.resumedActivity ?: return null
        return JSONObject().put("type", "header").put("protocol", PROTOCOL).put("platform", "android").put("unit", "px")
            .put("scale", activity.resources.displayMetrics.density.toDouble()).put("frames", frames)
            .put("startedAt", System.currentTimeMillis())
    }

    /** Main thread only. Every window of the app, bottom first: the activity, then popups, menus and dialogs. */
    fun frame(index: Int, milliseconds: Double): JSONObject {
        val decor = FrameRecorder.resumedActivity?.window?.decorView
        val capture = Capture(decor?.resources?.displayMetrics?.density ?: 1f)
        if (decor != null) {
            val origin = IntArray(2).also(decor::getLocationOnScreen)
            for (root in windowRoots(decor)) {
                val onScreen = IntArray(2).also(root::getLocationOnScreen)
                capture.windowOffset = PointF((onScreen[0] - origin[0]).toFloat(), (onScreen[1] - origin[1]).toFloat())
                val bounds = RectF(0f, 0f, root.width.toFloat(), root.height.toFloat())
                    .apply { offset(capture.windowOffset.x, capture.windowOffset.y) }
                capture.walkView(root, bounds, 1f)
            }
        }
        return JSONObject().put("type", "frame").put("i", index).put("t", round2(milliseconds))
            .put("elements", capture.elements).put("nodes", capture.nodes)
    }

    /**
     * The app's window roots in the order they were added (later ones draw on top). Popup windows (DropdownMenu,
     * Dialog, tooltips) are windows of their own that the activity's view tree does not hold.
     * WindowManagerGlobal is hidden API; Debug builds may use it. Falls back to the activity window alone.
     */
    fun windowRoots(decor: View): List<View> = runCatching {
        val (instance, getNames, getRootView) = Hidden.windowManager ?: return listOf(decor)
        @Suppress("UNCHECKED_CAST")
        val names = getNames.invoke(instance) as Array<String>
        names.mapNotNull { getRootView.invoke(instance, it) as? View }
            .filter { it.isAttachedToWindow && it.windowVisibility == View.VISIBLE && it.width > 0 }
            .ifEmpty { null }
    }.getOrNull()?.let { roots -> if (decor in roots) roots else listOf(decor) + roots } ?: listOf(decor)
}
