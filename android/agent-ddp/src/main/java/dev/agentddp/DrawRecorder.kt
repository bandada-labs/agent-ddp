package dev.agentddp

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import androidx.compose.ui.draw.CacheDrawScope
import androidx.compose.ui.draw.DrawResult
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.CanvasDrawScope
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import org.json.JSONObject
import java.lang.reflect.Proxy
import androidx.compose.ui.graphics.Canvas as ComposeCanvas

/** One shape a `Canvas` / `drawBehind` lambda drew, in the node's local pixels. */
internal class DrawOp(val op: String, val bounds: RectF, val paint: JSONObject, val points: FloatArray? = null)

/**
 * Replays a Compose draw lambda into a canvas that writes down each draw call instead of drawing.
 * The lambda reads the same animated state Compose reads, so the ops match what is on screen this frame.
 */
@Suppress("DEPRECATION") // getMatrix(): the only public way to read the current transform.
internal class DrawRecorder private constructor() : Canvas() {
    private val ops = mutableListOf<DrawOp>()
    private val transform = Matrix()

    private fun record(op: String, bounds: RectF, paint: Paint?, points: FloatArray? = null) {
        if (ops.size >= MAX_OPS_PER_LAMBDA) return
        val description = describe(paint) ?: return
        getMatrix(transform)
        transform.mapRect(bounds)
        points?.let(transform::mapPoints)
        ops += DrawOp(op, bounds, description, points)
    }

    override fun drawRect(left: Float, top: Float, right: Float, bottom: Float, paint: Paint) =
        record("rect", RectF(left, top, right, bottom), paint)

    override fun drawRoundRect(left: Float, top: Float, right: Float, bottom: Float, rx: Float, ry: Float, paint: Paint) =
        record("roundRect", RectF(left, top, right, bottom), paint)

    override fun drawOval(left: Float, top: Float, right: Float, bottom: Float, paint: Paint) =
        record("oval", RectF(left, top, right, bottom), paint)

    override fun drawCircle(cx: Float, cy: Float, radius: Float, paint: Paint) =
        record("circle", RectF(cx - radius, cy - radius, cx + radius, cy + radius), paint)

    override fun drawArc(left: Float, top: Float, right: Float, bottom: Float,
                         startAngle: Float, sweepAngle: Float, useCenter: Boolean, paint: Paint) =
        record("arc", RectF(left, top, right, bottom), paint)

    override fun drawLine(startX: Float, startY: Float, stopX: Float, stopY: Float, paint: Paint) =
        record("line", RectF(minOf(startX, stopX), minOf(startY, stopY), maxOf(startX, stopX), maxOf(startY, stopY)), paint,
            floatArrayOf(startX, startY, stopX, stopY))

    override fun drawPath(path: Path, paint: Paint) =
        record("path", RectF().also { path.computeBounds(it, true) }, paint)

    override fun drawBitmap(bitmap: Bitmap, left: Float, top: Float, paint: Paint?) =
        record("image", RectF(left, top, left + bitmap.width, top + bitmap.height), paint)

    override fun drawBitmap(bitmap: Bitmap, src: Rect?, dst: RectF, paint: Paint?) =
        record("image", RectF(dst), paint)

    override fun drawBitmap(bitmap: Bitmap, src: Rect?, dst: Rect, paint: Paint?) =
        record("image", RectF(dst), paint)

    // Text: what Compose's text in a Canvas (drawText with a TextMeasurer) ends in, through its layout.
    override fun drawText(text: CharArray, index: Int, count: Int, x: Float, y: Float, paint: Paint) =
        recordText(String(text, index, count), x, y, paint)

    override fun drawText(text: String, x: Float, y: Float, paint: Paint) = recordText(text, x, y, paint)

    override fun drawText(text: String, start: Int, end: Int, x: Float, y: Float, paint: Paint) =
        recordText(text.substring(start, end), x, y, paint)

    override fun drawText(text: CharSequence, start: Int, end: Int, x: Float, y: Float, paint: Paint) =
        recordText(text.subSequence(start, end).toString(), x, y, paint)

    override fun drawTextRun(text: CharArray, index: Int, count: Int, contextIndex: Int, contextCount: Int,
                             x: Float, y: Float, isRtl: Boolean, paint: Paint) =
        recordText(String(text, index, count), x, y, paint)

    override fun drawTextRun(text: CharSequence, start: Int, end: Int, contextStart: Int, contextEnd: Int,
                             x: Float, y: Float, isRtl: Boolean, paint: Paint) =
        recordText(text.subSequence(start, end).toString(), x, y, paint)

    /** The text's bounds as the paint measures them, at its baseline. */
    private fun recordText(text: String, x: Float, y: Float, paint: Paint) {
        if (text.isBlank()) return
        val bounds = Rect().also { paint.getTextBounds(text, 0, text.length, it) }
        val size = ops.size
        record("text", RectF(bounds).apply { offset(x, y) }, paint)
        if (ops.size > size) ops.last().paint.put("text", text)
    }

    companion object {
        // ponytail: caps a dense hatch or particle field; raise it if an agent needs every op.
        private const val MAX_OPS_PER_LAMBDA = 100

        /** Ops the lambda draws at this size, or null when it can't be replayed outside Compose. */
        fun replay(onDraw: DrawScope.() -> Unit, size: Size, density: Density): List<DrawOp>? = runCatching {
            val recorder = DrawRecorder()
            CanvasDrawScope().draw(density, LayoutDirection.Ltr, ComposeCanvas(recorder), size) { onDraw() }
            recorder.ops
        }.getOrNull()

        /** A `drawWithContent` lambda: `drawContent()` draws nothing here, the content's own nodes are read apart. */
        fun replayContent(onDraw: ContentDrawScope.() -> Unit, size: Size, density: Density): List<DrawOp>? = runCatching {
            val recorder = DrawRecorder()
            CanvasDrawScope().draw(density, LayoutDirection.Ltr, ComposeCanvas(recorder), size) {
                val scope: DrawScope = this
                object : ContentDrawScope, DrawScope by scope {
                    override fun drawContent() {}
                }.onDraw()
            }
            recorder.ops
        }.getOrNull()

        /**
         * A `drawWithCache` lambda: built in a CacheDrawScope sized like the node, then its draw block replayed like a
         * `drawWithContent` one. CacheDrawScope's constructor, its size parameters and DrawResult's block are Compose
         * internals (the `compose.drawCache` hook); a cache that needs a graphics layer fails it and draws nothing.
         */
        fun replayCache(onBuild: CacheDrawScope.() -> DrawResult, size: Size, density: Density): List<DrawOp>? {
            val block = runCatching {
                val scope = CacheDrawScope::class.java.getDeclaredConstructor().newInstance()
                val paramsType = Class.forName("androidx.compose.ui.draw.BuildDrawCacheParams")
                // Size is a value class: its getter returns the two floats packed in a long.
                val packed = (java.lang.Float.floatToRawIntBits(size.width).toLong() shl 32) or
                    (java.lang.Float.floatToRawIntBits(size.height).toLong() and 0xFFFFFFFFL)
                val params = Proxy.newProxyInstance(paramsType.classLoader, arrayOf(paramsType)) { _, method, _ ->
                    when {
                        method.name.startsWith("getSize") -> packed
                        method.name == "getLayoutDirection" -> LayoutDirection.Ltr
                        method.name == "getDensity" -> density
                        else -> null
                    }
                }
                CacheDrawScope::class.java.getMethod("setCacheParams\$ui_release", paramsType).invoke(scope, params)
                val result = scope.onBuild()
                @Suppress("UNCHECKED_CAST")
                DrawResult::class.java.getMethod("getBlock\$ui_release").invoke(result) as ContentDrawScope.() -> Unit
            }.onFailure { Hooks.failed("compose.drawCache", it.toString()) }.getOrNull() ?: return null
            Hooks.ok("compose.drawCache")
            return replayContent(block, size, density)
        }

        /** Fill or stroke with its color, or null when the paint draws nothing visible. */
        private fun describe(paint: Paint?): JSONObject? {
            val node = JSONObject().put("kind", "draw")
            if (paint == null) return node
            if (paint.alpha == 0) return null
            if (paint.shader != null) return node.put("shader", true)
            val color = hex(paint.color) ?: return null
            return if (paint.style == Paint.Style.STROKE)
                node.put("border", JSONObject().put("width", round2(maxOf(paint.strokeWidth, 1f).toDouble())).put("color", color))
            else node.put("fill", color)
        }
    }
}
