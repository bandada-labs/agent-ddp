package dev.agentddp

import android.graphics.PointF
import android.graphics.RectF
import android.os.Build
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowManager
import android.widget.TextView
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
 * Where a touch on an element would land, found the way the system routes a touch, without touching. Views: the walk
 * ViewGroup dispatch makes, with its own methods: the children in touch order, `canReceivePointerEvents` and
 * `isTransformedTouchPointInView` (hidden, by reflection), so Z, drawing order, transforms and scroll are the
 * platform's. Compose: `LayoutNode.hitTest`, the call Compose makes for each touch (internal, by reflection), which also
 * widens small controls to the 48 dp minimum touch target.
 *
 * A View on top takes the touch for sure only when no app code decides it: no touch listener, no `TouchDelegate`, and
 * no `onTouchEvent` or `dispatchTouchEvent` of its own class. Then View's contract answers (a clickable view takes it).
 * Otherwise the point is unknown and counts as reaching: nothing is reported that the engine did not decide.
 *
 * A touch that reaches the element can still be taken on its way down, before the element sees it: a parent's
 * `onInterceptTouchEvent`, or a Compose parent that consumes the press in `PointerEventPass.Initial`. Each parent is
 * asked with a press and then a cancel, the first half of what dispatch would send, so nothing is clicked.
 */
// ponytail: a parent that takes only the release, or only after a move, is not seen; `ddp tap` notices it after the
// touch, as "nothing changed". A window with a custom touchable region is taken as touchable all over.
internal object Hit {
    /**
     * Main thread only. Every tappable element, or the one named by id or label. Points in frame pixels. With
     * [minimumTouch] (px), a control the touch reaches over less than that, across or down, also gets `reach`.
     */
    fun check(name: String?, minimumTouch: Float = 0f): JSONArray {
        val capture = FrameCapture.capture()
        val chosen = (0 until capture.elements.length()).filter { index ->
            val element = capture.elements.getJSONObject(index)
            if (name == null) element.optBoolean("tap") else element.optString("id") == name || element.optString("label") == name
        }
        if (name != null && chosen.isEmpty()) throw IllegalArgumentException("No element '$name'")
        val keyboard = keyboardFrame()
        return JSONArray(chosen.map { check(capture.elements.getJSONObject(it), capture.targets[it], minimumTouch, keyboard) })
    }

    /**
     * Where the keyboard is, in the activity window's px, as the platform says it (the IME insets, API 30 and up). The
     * keyboard is another window, so the app's hit test does not see it. Null when it is hidden or the API cannot say.
     */
    private fun keyboardFrame(): RectF? {
        if (Build.VERSION.SDK_INT < 30) return null
        val root = FrameRecorder.resumedActivity?.window?.decorView ?: return null
        val insets = root.rootWindowInsets ?: return null
        if (!insets.isVisible(WindowInsets.Type.ime())) return null
        val height = insets.getInsets(WindowInsets.Type.ime()).bottom.takeIf { it > 0 } ?: return null
        return RectF(0f, (root.height - height).toFloat(), root.width.toFloat(), root.height.toFloat())
    }

    private sealed interface Taker {
        data object Nothing : Taker
        /** App code decides whether it takes the touch: a touch listener, a TouchDelegate, its own touch methods. */
        data object Unknown : Taker
        class OfView(val view: View) : Taker
        /** Compose's hit path, outermost first: the pointer input nodes and their layout nodes. */
        class OfCompose(val composeView: View, val nodes: List<Any>, val path: List<LayoutInfo>) : Taker
    }

    private fun check(element: JSONObject, target: Target, minimumTouch: Float, keyboard: RectF?): JSONObject {
        val frame = element.getJSONArray("frame").let { RectF(it.f(0), it.f(1), it.f(0) + it.f(2), it.f(1) + it.f(3)) }
        // The center first, then a 3 by 3 grid, so a partly covered element still gets a point that reaches it.
        val fractions = listOf(0.5f, 1f / 6, 5f / 6)
        val points = fractions.flatMap { y -> fractions.map { x -> PointF(frame.left + frame.width() * x, frame.top + frame.height() * y) } }
        val reaching = points.filter { !blocked(target, taker(it)) }
        // A parent that takes the touch on its way takes it at every point of the element.
        val thief = reaching.firstOrNull()?.let { parentTaking(target, it) }
        val taking = if (thief == null) reaching else emptyList()
        val result = JSONObject(element.toString()).put("points", taking.size)
        // Part of it past the window's edge (a row half scrolled off) is cut by the edge, not small.
        val window = FrameRecorder.resumedActivity?.window?.decorView?.let { RectF(0f, 0f, it.width.toFloat(), it.height.toFloat()) }
        if (minimumTouch > 0 && window?.contains(frame) == true) touchSize(target, frame, minimumTouch)?.let { (width, height) ->
            result.put("reach", JSONArray(listOf(round2(width.toDouble()), round2(height.toDouble()))))
        }
        taking.firstOrNull()?.let { result.put("point", JSONArray(listOf(round2(it.x.toDouble()), round2(it.y.toDouble())))) }
        when {
            element.optBoolean("disabled") -> result.put("reason", "disabled")
            // Only all of it under the keyboard: a control partly under still has points a finger reaches.
            keyboard != null && keyboard.contains(frame) -> {
                result.put("points", 0).remove("point")
                result.put("reason", "covered").put("by", "the keyboard")
            }
            thief != null -> result.put("reason", "intercepted").put("by", thief)
            reaching.firstOrNull() != points[0] -> {
                val (reason, by) = blocker(target, points[0])
                result.put("reason", reason)
                by?.let { result.put("by", it) }
            }
        }
        return result
    }

    /** Whether the engine says a touch does not reach the target. Unknown is not blocked. */
    private fun blocked(target: Target, taker: Taker): Boolean = when (taker) {
        Taker.Nothing -> true
        Taker.Unknown -> false
        is Taker.OfView -> target.layout != null || generateSequence(taker.view) { it.parent as? View }.none { it === target.view }
        is Taker.OfCompose -> taker.composeView !== target.view || target.layout == null ||
            taker.path.none { node -> generateSequence(node) { it.parentInfo }.any { it === target.layout } }
    }

    /**
     * How far the touch reaches the control across and down through its center, in px, when that is under
     * [minimum]: from the center out to the first point the engine says it does not reach. Null when it reaches
     * [minimum] both ways, or when the engine cannot say.
     */
    private fun touchSize(target: Target, frame: RectF, minimum: Float): Pair<Float, Float>? {
        val center = PointF(frame.centerX(), frame.centerY())
        fun reach(dx: Float, dy: Float): Float {
            val far = minimum / 2 - 0.5f
            if (!blocked(target, taker(PointF(center.x + dx * far, center.y + dy * far)))) return far
            // One px steps back toward the center: the first point that is not blocked is the edge.
            var distance = far - 1
            while (distance > 0 && blocked(target, taker(PointF(center.x + dx * distance, center.y + dy * distance)))) distance -= 1
            return distance.coerceAtLeast(0f)
        }
        val width = reach(-1f, 0f) + reach(1f, 0f)
        val height = reach(0f, -1f) + reach(0f, 1f)
        return (width to height).takeIf { width < minimum - 1 || height < minimum - 1 }
    }

    /** Who takes a touch at [point] (frame pixels): the topmost touchable window under it, then down its views. */
    private fun taker(point: PointF): Taker {
        val decor = FrameRecorder.resumedActivity?.window?.decorView ?: return Taker.Nothing
        val origin = FrameCapture.windowOrigin(decor)
        val screen = PointF(point.x + origin[0], point.y + origin[1])
        val root = FrameCapture.windowRoots(decor).lastOrNull { root ->
            val flags = (root.layoutParams as? WindowManager.LayoutParams)?.flags ?: 0
            root.isShown && flags and WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE == 0 && screenRect(root).contains(screen.x, screen.y)
        } ?: return Taker.Nothing
        if (Dispatch.windowCallbackDecides(root)) return Taker.Unknown
        val rootOrigin = IntArray(2).also(root::getLocationOnScreen)
        return viewTaker(root, PointF(screen.x - rootOrigin[0], screen.y - rootOrigin[1]), screen) ?: Taker.Nothing
    }

    /**
     * What takes a touch at [local] (the view's own coordinates) inside [view], which the walk found under it: a
     * child first, topmost first, as `ViewGroup.dispatchTouchEvent` asks them, then the view itself. Null: nothing.
     */
    private fun viewTaker(view: View, local: PointF, screen: PointF): Taker? {
        if (view is ViewRootForTest) {
            val (nodes, path) = when (val hit = composePath(view, screen)) {
                ComposeHit.Unavailable -> return Taker.Unknown
                ComposeHit.Empty -> return null
                is ComposeHit.Path -> hit.nodes to hit.layouts
            }
            // An AndroidView's layout node passes the touch on to its View (pointerInteropFilter).
            interopHolder(view, path, screen)?.let { holder ->
                val holderOrigin = IntArray(2).also(holder::getLocationOnScreen)
                val holderLocal = PointF(screen.x - holderOrigin[0], screen.y - holderOrigin[1])
                return children(holder, holderLocal, screen) ?: Taker.OfCompose(view, nodes, path)
            }
            return Taker.OfCompose(view, nodes, path)
        }
        if (view is ViewGroup) {
            if (Dispatch.decidesOwnDispatch(view)) return Taker.Unknown
            children(view, local, screen)?.let { return it }
        }
        return when {
            !Dispatch.noAppTouchCode(view) -> Taker.Unknown
            // View.onTouchEvent takes a touch when the view is clickable in any way, enabled or not.
            view.isClickable || view.isLongClickable || view.isContextClickable || view.tooltipText != null -> Taker.OfView(view)
            else -> null
        }
    }

    private fun children(group: ViewGroup, local: PointF, screen: PointF): Taker? {
        val order = Dispatch.touchOrder(group) ?: return Taker.Unknown
        for (child in order.asReversed()) {
            if (!Dispatch.canReceivePointerEvents(child)) continue
            val childLocal = Dispatch.pointInChild(group, local, child) ?: continue
            viewTaker(child, childLocal, screen)?.let { return it }
        }
        return null
    }

    private sealed interface ComposeHit {
        class Path(val nodes: List<Any>, val layouts: List<LayoutInfo>) : ComposeHit
        data object Empty : ComposeHit
        /** The internal hit test was not found or failed: the walk cannot say. */
        data object Unavailable : ComposeHit
    }

    /** Compose's own hit test at [screen], as pointer input nodes and their layout nodes. */
    private fun composePath(composeView: View, screen: PointF): ComposeHit {
        val hit = Hidden.composeHitTest(composeView) ?: return ComposeHit.Unavailable
            .also { Hooks.failed("compose.hitTest", "no LayoutNode.hitTest in this Compose version") }
        val origin = IntArray(2).also(composeView::getLocationOnScreen)
        val x = screen.x - origin[0]
        val y = screen.y - origin[1]
        // An Offset is its two floats packed into one Long.
        val packed = (x.toRawBits().toLong() shl 32) or (y.toRawBits().toLong() and 0xFFFFFFFFL)
        return runCatching {
            val result = hit.newResult.newInstance()
            hit.hitTest.invoke(hit.getRoot.invoke(composeView), packed, result, true, true)
            val nodes = (result as List<*>).filterNotNull()
            val layouts = nodes.map { node -> hit.requireLayoutNode.invoke(null, node) as LayoutInfo }
            Hooks.ok("compose.hitTest")
            if (nodes.isEmpty()) ComposeHit.Empty else ComposeHit.Path(nodes, layouts)
        }.getOrElse { error ->
            Hooks.failed("compose.hitTest", error.toString())
            ComposeHit.Unavailable
        }
    }

    /** The AndroidView holder whose layout node is in the hit path. Holders sit in the compose view's AndroidViewsHandler. */
    private fun interopHolder(composeView: View, path: List<LayoutInfo>, screen: PointF): ViewGroup? {
        val handler = (composeView as? ViewGroup)?.children()?.firstOrNull { "AndroidViewsHandler" in it.javaClass.name } as? ViewGroup
            ?: return null
        val deepest = path.last()
        return handler.children().filterIsInstance<ViewGroup>().lastOrNull { holder ->
            screenRect(holder).contains(screen.x, screen.y) && Hidden.interopLayout(holder).also {
                if (it == null) Hooks.failed("compose.interopLayout", "no AndroidViewHolder.getLayoutNode") else Hooks.ok("compose.interopLayout")
            }?.let { layout ->
                generateSequence(deepest) { it.parentInfo }.any { it === layout }
            } == true
        }
    }

    /** A parent that takes a touch at [point] (frame pixels) before the element sees it, described; null when none does. */
    private fun parentTaking(target: Target, point: PointF): String? {
        val decor = FrameRecorder.resumedActivity?.window?.decorView ?: return null
        val origin = FrameCapture.windowOrigin(decor)
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
            val origin = decor?.let(FrameCapture::windowOrigin) ?: IntArray(2)
            val screen = PointF(point.x + origin[0], point.y + origin[1])
            generateSequence(target.view) { it.parent as? View }.forEach { ancestor ->
                if (ancestor.visibility != View.VISIBLE) return "untouchable" to "${ancestor.javaClass.simpleName} not visible"
                if (!screenRect(ancestor).contains(screen.x, screen.y)) return "outside" to "${ancestor.javaClass.simpleName} bounds"
            }
        }
        return when (val taker = taker(point)) {
            // Unknown never gets here: a center the engine cannot decide counts as reaching.
            Taker.Nothing, Taker.Unknown -> "untouchable" to null
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

/**
 * The pieces of ViewGroup dispatch, the platform's own methods, looked up once (hidden API, by reflection), and what
 * the walk needs to know about app code. When a piece is missing on a platform version, the walk says unknown.
 */
private object Dispatch {
    private const val DECOR_VIEW = "com.android.internal.policy.DecorView"
    private val touchDispatchChildList by lazy { hidden(ViewGroup::class.java, "buildTouchDispatchChildList") }
    private val childrenDrawingOrderEnabled by lazy { hidden(ViewGroup::class.java, "isChildrenDrawingOrderEnabled") }
    private val transformedTouchPointInView by lazy {
        hidden(ViewGroup::class.java, "isTransformedTouchPointInView", Float::class.java, Float::class.java, View::class.java, PointF::class.java)
    }
    private val canReceivePointerEventsMethod by lazy { hidden(View::class.java, "canReceivePointerEvents") }
    private val decorWindow by lazy {
        runCatching { Class.forName("com.android.internal.policy.DecorView").getDeclaredField("mWindow").apply { isAccessible = true } }.getOrNull()
    }
    private val ownDispatch = HashMap<Class<*>, Boolean>()
    private val ownTouchCode = HashMap<Class<*>, Boolean>()

    private fun hidden(owner: Class<*>, name: String, vararg parameters: Class<*>) =
        runCatching { owner.getDeclaredMethod(name, *parameters).apply { isAccessible = true } }.getOrNull()

    /**
     * The children in the order dispatch asks them, bottom first (it asks them from the end). Null: unknown.
     * `buildTouchDispatchChildList` is blocked from API 29 (max-target-o); without it, its documented rule: drawing
     * order (custom when the group enables it), then by Z, stable.
     */
    fun touchOrder(group: ViewGroup): List<View>? = runCatching {
        if (transformedTouchPointInView == null) return null.also { Hooks.failed("viewGroup.touchPoint", "isTransformedTouchPointInView is blocked") }
        if (touchDispatchChildList == null) Hooks.fallback("viewGroup.touchOrder", "blocked; drawing order, then Z")
        touchDispatchChildList?.let { method ->
            @Suppress("UNCHECKED_CAST")
            val sorted = method.invoke(group) as ArrayList<View>?
            // Null: no child has a Z, so dispatch keeps drawing order. It clears its list after use, so does this.
            Hooks.ok("viewGroup.touchOrder")
            if (sorted != null) return sorted.toList().also { sorted.clear() }
        }
        val custom = childrenDrawingOrderEnabled?.invoke(group) as Boolean? ?: return null
        val drawn = (0 until group.childCount).map { position ->
            when {
                !custom -> group.getChildAt(position)
                Build.VERSION.SDK_INT >= 29 -> group.getChildAt(group.getChildDrawingOrder(position))
                else -> return null
            }
        }
        drawn.sortedBy { it.z }
    }.getOrNull()

    fun canReceivePointerEvents(view: View): Boolean {
        canReceivePointerEventsMethod?.let { method ->
            runCatching { method.invoke(view) as Boolean }.getOrNull()?.let { return it.also { Hooks.ok("view.canReceivePointerEvents") } }
        }
        // The platform's own definition, for a version that blocks the method.
        Hooks.fallback("view.canReceivePointerEvents", "blocked; visible or animating")
        return view.visibility == View.VISIBLE || view.animation != null
    }

    /** [local] in [child]'s coordinates when the point is inside it, as dispatch tests it (transforms and scroll). */
    fun pointInChild(group: ViewGroup, local: PointF, child: View): PointF? {
        val out = PointF()
        val inside = runCatching { transformedTouchPointInView!!.invoke(group, local.x, local.y, child, out) as Boolean }.getOrNull()
        if (inside == null) Hooks.failed("viewGroup.touchPoint", "isTransformedTouchPointInView threw") else Hooks.ok("viewGroup.touchPoint")
        return out.takeIf { inside == true }
    }

    /**
     * A group whose class changes `dispatchTouchEvent`: its children are not asked the way the walk asks them.
     * DecorView's hands the touch to its window callback, which [windowCallbackDecides] asks about.
     */
    fun decidesOwnDispatch(group: ViewGroup): Boolean = ownDispatch.getOrPut(group.javaClass) {
        classesBelow(group.javaClass, ViewGroup::class.java).any { it.name != DECOR_VIEW && declares(it, "dispatchTouchEvent") }
    }

    /**
     * True when only View's contract decides whether [view] takes a touch: no touch listener (or one that cannot be
     * read), no TouchDelegate, no touch method of its own class. TextView's `onTouchEvent` returns true whenever
     * View's does, so a TextView or Button still counts.
     */
    fun noAppTouchCode(view: View): Boolean {
        if (!Hidden.touchListenerReadable) return false.also { Hooks.failed("view.touchListener", "View.mListenerInfo is blocked") }
        Hooks.ok("view.touchListener")
        if (Hidden.touchListener(view) != null || view.touchDelegate != null) return false
        return !ownTouchCode.getOrPut(view.javaClass) {
            classesBelow(view.javaClass, View::class.java).any { type ->
                type != ViewGroup::class.java && declares(type, "dispatchTouchEvent") ||
                    type != TextView::class.java && declares(type, "onTouchEvent")
            }
        }
    }

    /** A window root whose window callback (the Activity or Dialog) changes `dispatchTouchEvent`. */
    fun windowCallbackDecides(root: View): Boolean {
        if (root.javaClass.name != DECOR_VIEW) return root is ViewGroup && decidesOwnDispatch(root)
        val windowCallback = Hidden.windowOf(root)?.callback ?: return true
        // The library's own tap recorder only watches, then hands the touch on.
        val callback = (windowCallback as? TouchWatcher)?.inner ?: windowCallback
        val base = if (callback is android.app.Dialog) android.app.Dialog::class.java else android.app.Activity::class.java
        return classesBelow(callback.javaClass, base).any { declares(it, "dispatchTouchEvent") }
    }

    private fun classesBelow(type: Class<*>, top: Class<*>): Sequence<Class<*>> =
        generateSequence(type) { it.superclass }.takeWhile { it != top && it != Any::class.java }

    private fun declares(type: Class<*>, name: String) =
        runCatching { type.getDeclaredMethod(name, MotionEvent::class.java); true }.getOrDefault(false)
}
