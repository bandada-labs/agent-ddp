package dev.agentddp

import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.view.Choreographer
import android.view.FrameMetrics
import android.view.Window
import org.json.JSONObject
import kotlin.math.ceil

/**
 * `perf.metrics`: how the app runs for a while, from the platform's own clocks. Refreshes the main thread missed come
 * from Choreographer's vsync times, as on iOS from CADisplayLink's, so both platforms count the same thing. The work
 * per drawn frame comes from FrameMetrics: input, animation, layout, draw, sync and commands, the app's own share.
 * Not TOTAL_DURATION, which adds the wait for the GPU and the buffer queue: a pipelined frame can take longer than a
 * refresh with nothing missed (every frame on an emulator, which draws in software). History captures nothing
 * meanwhile, so its cost is not counted.
 */
internal object Perf {
    private val WORK = listOf(FrameMetrics.INPUT_HANDLING_DURATION, FrameMetrics.ANIMATION_DURATION,
        FrameMetrics.LAYOUT_MEASURE_DURATION, FrameMetrics.DRAW_DURATION, FrameMetrics.SYNC_DURATION,
        FrameMetrics.COMMAND_ISSUE_DURATION)

    fun measure(params: JSONObject): JSONObject {
        val ms = params.optLong("ms", 5000).coerceIn(100, 60_000)
        val window = onMain { FrameRecorder.resumedActivity?.window } ?: throw IllegalStateException("No activity in front")
        val periodMs = onMain {
            val activity = FrameRecorder.resumedActivity
            val rate = if (Build.VERSION.SDK_INT >= 30) activity?.display?.refreshRate
            else @Suppress("DEPRECATION") activity?.windowManager?.defaultDisplay?.refreshRate
            1000.0 / (rate ?: 60f)
        }
        val vsyncs = mutableListOf<Long>()
        val draws = mutableListOf<Double>()
        val metricsThread = HandlerThread("agentddp-perf").apply { start() }
        val metrics = Window.OnFrameMetricsAvailableListener { _, frame, _ ->
            val work = WORK.sumOf { frame.getMetric(it) } / 1e6
            synchronized(draws) { draws += work }
        }
        var running = true
        val tick = object : Choreographer.FrameCallback {
            override fun doFrame(frameTimeNanos: Long) {
                if (!running) return
                vsyncs += frameTimeNanos
                Choreographer.getInstance().postFrameCallback(this)
            }
        }
        onMain {
            History.quiet++
            window.addOnFrameMetricsAvailableListener(metrics, Handler(metricsThread.looper))
            Choreographer.getInstance().postFrameCallback(tick)
        }
        try {
            Thread.sleep(ms)
        } finally {
            onMain {
                running = false
                Choreographer.getInstance().removeFrameCallback(tick)
                window.removeOnFrameMetricsAvailableListener(metrics)
                History.quiet--
            }
            metricsThread.quitSafely()
        }
        val gaps = onMain { vsyncs.zipWithNext { a, b -> (b - a) / 1e6 } }
        val drawn = synchronized(draws) { draws.sorted() }
        return JSONObject().put("ms", ms).put("refreshMs", round2(periodMs))
            .put("refreshes", gaps.size)
            .put("missed", gaps.sumOf { missed(it, periodMs) })
            .put("worstGapMs", round2(gaps.maxOrNull() ?: 0.0))
            .put("drawn", drawn.size)
            .put("drawMeanMs", if (drawn.isEmpty()) JSONObject.NULL else round2(drawn.average()))
            .put("drawP95Ms", if (drawn.isEmpty()) JSONObject.NULL else round2(drawn[ceil(drawn.size * 0.95).toInt() - 1]))
            .put("drawnOverRefresh", drawn.count { it > periodMs })
    }

    /** A gap of two periods is one refresh missed; a little over one period is timer noise, not a miss. */
    private fun missed(gapMs: Double, periodMs: Double): Int = maxOf(0, Math.round(gapMs / periodMs).toInt() - 1)
}
