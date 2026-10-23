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
import android.graphics.RectF
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.Choreographer
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.InspectableValue
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.platform.isDebugInspectorInfoEnabled
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import org.json.JSONArray
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
        LiveServer.start(application.packageName)
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
            override fun onActivityResumed(activity: Activity) { resumedActivity = activity }
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
    private var running = false
    private var frameIndex = 0
    private var firstFrameNanos = 0L
    private var slowestCaptureMs = 0.0
    private var totalCaptureMs = 0.0

    fun start() {
        val header = FrameCapture.header(frameLimit)
        if (header == null || !send(header)) return finished()
        running = true
        Choreographer.getInstance().postFrameCallback(this)
    }

    fun stop() {
        if (!running) return
        running = false
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
        if (!send(line) || (frameLimit > 0 && frameIndex >= frameLimit)) stop()
        else Choreographer.getInstance().postFrameCallback(this)
    }
}

internal object FrameCapture {
    /** Null when no activity is in the foreground. */
    fun header(frames: Int): JSONObject? {
        val activity = FrameRecorder.resumedActivity ?: return null
        return JSONObject().put("type", "header").put("platform", "android").put("unit", "px")
            .put("scale", activity.resources.displayMetrics.density.toDouble()).put("frames", frames)
    }

    /** Main thread only. */
    fun frame(index: Int, milliseconds: Double): JSONObject {
        val root = FrameRecorder.resumedActivity?.window?.decorView
        val capture = Capture(root?.resources?.displayMetrics?.density ?: 1f)
        if (root != null) capture.walkView(root, RectF(0f, 0f, root.width.toFloat(), root.height.toFloat()), 1f)
        return JSONObject().put("type", "frame").put("i", index).put("t", round2(milliseconds))
            .put("elements", capture.elements).put("nodes", capture.nodes)
    }
}

/** One frame: elements (what an agent names) and painted nodes in draw order (later covers earlier). */
private class Capture(private val density: Float) {
    val elements = JSONArray()
    val nodes = JSONArray()
    private val composeDensity = Density(density)

    // ponytail: View draw order is child index, re-sorted by Z; custom getChildDrawingOrder is ignored.
    fun walkView(view: View, clip: RectF, parentAlpha: Float) {
        if (view.visibility != View.VISIBLE) return
        val alpha = parentAlpha * view.alpha
        if (alpha <= 0.001f) return
        val location = IntArray(2).also(view::getLocationInWindow)
        val frame = RectF(location[0].toFloat(), location[1].toFloat(),
            (location[0] + view.width).toFloat(), (location[1] + view.height).toFloat())
        val visible = RectF(frame).takeIf { it.intersect(clip) } ?: return

        val source = view.javaClass.simpleName
        view.background?.let { addNode(drawableNode(it).put("source", source), frame, visible, alpha) }
        if (view is TextView && view.text.isNotEmpty() || view is ImageView && view.drawable != null) {
            addNode(JSONObject().put("kind", "bitmap").put("source", source), frame, visible, alpha)
        }
        viewElement(view, frame)

        if (view is ViewRootForTest) {
            walkSemantics(view.semanticsOwner.unmergedRootSemanticsNode, visible, alpha)
        }
        if (view is ViewGroup) {
            val childClip = if (view.clipChildren) visible else clip
            (0 until view.childCount).map(view::getChildAt).sortedBy { it.z }
                .forEach { walkView(it, childClip, alpha) }
        }
    }

    private fun drawableNode(drawable: Drawable): JSONObject {
        val node = JSONObject()
        val color = when (drawable) {
            is ColorDrawable -> drawable.color
            is GradientDrawable -> drawable.color?.defaultColor
            else -> null
        }
        return if (color != null) node.put("kind", "layer").also { hex(color)?.let { node.put("fill", it) } }
        else node.put("kind", "bitmap")
    }

    private fun viewElement(view: View, frame: RectF) {
        val label = (view as? TextView)?.text?.toString()?.takeIf { it.isNotEmpty() }
            ?: view.contentDescription?.toString()
        val id = view.id.takeIf { it != View.NO_ID }?.let {
            runCatching {
                if (view.resources.getResourcePackageName(it) == "android") null
                else view.resources.getResourceEntryName(it)
            }.getOrNull()
        }
        if (label == null && id == null) return
        elements.put(JSONObject().put("frame", rectArray(frame)).apply {
            id?.let { put("id", it) }
            label?.let { put("label", it) }
        })
    }

    // ponytail: only Compose nodes that carry semantics are seen (testTag, text, clickable...).
    // A plain Box with a background and no semantics is invisible here.
    // A clickable that merges its children (Button, FilterChip, Modifier.clickable) is one element,
    // the way agent-device and uiautomator see it: its own frame, fill and border, its children's text.
    // Untagged children inside it are not elements of their own.
    // clipRound: the innermost rounded clip around this node ([x, y, w, h, radius]), so `ddp check corners`
    // knows which draws a rounded clip cuts.
    private fun walkSemantics(node: SemanticsNode, clip: RectF, parentAlpha: Float, insideMerged: Boolean = false,
                              clipRound: JSONArray? = null) {
        val position = node.positionInWindow
        val frame = RectF(position.x, position.y, position.x + node.size.width, position.y + node.size.height)
        val modifiers = node.layoutInfo.getModifierInfo().mapNotNull { inspect(it.modifier) }
        val alpha = parentAlpha * modifiers.filter { it.first == "graphicsLayer" || it.first == "alpha" }
            .fold(1f) { total, (_, props) -> total * ((props["alpha"] as? Float) ?: 1f) }
        val visible = RectF(frame).takeIf { it.intersect(clip) }
        val nodeSize = Size(node.size.width.toFloat(), node.size.height.toFloat())

        val text = textOf(node)
        val tag = node.config.getOrNull(SemanticsProperties.TestTag)
        val merges = !insideMerged && node.config.isMergingSemanticsOfDescendants
        val label = text ?: node.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString()
            ?: if (merges) childrenText(node) else null
        if (tag != null || (label != null && !insideMerged)) {
            elements.put(JSONObject().put("frame", rectArray(frame)).apply {
                tag?.let { put("id", it) }
                label?.let { put("label", it) }
            })
        }

        val clipLayer = modifiers.firstOrNull { it.first == "graphicsLayer" && it.second["clip"] == true }
        val childClip = if (clipLayer != null && visible != null) visible else clip
        val childClipRound = clipLayer?.let { cornerOf(it.second["shape"], nodeSize) }
            ?.let { radius -> rectArray(frame).put(radius) } ?: clipRound

        if (visible != null && alpha > 0.001f) {
            modifiers.filter { it.first == "background" }.forEach { (_, props) ->
                val fill = colorOf(props["color"] ?: props["brush"])
                val node = JSONObject().put("kind", "layer")
                fill?.let { node.put("fill", it) }
                cornerOf(props["shape"], nodeSize)?.let { node.put("corner", it) }
                addNode(node, frame, visible, alpha, clipRound)
            }
            // ponytail: drawBehind only (what Canvas uses). drawWithCache and drawWithContent need
            // internal Compose scopes; add them when an app draws its animation that way.
            modifiers.filter { it.first == "drawBehind" }.forEach { (_, props) ->
                @Suppress("UNCHECKED_CAST")
                val onDraw = props["onDraw"] as? DrawScope.() -> Unit ?: return@forEach
                // The node's own clip cuts what it draws, so draws use the child clip.
                DrawRecorder.replay(onDraw, nodeSize, composeDensity)?.forEach { op ->
                    val opFrame = RectF(op.bounds).apply { offset(position.x, position.y) }
                    val opVisible = RectF(opFrame).takeIf { it.intersect(childClip) } ?: return@forEach
                    op.points?.let { points ->
                        op.paint.put("points", JSONArray(points.mapIndexed { i, value ->
                            round2((value + if (i % 2 == 0) position.x else position.y).toDouble())
                        }))
                    }
                    addNode(op.paint.put("op", op.op), opFrame, opVisible, alpha, childClipRound)
                }
            }
            if (text != null) addNode(JSONObject().put("kind", "bitmap"), frame, visible, alpha)
        }

        node.children.forEach { walkSemantics(it, childClip, alpha, insideMerged || merges, childClipRound) }

        // Border draws after content, so it goes after the children in paint order.
        if (visible != null && alpha > 0.001f) {
            modifiers.filter { it.first == "border" }.forEach { (_, props) ->
                val width = (props["width"] as? Dp)?.value?.times(density) ?: return@forEach
                val color = colorOf(props["color"] ?: props["brush"]) ?: return@forEach
                if (width <= 0f) return@forEach
                val node = JSONObject().put("kind", "layer")
                    .put("border", JSONObject().put("width", round2(width.toDouble())).put("color", color))
                cornerOf(props["shape"], nodeSize)?.let { node.put("corner", it) }
                addNode(node, frame, visible, alpha, clipRound)
            }
        }
    }

    private fun textOf(node: SemanticsNode) =
        node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }

    private fun childrenText(node: SemanticsNode): String? =
        node.children.filterNot { it.config.isMergingSemanticsOfDescendants }
            .flatMap { listOfNotNull(textOf(it), childrenText(it)) }
            .joinToString(" ").ifEmpty { null }

    private fun cornerOf(shape: Any?, size: Size): Double? {
        val outline = (shape as? Shape)?.createOutline(size, LayoutDirection.Ltr, composeDensity)
        val radius = (outline as? Outline.Rounded)?.roundRect?.topLeftCornerRadius?.x ?: return null
        return round2(radius.toDouble()).takeIf { it > 0 }
    }

    private fun addNode(node: JSONObject, frame: RectF, visible: RectF, alpha: Float, clipRound: JSONArray? = null) {
        node.put("z", nodes.length()).put("frame", rectArray(frame))
        clipRound?.let { node.put("clipRound", it) }
        if (visible != frame) node.put("visible", rectArray(visible))
        if (alpha < 0.999f) node.put("opacity", round2(alpha.toDouble()))
        nodes.put(node)
    }
}

/** A modifier's inspector name and properties (needs isDebugInspectorInfoEnabled). */
internal fun inspect(modifier: Any): Pair<String, Map<String, Any?>>? {
    val value = modifier as? InspectableValue ?: return null
    val name = value.nameFallback ?: modifier::class.simpleName ?: return null
    return name to value.inspectableElements.associate { it.name to it.value }
}

internal fun colorOf(value: Any?): String? = when (value) {
    is Color -> hex(value.toArgb())
    is SolidColor -> hex(value.value.toArgb())
    else -> null
}

/** "#RRGGBBAA", or null when fully transparent. */
internal fun hex(argb: Int): String? {
    val alpha = argb ushr 24
    if (alpha == 0) return null
    return "#%06X%02X".format(argb and 0xFFFFFF, alpha)
}

private fun rectArray(rect: RectF) =
    JSONArray(listOf(rect.left, rect.top, rect.width(), rect.height()).map { round2(it.toDouble()) })

internal fun round2(value: Double) = Math.round(value * 100) / 100.0
