package dev.agentddp

import android.view.Choreographer
import androidx.compose.runtime.MonotonicFrameClock
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.InternalComposeUiApi
import androidx.compose.ui.platform.WindowRecomposerPolicy
import androidx.compose.ui.platform.createLifecycleAwareWindowRecomposer
import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONObject

/**
 * ddp's controllable clock for the app's Debug composition root: plug [elapsedMillis] into the app's own
 * clock port and its timelines pause, step and rewind with `ddp time`.
 */
object AgentDdpTime {
    fun elapsedMillis(): Long = TimeControl.nowNanos() / 1_000_000
}

/**
 * `ddp time`: one virtual clock. Compose animations follow it through a frame clock installed for every
 * window (Debug only); the app's own timelines follow it through [AgentDdpTime]. Views' ValueAnimators and
 * system-drawn parts (keyboard, system menus) do not.
 */
internal object TimeControl {
    private const val FRAME_NANOS = 16_666_667L
    private var rate = 1.0
    private var anchorReal = System.nanoTime()
    private var anchorVirtual = anchorReal
    private var pausePoint: Long? = null

    @Synchronized
    fun nowNanos(): Long = anchorVirtual + ((System.nanoTime() - anchorReal) * rate).toLong()

    @Synchronized
    fun virtualNanos(realNanos: Long): Long = anchorVirtual + ((realNanos - anchorReal) * rate).toLong()

    @OptIn(InternalComposeUiApi::class, ExperimentalComposeUiApi::class)
    fun installComposeClock() {
        WindowRecomposerPolicy.setFactory { rootView -> rootView.createLifecycleAwareWindowRecomposer(ControllableFrameClock) }
    }

    @Synchronized
    fun handle(action: String, value: Double?): JSONObject {
        when (action) {
            "pause" -> {
                if (pausePoint == null) pausePoint = nowNanos()
                set(nowNanos(), 0.0)
            }
            "resume" -> {
                pausePoint = null
                set(nowNanos(), 1.0)
            }
            "rate" -> {
                if (value == null || value < 0) return error("rate needs a number >= 0, e.g. 0.1")
                set(nowNanos(), value)
            }
            "step" -> {
                if (rate != 0.0) return error("pause first, then step")
                set(nowNanos() + (value ?: 1.0).toLong() * FRAME_NANOS, 0.0)
            }
            "seek" -> {
                val from = pausePoint ?: return error("pause first; seek takes a time from the pause point")
                set(from + ((value ?: return error("seek needs a time")) * 1_000_000).toLong(), 0.0)
            }
            "status" -> Unit
            else -> return error("time actions: pause, resume, step [frames], rate <factor>, seek <time>, status")
        }
        return JSONObject().put("type", "time").put("rate", rate).put("paused", rate == 0.0).apply {
            pausePoint?.let { put("sincePauseMs", round2((nowNanos() - it) / 1e6)) }
        }
    }

    private fun set(virtual: Long, newRate: Double) {
        anchorReal = System.nanoTime()
        anchorVirtual = virtual
        rate = newRate
    }

    private fun error(message: String) = JSONObject().put("type", "error").put("error", message)
}

/**
 * Compose's frame clock on ddp's time. Frames keep coming while paused (state changes still draw), but
 * their time stands still, so animations do. Time never goes back here: a seek back only rewinds the app's
 * own clock, Compose animations stay where they are.
 */
private object ControllableFrameClock : MonotonicFrameClock {
    private var lastDelivered = 0L

    override suspend fun <R> withFrameNanos(onFrame: (frameTimeNanos: Long) -> R): R =
        suspendCancellableCoroutine { continuation ->
            // Compose asks on the main thread; cancellation may come from another one.
            val choreographer = Choreographer.getInstance()
            val callback = Choreographer.FrameCallback { realNanos ->
                lastDelivered = maxOf(lastDelivered, TimeControl.virtualNanos(realNanos))
                continuation.resumeWith(runCatching { onFrame(lastDelivered) })
            }
            choreographer.postFrameCallback(callback)
            continuation.invokeOnCancellation { choreographer.removeFrameCallback(callback) }
        }
}
