package dev.agentddp

import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.os.Handler
import android.os.Looper
import org.json.JSONObject

/**
 * `screen.highlight`: a box over the live app for a while, for a person watching the phone. It goes in the activity
 * window's ViewOverlay, which draws on top without taking touches or moving the layout.
 */
internal object Highlight {
    private var shown: Pair<android.view.ViewGroupOverlay, GradientDrawable>? = null

    fun show(params: JSONObject): JSONObject {
        val rect = params.optJSONArray("rect")?.takeIf { it.length() == 4 }
            ?: throw IllegalArgumentException("rect is [x, y, width, height] in px")
        val ms = params.optLong("ms", 3000).coerceIn(100, 60_000)
        onMain {
            val root = FrameRecorder.resumedActivity?.window?.decorView as? android.view.ViewGroup ?: return@onMain
            shown?.let { (overlay, box) -> overlay.remove(box) }
            val box = GradientDrawable().apply {
                setStroke(6, Color.RED)
                setColor(Color.argb(38, 255, 0, 0))
                val (x, y) = rect.getDouble(0).toInt() to rect.getDouble(1).toInt()
                setBounds(x, y, x + rect.getDouble(2).toInt(), y + rect.getDouble(3).toInt())
            }
            root.overlay.add(box)
            shown = root.overlay to box
            Handler(Looper.getMainLooper()).postDelayed({
                root.overlay.remove(box)
                if (shown?.second === box) shown = null
            }, ms)
        }
        return JSONObject()
    }
}
