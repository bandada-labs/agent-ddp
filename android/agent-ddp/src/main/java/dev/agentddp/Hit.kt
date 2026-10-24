package dev.agentddp

import android.graphics.PointF
import android.graphics.RectF
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.PointerEvent
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.node.PointerInputModifierNode
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import org.json.JSONArray
import org.json.JSONObject

/**
 * Where a touch on an element would land, found the way the system routes a touch, without touching. Views: the
 * topmost child under the point that takes touches, as ViewGroup dispatch picks it (a parent only passes on points
 * inside its own bounds). Compose: `LayoutNode.hitTest`, the call Compose makes for each touch (internal, by
 * reflection), which also widens small controls to the 48 dp minimum touch target.
 *
 * A touch that reaches the element can still be taken on its way down, before the element sees it: a parent's
 * `onInterceptTouchEvent`, or a Compose parent that consumes the press in `PointerEventPass.Initial`. Each parent is
 * asked with a press and then a cancel, the first half of what dispatch would send, so nothing is clicked.
 */
// ponytail: a parent that takes only the release, or only after a move, is not seen; `ddp tap` notices it after the
// touch, as "nothing changed".
internal object Hit {
    /** Main thread only. Every tappable element, or the one named by id or label. Points in frame pixels. */
    fun check(name: String?): JSONArray {
        val capture = FrameCapture.capture()
        val chosen = (0 until capture.elements.length()).filter { index ->
            val element = capture.elements.getJSONObject(index)
            if (name == null) element.optBoolean("tap") else element.optString("id") == name || element.optString("label") == name
        }
        if (name != null && chosen.isEmpty()) throw IllegalArgumentException("No element '$name'")
        return JSONArray(chosen.map { check(capture.elements.getJSONObject(it), capture.targets[it]) })
    }

    private sealed interface Taker {
        data object Nothing : Taker
        class OfView(val view: View) : Taker
        /** Compose's hit path, outermost first: the pointer input nodes and their layout nodes. */
        class OfCompose(val composeView: View, val nodes: List<Any>, val path: List<LayoutInfo>) : Taker
    }

    private fun check(element: JSONObject, target: Target): JSONObject {
        val frame = element.getJSONArray("frame").let { RectF(it.f(0), it.f(1), it.f(0) + it.f(2), it.f(1) + it.f(3)) }
        // The center first, then a 3 by 3 grid, so a partly covered element still gets a point that reaches it.
        val fractions = listOf(0.5f, 1f / 6, 5f / 6)
        val points = fractions.flatMap { y -> fractions.map { x -> PointF(frame.left + frame.width() * x, frame.top + frame.height() * y) } }
        val reaching = points.filter { reaches(target, taker(it)) }
        // A parent that takes the touch on its way takes it at every point of the element.
        val thief = reaching.firstOrNull()?.let { parentTaking(target, it) }
        val taking = if (thief == null) reaching else emptyList()
        val result = JSONObject(element.toString()).put("points", taking.size)
        taking.firstOrNull()?.let { result.put("point", JSONArray(listOf(round2(it.x.toDouble()), round2(it.y.toDouble())))) }
        when {
            element.optBoolean("disabled") -> result.put("reason", "disabled")
            thief != null -> result.put("reason", "intercepted").put("by", thief)
            reaching.firstOrNull() != points[0] -> {
                val (reason, by) = blocker(target, points[0])
                result.put("reason", reason)
                by?.let { result.put("by", it) }
            }
        }
        return result
    }

    private fun reaches(target: Target, taker: Taker): Boolean = when (taker) {
        Taker.Nothing -> false
        is Taker.OfView -> target.layout == null && generateSequence(taker.view) { it.parent as? View }.any { it === target.view }
        is Taker.OfCompose -> taker.composeView === target.view && target.layout != null &&
            taker.path.any { node -> generateSequence(node) { it.parentInfo }.any { it === target.layout } }
    }

    /** Who takes a touch at [point] (frame pixels): the topmost window under it, then down its views. */
    private fun taker(point: PointF): Taker {
        val decor = FrameRecorder.resumedActivity?.window?.decorView ?: return Taker.Nothing
        val origin = IntArray(2).also(decor::getLocationOnScreen)
        val screen = PointF(point.x + origin[0], point.y + origin[1])
        val root = FrameCapture.windowRoots(decor).lastOrNull { it.isShown && screenRect(it).contains(screen.x, screen.y) }
            ?: return Taker.Nothing
        return viewTaker(root, screen) ?: Taker.Nothing
    }

    private fun viewTaker(view: View, screen: PointF): Taker? {
        if (view.visibility != View.VISIBLE || !screenRect(view).contains(screen.x, screen.y)) return null
        if (view is ViewRootForTest) {
            val (nodes, path) = composePath(view, screen) ?: return null
            // An AndroidView's layout node passes the touch on to its View (pointerInteropFilter).
            interopHolder(view, path, screen)?.let { holder -> return viewTaker(holder, screen) ?: Taker.OfCompose(view, nodes, path) }
            return Taker.OfCompose(view, nodes, path)
        }
        if (view is ViewGroup) {
            drawingOrder(view).asReversed().forEach { child -> viewTaker(child, screen)?.let { return it } }
        }
        val takesTouches = view.isClickable || view.isLongClickable || view.isContextClickable ||
            Hidden.touchListener(view) != null
        return if (takesTouches) Taker.OfView(view) else null
    }

    /** Compose's own hit test at [screen], as pointer input nodes and their layout nodes; null when none takes it. */
    private fun composePath(composeView: View, screen: PointF): Pair<List<Any>, List<LayoutInfo>>? {
        val hit = Hidden.composeHitTest(composeView) ?: return null
        val origin = IntArray(2).also(composeView::getLocationOnScreen)
        val x = screen.x - origin[0]
        val y = screen.y - origin[1]
        // An Offset is its two floats packed into one Long.
        val packed = (x.toRawBits().toLong() shl 32) or (y.toRawBits().toLong() and 0xFFFFFFFFL)
        return runCatching {
            val result = hit.newResult.newInstance()
            hit.hitTest.invoke(hit.getRoot.invoke(composeView), packed, result, true, true)
            val nodes = (result as List<*>).filterNotNull()
            nodes to nodes.map { node -> hit.requireLayoutNode.invoke(null, node) as LayoutInfo }
        }.getOrNull()?.takeIf { it.first.isNotEmpty() }
    }

    /** The AndroidView holder whose layout node is in the hit path. Holders sit in the compose view's AndroidViewsHandler. */
    private fun interopHolder(composeView: View, path: List<LayoutInfo>, screen: PointF): View? {
        val handler = (composeView as? ViewGroup)?.children()?.firstOrNull { "AndroidViewsHandler" in it.javaClass.name } as? ViewGroup
            ?: return null
        val deepest = path.last()
        return handler.children().lastOrNull { holder ->
            screenRect(holder).contains(screen.x, screen.y) && Hidden.interopLayout(holder)?.let { layout ->
                generateSequence(deepest) { it.parentInfo }.any { it === layout }
            } == true
        }
    }

    /** A parent that takes a touch at [point] (frame pixels) before the element sees it, described; null when none does. */
    private fun parentTaking(target: Target, point: PointF): String? {
        val decor = FrameRecorder.resumedActivity?.window?.decorView ?: return null
        val origin = IntArray(2).also(decor::getLocationOnScreen)
        val screen = PointF(point.x + origin[0], point.y + origin[1])
        interceptingParent(target.view, screen)?.let { return "${described(it)} (onInterceptTouchEvent)" }
        val taker = taker(point) as? Taker.OfCompose ?: return null
        return initialPassParent(target, taker, screen)?.let { "${kind(it)} (PointerEventPass.Initial)" }
    }

    /** The outermost parent of [view] whose `onInterceptTouchEvent` keeps a press at [screen], as dispatch asks it. */
    private fun interceptingParent(view: View, screen: PointF): ViewGroup? {
        val parents = generateSequence(view.parent as? ViewGroup) { it.parent as? ViewGroup }.toList().asReversed()
        val asked = mutableListOf<ViewGroup>()
        val now = SystemClock.uptimeMillis()
        fun event(action: Int, parent: ViewGroup): MotionEvent {
            val origin = IntArray(2).also(parent::getLocationOnScreen)
            return MotionEvent.obtain(now, now, action, screen.x - origin[0], screen.y - origin[1], 0)
                .apply { source = InputDevice.SOURCE_TOUCHSCREEN }
        }
        try {
            return parents.firstOrNull { parent ->
                asked += parent
                val press = event(MotionEvent.ACTION_DOWN, parent)
                parent.onInterceptTouchEvent(press).also { press.recycle() }
            }
        } finally {
            asked.forEach { parent -> event(MotionEvent.ACTION_CANCEL, parent).also { parent.onInterceptTouchEvent(it) }.recycle() }
        }
    }

    /** The outermost pointer input node, outside the element, that consumes a press at [screen] in the Initial pass. */
    private fun initialPassParent(target: Target, taker: Taker.OfCompose, screen: PointF): PointerInputModifierNode? {
        val layout = target.layout ?: return null
        val parents = taker.nodes.zip(taker.path).filter { (node, path) ->
            node is PointerInputModifierNode && generateSequence(path) { it.parentInfo }.none { it === layout }
        }
        if (parents.isEmpty()) return null
        val origin = IntArray(2).also(taker.composeView::getLocationOnScreen)
        val inComposeView = Offset(screen.x - origin[0], screen.y - origin[1])
        val root = generateSequence(layout) { it.parentInfo }.last().coordinates
        val now = SystemClock.uptimeMillis()
        val asked = mutableListOf<PointerInputModifierNode>()
        try {
            return parents.firstOrNull { (node, path) ->
                node as PointerInputModifierNode
                asked += node
                val position = path.coordinates.localPositionOf(root, inComposeView)
                val press = PointerInputChange(PointerId(0), now, position, true, now, position, false, false)
                node.onPointerEvent(PointerEvent(listOf(press)), PointerEventPass.Initial, path.coordinates.size)
                press.isConsumed
            }?.first as PointerInputModifierNode?
        } finally {
            asked.forEach { it.onCancelPointerInput() }
        }
    }

    /** What kind of pointer input a node is, from its class: `pointerInput`, `clickable`, `scrollable`. */
    private fun kind(node: Any): String {
        val name = node.javaClass.simpleName
        return listOf("PointerInput" to "pointerInput", "Clickable" to "clickable", "Scrollable" to "scrollable", "Draggable" to "draggable")
            .firstOrNull { it.first in name }?.second ?: name
    }

    /** Why the touch at the element's center misses it, and what takes it instead. */
    private fun blocker(target: Target, point: PointF): Pair<String, String?> {
        if (target.layout == null) {
            val decor = FrameRecorder.resumedActivity?.window?.decorView
            val origin = IntArray(2).also { decor?.getLocationOnScreen(it) }
            val screen = PointF(point.x + origin[0], point.y + origin[1])
            generateSequence(target.view) { it.parent as? View }.forEach { ancestor ->
                if (ancestor.visibility != View.VISIBLE) return "untouchable" to "${ancestor.javaClass.simpleName} not visible"
                if (!screenRect(ancestor).contains(screen.x, screen.y)) return "outside" to "${ancestor.javaClass.simpleName} bounds"
            }
        }
        return when (val taker = taker(point)) {
            Taker.Nothing -> "untouchable" to null
            is Taker.OfView -> "covered" to described(taker.view)
            is Taker.OfCompose -> "covered" to described(taker)
        }
    }

    private fun described(view: View): String {
        val id = view.id.takeIf { it != View.NO_ID }?.let { runCatching { view.resources.getResourceEntryName(it) }.getOrNull() }
        // An anonymous subclass has no simple name; its parent class says what it is.
        return id ?: view.javaClass.simpleName.ifEmpty { view.javaClass.superclass.simpleName }
    }

    /** The deepest named node in the hit path (its test tag or text), or what kind of pointer input took the touch. */
    private fun described(taker: Taker.OfCompose): String {
        val semantics = mutableMapOf<Int, SemanticsNode>()
        fun collect(node: SemanticsNode) {
            semantics[node.id] = node
            node.children.forEach(::collect)
        }
        collect((taker.composeView as ViewRootForTest).semanticsOwner.unmergedRootSemanticsNode)
        val deepest = taker.path.last()
        generateSequence(deepest) { it.parentInfo }.forEach { layout ->
            val node = semantics[layout.semanticsId] ?: return@forEach
            node.config.getOrNull(SemanticsProperties.TestTag)?.let { return it }
            node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }?.let { return it }
        }
        val kind = deepest.getModifierInfo().map { it.modifier.javaClass.simpleName }
            .firstOrNull { "PointerInput" in it || "Clickable" in it || "Scrollable" in it || "Draggable" in it }
        return when {
            kind == null -> "a Compose node"
            "PointerInput" in kind -> "pointerInput"
            "Clickable" in kind -> "clickable"
            else -> kind.removeSuffix("Element").replaceFirstChar { it.lowercase() }
        }
    }

    private fun ViewGroup.children(): List<View> = (0 until childCount).map(::getChildAt)

    private fun screenRect(view: View): RectF {
        val origin = IntArray(2).also(view::getLocationOnScreen)
        return RectF(origin[0].toFloat(), origin[1].toFloat(), (origin[0] + view.width).toFloat(), (origin[1] + view.height).toFloat())
    }

    private fun JSONArray.f(index: Int) = getDouble(index).toFloat()
}
