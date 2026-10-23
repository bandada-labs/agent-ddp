package dev.agentddp

import android.graphics.PointF
import android.graphics.RectF
import android.os.SystemClock
import android.text.InputType
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.inputmethod.EditorInfo

/**
 * The CDP Input domain: touches and typing dispatched into the app's own windows, the way the system delivers them,
 * so gesture detectors and Compose see a real finger. Main thread only.
 */
// ponytail: one finger. Add more when a client sends multi-touch (pinch).
internal object Input {
    private class Finger(val root: View, val downTime: Long, var screenPx: PointF)

    private var finger: Finger? = null
    val isTouching get() = finger != null

    /**
     * Moves the one finger to [point] in dp on screen, or lifts it when null. Pressing, moving and lifting follow from
     * whether a finger is down, so mouse and touch events share this.
     */
    fun touch(point: PointF?) {
        val down = finger
        when {
            down == null && point != null -> {
                val screenPx = toPx(point)
                val root = roots().lastOrNull { it.isShown && screenRect(it).contains(screenPx.x, screenPx.y) }
                    ?: throw IllegalStateException("No window at that point")
                Finger(root, SystemClock.uptimeMillis(), screenPx).also { finger = it }.dispatch(MotionEvent.ACTION_DOWN)
            }
            down != null && point != null -> down.apply { screenPx = toPx(point) }.dispatch(MotionEvent.ACTION_MOVE)
            down != null -> {
                finger = null
                down.dispatch(MotionEvent.ACTION_UP)
            }
        }
    }

    /**
     * Text into the focused field through its input connection, as the keyboard types it. Nothing happens when no
     * field has focus, like keys sent to a page with nothing focused.
     */
    fun type(text: String) {
        val connection = focused()?.onCreateInputConnection(EditorInfo()) ?: return
        text.replace("\r", "\n").split("\n").forEachIndexed { index, line ->
            if (index > 0) press("Enter")
            if (line.isNotEmpty()) connection.commitText(line, 1)
        }
    }

    /**
     * A key without text of its own, by its CDP `key` name. Other keys do nothing. Enter in a single-line field runs
     * the field's action, as the keyboard's action button does: Compose ignores a hardware Enter there.
     */
    fun press(key: String) {
        val code = KEY_CODES[key] ?: return
        val view = focused() ?: return
        val info = EditorInfo()
        val connection = view.onCreateInputConnection(info)
        val action = info.imeOptions and EditorInfo.IME_MASK_ACTION
        val singleLine = info.inputType and InputType.TYPE_TEXT_FLAG_MULTI_LINE == 0
        if (code == KeyEvent.KEYCODE_ENTER && connection != null && singleLine && action != EditorInfo.IME_ACTION_NONE &&
            action != EditorInfo.IME_ACTION_UNSPECIFIED
        ) {
            connection.performEditorAction(action)
            return
        }
        listOf(KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP).forEach { view.rootView.dispatchKeyEvent(KeyEvent(it, code)) }
    }

    private val KEY_CODES = mapOf("Enter" to KeyEvent.KEYCODE_ENTER, "Tab" to KeyEvent.KEYCODE_TAB, "Backspace" to KeyEvent.KEYCODE_DEL)

    private fun Finger.dispatch(action: Int) {
        val origin = screenRect(root)
        val event = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), action, screenPx.x - origin.left, screenPx.y - origin.top, 0)
        event.source = InputDevice.SOURCE_TOUCHSCREEN
        root.dispatchTouchEvent(event)
        event.recycle()
    }

    private fun roots(): List<View> = FrameRecorder.resumedActivity?.window?.decorView?.let(FrameCapture::windowRoots).orEmpty()

    private fun focused(): View? = roots().asReversed().firstNotNullOfOrNull { it.findFocus() }

    private fun toPx(point: PointF): PointF {
        val density = roots().firstOrNull()?.resources?.displayMetrics?.density ?: 1f
        return PointF(point.x * density, point.y * density)
    }

    private fun screenRect(view: View): RectF {
        val origin = IntArray(2).also(view::getLocationOnScreen)
        return RectF(origin[0].toFloat(), origin[1].toFloat(), (origin[0] + view.width).toFloat(), (origin[1] + view.height).toFloat())
    }
}
