package dev.agentddp

import org.json.JSONArray
import org.json.JSONObject

/**
 * Every engine piece the library reaches outside public API, how fragile that path is, and how it behaved in this
 * run, for `app.hooks` and `ddp doctor`. A hook that stops working on a new Android or Compose shows here; the checks
 * that depend on it go quiet rather than guess.
 *
 * `hidden`: an Android SDK member marked @hide, reached by reflection; Android blocks some per target SDK, then a
 * fallback (the platform's documented rule) answers and is counted. `internal`: Compose internals, found by name;
 * they can change with any Compose release.
 */
internal object Hooks {
    class Hook(val name: String, val kind: String, val feeds: String)

    val all = listOf(
        Hook("windowManager.roots", "hidden", "popups, menus and dialogs (WindowManagerGlobal)"),
        Hook("gradientDrawable.stroke", "hidden", "View borders (GradientDrawable.mStrokePaint)"),
        Hook("view.touchListener", "hidden", "whether app code decides a touch (View.mListenerInfo)"),
        Hook("decorView.window", "hidden", "whether the Activity decides a touch (DecorView.mWindow)"),
        Hook("viewGroup.touchPoint", "hidden", "tap, check tappable on Views (isTransformedTouchPointInView)"),
        Hook("viewGroup.touchOrder", "hidden", "the order Views are asked (buildTouchDispatchChildList)"),
        Hook("view.canReceivePointerEvents", "hidden", "which Views can get a touch"),
        Hook("compose.hitTest", "internal", "tap, check tappable, touch size on Compose (LayoutNode.hitTest)"),
        Hook("compose.zSortedChildren", "internal", "Compose draw order (LayoutNode.getZSortedChildren)"),
        Hook("compose.interopLayout", "internal", "AndroidView inside Compose (AndroidViewHolder.getLayoutNode)"),
        Hook("compose.textColor", "internal", "whether BasicText paints another color (its color producer)"),
    )

    private class Count(var ok: Int = 0, var failed: Int = 0, var fallback: Int = 0, var why: String? = null)

    private val counts = HashMap<String, Count>()

    fun ok(name: String) = update(name) { it.ok++ }

    fun failed(name: String, why: String) = update(name) { it.failed++; it.why = why }

    /** The hook is not there, and the platform's documented rule answered instead. */
    fun fallback(name: String, why: String) = update(name) { it.fallback++; it.why = why }

    /** One object per hook: `ok`, `fallback`, `failing` (never answered), `mixed`, or `unused` in this run. */
    fun report(): JSONArray = synchronized(counts) {
        JSONArray(all.map { hook ->
            val count = counts[hook.name] ?: Count()
            val status = when {
                count.ok + count.failed + count.fallback == 0 -> "unused"
                count.failed == 0 && count.fallback == 0 -> "ok"
                count.failed == 0 -> "fallback"
                count.ok + count.fallback == 0 -> "failing"
                else -> "mixed"
            }
            JSONObject().put("name", hook.name).put("kind", hook.kind).put("feeds", hook.feeds).put("status", status)
                .put("ok", count.ok).put("failed", count.failed).put("fallback", count.fallback)
                .apply { count.why?.let { put("error", it) } }
        })
    }

    private fun update(name: String, change: (Count) -> Unit) = synchronized(counts) { change(counts.getOrPut(name) { Count() }) }
}
