package dev.agentddp

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.RectF
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Base64
import android.view.PixelCopy
import android.view.View
import android.view.Window
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * What the app's own windows show, copied from their surfaces (`PixelCopy`), for the checks that judge what is on
 * screen. Only the app's windows: a system dialog or notification over the app is not the app's to answer for.
 * API 34 copies any window; before it, a window without a `Window` object (a popup) cannot be copied, and the regions
 * it covers say so.
 */
internal object Pixels {
    private val thread by lazy { HandlerThread("agentddp-pixels").apply { start() } }

    private class Root(val view: View, val left: Int, val top: Int, val window: Window?)

    /** Not on the main thread: it waits for the copies. Rectangles and pixels in the activity window's pixels. */
    fun regions(rects: List<RectF>): JSONObject {
        val (roots, width, height) = onMain {
            val decor = FrameRecorder.resumedActivity?.window?.decorView ?: return@onMain Triple(emptyList<Root>(), 0, 0)
            val origin = FrameCapture.windowOrigin(decor)
            val roots = FrameCapture.windowRoots(decor).map { root ->
                // A window's surface is copied as drawn, pan included: it goes where the window is.
                val at = FrameCapture.windowOrigin(root)
                Root(root, at[0] - origin[0], at[1] - origin[1], Hidden.windowOf(root))
            }
            Triple(roots, decor.width, decor.height)
        }
        val screen = Bitmap.createBitmap(width.coerceAtLeast(1), height.coerceAtLeast(1), Bitmap.Config.ARGB_8888)
        val canvas = Canvas(screen)
        val missing = mutableListOf<Pair<RectF, String>>()
        // Bottom window first; each one composites over what is under it, as on screen.
        for (root in roots) {
            val copy = copy(root)
            val area = RectF(root.left.toFloat(), root.top.toFloat(), (root.left + root.view.width).toFloat(), (root.top + root.view.height).toFloat())
            if (copy == null) missing += area to "a window this Android version cannot copy" else canvas.drawBitmap(copy, root.left.toFloat(), root.top.toFloat(), null)
        }
        val regions = rects.map { rect ->
            val left = rect.left.toInt().coerceAtLeast(0)
            val top = rect.top.toInt().coerceAtLeast(0)
            val right = kotlin.math.ceil(rect.right).toInt().coerceAtMost(width)
            val bottom = kotlin.math.ceil(rect.bottom).toInt().coerceAtMost(height)
            val region = JSONObject().put("rect", JSONArray(listOf(rect.left, rect.top, rect.width(), rect.height()).map { round2(it.toDouble()) }))
            val blocked = missing.firstOrNull { RectF.intersects(it.first, rect) }
            when {
                roots.isEmpty() -> region.put("missing", "no activity in the foreground")
                blocked != null -> region.put("missing", blocked.second)
                right <= left || bottom <= top -> region.put("missing", "off the window")
                else -> {
                    val argb = IntArray((right - left) * (bottom - top))
                    screen.getPixels(argb, 0, right - left, left, top, right - left, bottom - top)
                    val rgba = ByteArray(argb.size * 4)
                    argb.forEachIndexed { i, c ->
                        rgba[i * 4] = (c shr 16).toByte(); rgba[i * 4 + 1] = (c shr 8).toByte()
                        rgba[i * 4 + 2] = c.toByte(); rgba[i * 4 + 3] = (c ushr 24).toByte()
                    }
                    region.put("x", left).put("y", top).put("width", right - left).put("height", bottom - top)
                        .put("rgba", Base64.encodeToString(rgba, Base64.NO_WRAP))
                }
            }
        }
        screen.recycle()
        return JSONObject().put("scale", 1).put("regions", JSONArray(regions))
    }

    /** One window's surface as a bitmap, or null when it cannot be copied. */
    private fun copy(root: Root): Bitmap? {
        if (root.view.width <= 0 || root.view.height <= 0) return null
        val done = CountDownLatch(1)
        var result: Bitmap? = null
        if (Build.VERSION.SDK_INT >= 34) {
            val destination = Bitmap.createBitmap(root.view.width, root.view.height, Bitmap.Config.ARGB_8888)
            val request = PixelCopy.Request.Builder.ofWindow(root.view).setDestinationBitmap(destination).build()
            PixelCopy.request(request, Runnable::run) { copied ->
                if (copied.status == PixelCopy.SUCCESS) result = copied.bitmap
                done.countDown()
            }
        } else {
            val window = root.window ?: return null
            val bitmap = Bitmap.createBitmap(root.view.width, root.view.height, Bitmap.Config.ARGB_8888)
            PixelCopy.request(window, bitmap, { status ->
                if (status == PixelCopy.SUCCESS) result = bitmap
                done.countDown()
            }, Handler(thread.looper))
        }
        return if (done.await(2, TimeUnit.SECONDS)) result else null
    }
}
