package dev.agentddp

import android.os.Handler
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import androidx.compose.runtime.Composer
import androidx.compose.runtime.Composition
import androidx.compose.runtime.CompositionTracer
import androidx.compose.runtime.ExperimentalComposeRuntimeApi
import androidx.compose.runtime.InternalComposeTracingApi
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.RecomposeScope
import androidx.compose.runtime.State
import androidx.compose.runtime.tooling.CompositionObserver
import androidx.compose.runtime.tooling.CompositionObserverHandle
import androidx.compose.runtime.tooling.RecomposeScopeObserver
import androidx.compose.runtime.tooling.observe
import androidx.compose.ui.R
import androidx.compose.ui.platform.ViewRootForTest
import org.json.JSONArray
import org.json.JSONObject
import java.util.IdentityHashMap

/**
 * `compose.recompositions.start` / `.stop` (`ddp recompositions`): which composables ran in a time window, how often,
 * and which state objects made Compose run them again. All of it is Compose answering:
 *
 * - Runs: the compiler puts `traceEventStart(key, dirty1, dirty2, "pkg.Name (File.kt:N)")` at the start of every
 *   composable body, and it runs only when the body runs, not when Compose skips it. ddp installs a
 *   [CompositionTracer] for the window and counts those calls. A tracer the app already had (Perfetto's
 *   runtime-tracing) gets every call too and is put back after.
 * - Invalidations: [RecompositionCauses], from Compose's composition observer. Compose 1.9 replaced that API, so
 *   there the runs are counted and the answer says the causes were not read.
 *
 * Never says a run was unnecessary: it reports counts and causes only.
 */
@OptIn(InternalComposeTracingApi::class)
internal object Recompositions {
    private const val MAX_MS = 60_000L

    // Main thread only.
    private var active = false
    private var previousTracer: CompositionTracer? = null
    private val runs = HashMap<String, Int>()
    private var causes: RecompositionCauses? = null
    private var causesUnread: String? = null
    private var startedAt = 0L
    // Lazy: unit tests have no main looper.
    private val main by lazy { Handler(Looper.getMainLooper()) }
    // Removed through the same handler that posted it; a no-op when a stop already came.
    private val autoStop = Runnable { if (active) stop() }

    private val tracer = object : CompositionTracer {
        override fun isTraceInProgress() = true

        override fun traceEventStart(key: Int, dirty1: Int, dirty2: Int, info: String) {
            runs[info] = (runs[info] ?: 0) + 1
            causes?.bodyStarted(info)
            previousTracer?.takeIf { it.isTraceInProgress() }?.traceEventStart(key, dirty1, dirty2, info)
        }

        override fun traceEventEnd() {
            previousTracer?.takeIf { it.isTraceInProgress() }?.traceEventEnd()
        }
    }

    /** What stays when the field cannot be written: a tracer that is never in progress costs one call per body. */
    private val idle = object : CompositionTracer {
        override fun isTraceInProgress() = false
        override fun traceEventStart(key: Int, dirty1: Int, dirty2: Int, info: String) {}
        override fun traceEventEnd() {}
    }

    private fun tracerField() = Class.forName("androidx.compose.runtime.ComposerKt")
        .getDeclaredField("compositionTracer").apply { isAccessible = true }

    fun start(params: JSONObject): JSONObject = onMain {
        if (active) stop()
        val ms = params.optLong("ms", 5000).coerceIn(100, MAX_MS)
        previousTracer = runCatching { tracerField().get(null) as CompositionTracer? }
            .onSuccess { Hooks.ok("compose.tracer") }.getOrElse { Hooks.failed("compose.tracer", it.toString()); null }
        val compositions = rootCompositions()
        if (compositions.isEmpty()) throw IllegalStateException("No Compose screen in front")
        // The observer API is experimental and Compose 1.9 replaced it: a missing class or method must cost the
        // causes, never the app.
        causesUnread = null
        causes = try {
            RecompositionCauses(compositions).also { Hooks.ok("compose.compositionObserver") }
        } catch (missing: LinkageError) {
            Hooks.failed("compose.compositionObserver", missing.toString())
            causesUnread = "this app's Compose runtime has no composition observer ddp can read (Compose 1.9 replaced it)"
            null
        }
        Composer.setTracer(tracer)
        active = true
        startedAt = System.nanoTime()
        // A client that never says stop must not leave the tracer in the app.
        main.postDelayed(autoStop, ms + 5_000)
        JSONObject().put("compositions", compositions.size)
    }

    fun stop(): JSONObject = onMain {
        if (!active) throw IllegalStateException("Not measuring: start first")
        main.removeCallbacks(autoStop)
        // setTracer takes no null: "no tracer" goes back through the field it was read from.
        runCatching { tracerField().set(null, previousTracer) }.getOrElse { Composer.setTracer(previousTracer ?: idle) }
        val byCause = causes?.stop().orEmpty()
        active = false
        val result = JSONObject().put("ms", (System.nanoTime() - startedAt) / 1_000_000)
            .put("composables", JSONArray(runs.entries.sortedByDescending { it.value }.map { (info, count) ->
                JSONObject().put("name", info).put("runs", count).apply {
                    byCause[info]?.let { counts ->
                        put("invalidatedBy", JSONArray(counts.entries.sortedByDescending { it.value }.map { (cause, times) ->
                            JSONObject().put("state", cause).put("times", times)
                        }))
                    }
                }
            }))
            .apply { causesUnread?.let { put("causesUnread", it) } }
        runs.clear()
        causes = null
        causesUnread = null
        previousTracer = null
        result
    }

    /** The composition behind each Compose view on screen: WrappedComposition keeps it as `original`. */
    private fun rootCompositions(): List<Composition> {
        val root = FrameRecorder.resumedActivity?.window?.decorView ?: return emptyList()
        return composeViews(root).mapNotNull { view ->
            val wrapped = view.getTag(R.id.wrapped_composition_tag) ?: return@mapNotNull null
            runCatching { wrapped.javaClass.getMethod("getOriginal").invoke(wrapped) as Composition }
                .onSuccess { Hooks.ok("compose.wrappedComposition") }
                .getOrElse { Hooks.failed("compose.wrappedComposition", it.toString()); null }
        }
    }

    private fun composeViews(view: View): List<View> = when (view) {
        is ViewRootForTest -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { composeViews(view.getChildAt(it)) }
        else -> emptyList()
    }
}

/**
 * Which state objects made Compose run each composable again, for one window. A [CompositionObserver] on each root
 * composition (child compositions such as LazyColumn items share it) hands, at the start of each recomposition, the
 * scopes that will run again and the state objects that invalidated each. A [RecomposeScopeObserver] tells when a
 * scope starts, and the first composable body that starts right after it is that scope's composable.
 *
 * Its own class so that the observer types load only when it is built: on a Compose without them (1.9 and later)
 * building it throws a [LinkageError], which [Recompositions] catches.
 */
@OptIn(ExperimentalComposeRuntimeApi::class)
internal class RecompositionCauses(compositions: List<Composition>) {
    // Main thread only.
    private val causes = HashMap<String, HashMap<String, Int>>()
    private val scopeNames = IdentityHashMap<RecomposeScope, String>()
    private val scopeCauses = IdentityHashMap<RecomposeScope, MutableList<String>>()
    private val handles = mutableListOf<CompositionObserverHandle>()
    private var startingScope: RecomposeScope? = null

    private val scopeObserver = object : RecomposeScopeObserver {
        override fun onBeginScopeComposition(scope: RecomposeScope) {
            startingScope = scope
        }
        override fun onEndScopeComposition(scope: RecomposeScope) {}
        override fun onScopeDisposed(scope: RecomposeScope) {}
    }

    private val compositionObserver = object : CompositionObserver {
        override fun onBeginComposition(composition: Composition, invalidationMap: Map<RecomposeScope, Set<Any>?>) {
            invalidationMap.forEach { (scope, states) ->
                val named = scopeNames[scope]
                val described = states.orEmpty().map(::describe).ifEmpty { listOf("invalidated directly") }
                if (named != null) described.forEach { count(named, it) }
                else {
                    scopeCauses.getOrPut(scope) { mutableListOf() } += described
                    scope.observe(scopeObserver)?.let(handles::add)
                }
            }
        }
        override fun onEndComposition(composition: Composition) {}
    }

    init {
        compositions.forEach { composition -> composition.observe(compositionObserver)?.let(handles::add) }
    }

    /** A composable body started: the scope that started just before it is that composable's. */
    fun bodyStarted(info: String) {
        startingScope?.let { scope ->
            scopeNames[scope] = info
            scopeCauses.remove(scope)?.forEach { cause -> count(info, cause) }
        }
        startingScope = null
    }

    /** Removes the observers; the causes by composable name. */
    fun stop(): Map<String, Map<String, Int>> {
        handles.forEach { it.dispose() }
        handles.clear()
        return causes
    }

    private fun count(info: String, cause: String) {
        val byCause = causes.getOrPut(info) { HashMap() }
        byCause[cause] = (byCause[cause] ?: 0) + 1
    }

    /** A state object by its kind and the type of its value, never the value itself (it can hold user data). */
    private fun describe(state: Any): String {
        val kind = when (state) {
            is MutableState<*> -> "MutableState"
            is State<*> -> "State"
            is RecomposeScope -> return "invalidated directly"
            else -> return state.javaClass.simpleName.ifEmpty { "state" }
        }
        val type = runCatching { state.value?.javaClass?.simpleName }.getOrNull() ?: "null"
        return "$kind<$type>"
    }
}
