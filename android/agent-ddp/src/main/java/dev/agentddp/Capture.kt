package dev.agentddp

import android.graphics.Paint
import android.graphics.PointF
import android.graphics.RectF
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.isSpecified
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.InspectableValue
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.platform.isDebugInspectorInfoEnabled
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.isSpecified
import org.json.JSONArray
import org.json.JSONObject

/** One frame: elements (what an agent names) and painted nodes in draw order (later covers earlier). */
internal class Capture(private val density: Float) {
    val elements = JSONArray()
    val nodes = JSONArray()
    /** Where the window being walked sits relative to the activity window. */
    var windowOffset = PointF(0f, 0f)
    /** Index in [elements] of the element whose subtree is being walked, -1 for none: each node's `own`. */
    private var owner = -1
    private val composeDensity = Density(density)

    fun walkView(view: View, clip: RectF, parentAlpha: Float) {
        if (view.visibility != View.VISIBLE) return
        val alpha = parentAlpha * view.alpha
        if (alpha <= 0.001f) return
        val location = IntArray(2).also(view::getLocationInWindow)
        val frame = RectF(location[0].toFloat(), location[1].toFloat(),
            (location[0] + view.width).toFloat(), (location[1] + view.height).toFloat())
            .apply { offset(windowOffset.x, windowOffset.y) }
        val visible = RectF(frame).takeIf { it.intersect(clip) } ?: return

        val outerOwner = owner
        viewElement(view, frame)?.let { owner = it }
        val source = view.javaClass.simpleName
        view.background?.let { addNode(drawableNode(it).put("source", source), frame, visible, alpha) }
        if (view is TextView && view.text.isNotEmpty()) {
            addNode(JSONObject().put("kind", "bitmap").put("source", source).put("text", textStyle(view)), frame, visible, alpha)
        } else if (view is ImageView && view.drawable != null) {
            addNode(JSONObject().put("kind", "bitmap").put("source", source), frame, visible, alpha)
        }

        if (view is ViewRootForTest) {
            val semantics = mutableMapOf<Int, SemanticsNode>()
            collectSemantics(view.semanticsOwner.unmergedRootSemanticsNode, semantics)
            walkLayout(view.semanticsOwner.unmergedRootSemanticsNode.layoutInfo, semantics, visible, alpha)
        }
        if (view is ViewGroup) {
            val childClip = if (view.clipChildren) visible else clip
            drawingOrder(view).forEach { walkView(it, childClip, alpha) }
        }
        owner = outerOwner
    }

    /** Children in the order they draw: a custom drawing order (API 29+ exposes it), then by Z, stable. */
    private fun drawingOrder(group: ViewGroup): List<View> {
        val ordered = (0 until group.childCount).map { position ->
            group.getChildAt(if (Build.VERSION.SDK_INT >= 29) group.getChildDrawingOrder(position) else position)
        }
        return ordered.sortedBy { it.z }
    }

    private fun drawableNode(drawable: Drawable): JSONObject {
        val node = JSONObject()
        val color = when (drawable) {
            is ColorDrawable -> drawable.color
            is GradientDrawable -> drawable.color?.defaultColor
            else -> null
        }
        if (drawable is GradientDrawable) {
            if (Build.VERSION.SDK_INT >= 24 && drawable.cornerRadius > 0) node.put("corner", round2(drawable.cornerRadius.toDouble()))
            strokeOf(drawable)?.let { node.put("border", it) }
        }
        return if (color != null || node.has("border")) node.put("kind", "layer").also { color?.let(::hex)?.let { node.put("fill", it) } }
        else node.put("kind", "bitmap")
    }

    /** GradientDrawable has no public stroke getter; its stroke Paint is a hidden field. Null when it can't be read. */
    private fun strokeOf(drawable: GradientDrawable): JSONObject? = runCatching {
        val paint = Hidden.strokePaint?.get(drawable) as? Paint ?: return null
        val color = hex(paint.color) ?: return null
        if (paint.strokeWidth <= 0f) return null
        JSONObject().put("width", round2(paint.strokeWidth.toDouble())).put("color", color)
    }.getOrNull()

    private fun textStyle(view: TextView): JSONObject {
        val weight = if (Build.VERSION.SDK_INT >= 28) view.typeface?.weight ?: 400
        else if (view.typeface?.isBold == true) 700 else 400
        return JSONObject().put("size", round2(view.textSize.toDouble())).put("color", hex(view.currentTextColor) ?: "transparent")
            .put("weight", weight)
    }

    /** The new element's index, or null when the view is not one. */
    private fun viewElement(view: View, frame: RectF): Int? {
        val label = (view as? TextView)?.text?.toString()?.takeIf { it.isNotEmpty() }
            ?: view.contentDescription?.toString()
        val id = view.id.takeIf { it != View.NO_ID }?.let {
            runCatching {
                if (view.resources.getResourcePackageName(it) == "android") null
                else view.resources.getResourceEntryName(it)
            }.getOrNull()
        }
        if (label == null && id == null) return null
        elements.put(JSONObject().put("frame", rectArray(frame)).apply {
            id?.let { put("id", it) }
            label?.let { put("label", it) }
            if (view.isClickable && view.hasOnClickListeners()) put("tap", true)
        })
        return elements.length() - 1
    }

    private fun collectSemantics(node: SemanticsNode, into: MutableMap<Int, SemanticsNode>) {
        into[node.id] = node
        node.children.forEach { collectSemantics(it, into) }
    }

    /** A layout node's children in draw order (zIndex, then placement order). The getter is public on an internal class. */
    private fun zSortedChildren(node: LayoutInfo): List<LayoutInfo> = runCatching {
        val (getChildren, asList) = Hidden.layoutChildren(node) ?: return emptyList()
        @Suppress("UNCHECKED_CAST")
        asList.invoke(getChildren.invoke(node)) as List<LayoutInfo>
    }.getOrDefault(emptyList())

    // Walks every Compose layout node, not only the ones with semantics, so a plain Box with a background is
    // seen too (it paints, and it can cover something). Children come in draw order, so zIndex counts.
    // Semantics, where a node has them, name the elements.
    // A clickable that merges its children (Button, FilterChip, Modifier.clickable) is one element,
    // the way agent-device and uiautomator see it: its own frame, fill and border, its children's text.
    // Untagged children inside it are not elements of their own.
    // clipRound: the innermost rounded clip around this node ([x, y, w, h, radius]), so `ddp check corners`
    // knows which draws a rounded clip cuts.
    private fun walkLayout(layout: LayoutInfo, semantics: Map<Int, SemanticsNode>, clip: RectF, parentAlpha: Float,
                           insideMerged: Boolean = false, clipRound: JSONArray? = null) {
        if (!layout.isPlaced) return
        val modifierInfo = layout.getModifierInfo()
        val modifiers = modifierInfo.mapNotNull { info ->
            inspect(info.modifier)?.let { (name, props) -> Painted(name, props, windowRect(info.coordinates)) }
        }
        val alpha = parentAlpha * modifiers.filter { it.name == "graphicsLayer" || it.name == "alpha" }
            .fold(1f) { total, modifier -> total * ((modifier.props["alpha"] as? Float) ?: 1f) }
        val node = semantics[layout.semanticsId]
        val frame = node?.let {
            val position = it.positionInWindow
            RectF(position.x, position.y, position.x + it.size.width, position.y + it.size.height)
                .apply { offset(windowOffset.x, windowOffset.y) }
        } ?: windowRect(layout.coordinates)
        val visible = RectF(frame).takeIf { it.intersect(clip) }

        val text = node?.let(::textOf)
        val tag = node?.config?.getOrNull(SemanticsProperties.TestTag)
        val merges = node != null && !insideMerged && node.config.isMergingSemanticsOfDescendants
        val label = text ?: node?.config?.getOrNull(SemanticsProperties.ContentDescription)?.joinToString()
            ?: if (merges) childrenText(node!!) else null
        val outerOwner = owner
        if (node != null && (tag != null || (label != null && !insideMerged))) {
            owner = elements.length()
            elements.put(JSONObject().put("frame", rectArray(frame)).apply {
                tag?.let { put("id", it) }
                label?.let { put("label", it) }
                if (node.config.getOrNull(SemanticsActions.OnClick) != null) {
                    put("tap", true)
                    // Material pads small controls to 48 dp with an outer layout modifier: that is the touch area.
                    val touch = modifierInfo.map { windowRect(it.coordinates) }.fold(RectF(frame)) { all, r -> all.apply { union(r) } }
                    if (touch != frame) put("touch", rectArray(touch))
                }
            })
        }

        val clipLayer = modifiers.firstOrNull { it.name == "graphicsLayer" && it.props["clip"] == true }
        val childClip = if (clipLayer != null && visible != null) RectF(clipLayer.frame).apply { intersect(visible) } else clip
        val childClipRound = clipLayer?.let { cornerOf(it.props["shape"], it.size) }
            ?.let { radius -> rectArray(clipLayer.frame).put(radius) } ?: clipRound

        if (visible != null && alpha > 0.001f) {
            modifiers.filter { it.name == "background" }.forEach { modifier ->
                val paintVisible = RectF(modifier.frame).takeIf { it.intersect(clip) } ?: return@forEach
                val paint = JSONObject().put("kind", "layer")
                colorOf(modifier.props["color"] ?: modifier.props["brush"])?.let { paint.put("fill", it) }
                cornerOf(modifier.props["shape"], modifier.size)?.let { paint.put("corner", it) }
                addNode(paint, modifier.frame, paintVisible, alpha, clipRound)
            }
            // ponytail: drawBehind only (what Canvas uses). drawWithCache and drawWithContent need
            // internal Compose scopes; add them when an app draws its animation that way.
            modifiers.filter { it.name == "drawBehind" }.forEach { modifier ->
                @Suppress("UNCHECKED_CAST")
                val onDraw = modifier.props["onDraw"] as? DrawScope.() -> Unit ?: return@forEach
                // The node's own clip cuts what it draws, so draws use the child clip.
                DrawRecorder.replay(onDraw, modifier.size, composeDensity)?.forEach { op ->
                    val opFrame = RectF(op.bounds).apply { offset(modifier.frame.left, modifier.frame.top) }
                    val opVisible = RectF(opFrame).takeIf { it.intersect(childClip) } ?: return@forEach
                    op.points?.let { points ->
                        op.paint.put("points", JSONArray(points.mapIndexed { i, value ->
                            round2((value + if (i % 2 == 0) modifier.frame.left else modifier.frame.top).toDouble())
                        }))
                    }
                    addNode(op.paint.put("op", op.op), opFrame, opVisible, alpha, childClipRound)
                }
            }
            if (text != null) {
                addNode(JSONObject().put("kind", "bitmap").apply { textStyle(node!!)?.let { put("text", it) } },
                    frame, visible, alpha)
            }
        }

        zSortedChildren(layout).forEach {
            walkLayout(it, semantics, childClip, alpha, insideMerged || merges, childClipRound)
        }

        // Border draws after content, so it goes after the children in paint order.
        if (visible != null && alpha > 0.001f) {
            modifiers.filter { it.name == "border" }.forEach { modifier ->
                val width = (modifier.props["width"] as? Dp)?.value?.times(density) ?: return@forEach
                val color = colorOf(modifier.props["color"] ?: modifier.props["brush"]) ?: return@forEach
                if (width <= 0f) return@forEach
                val paintVisible = RectF(modifier.frame).takeIf { it.intersect(clip) } ?: return@forEach
                val paint = JSONObject().put("kind", "layer")
                    .put("border", JSONObject().put("width", round2(width.toDouble())).put("color", color))
                cornerOf(modifier.props["shape"], modifier.size)?.let { paint.put("corner", it) }
                addNode(paint, modifier.frame, paintVisible, alpha, clipRound)
            }
        }
        owner = outerOwner
    }

    /** One inspectable modifier and the rectangle it applies to, in activity-window pixels. */
    private class Painted(val name: String, val props: Map<String, Any?>, val frame: RectF) {
        val size get() = Size(frame.width(), frame.height())
    }

    private fun windowRect(coordinates: androidx.compose.ui.layout.LayoutCoordinates): RectF {
        val position = coordinates.positionInWindow()
        return RectF(position.x, position.y, position.x + coordinates.size.width, position.y + coordinates.size.height)
            .apply { offset(windowOffset.x, windowOffset.y) }
    }

    /** Size in px, color and weight of a Compose text, from its laid-out style. */
    private fun textStyle(node: SemanticsNode): JSONObject? {
        val results = mutableListOf<TextLayoutResult>()
        val laidOut = runCatching { node.config.getOrNull(SemanticsActions.GetTextLayoutResult)?.action?.invoke(results) }
            .getOrNull() == true
        val input = results.firstOrNull()?.layoutInput?.takeIf { laidOut } ?: return null
        val style = input.style
        return JSONObject().apply {
            if (style.fontSize.isSpecified) put("size", round2(with(input.density) { style.fontSize.toPx() }.toDouble()))
            if (style.color.isSpecified) hex(style.color.toArgb())?.let { put("color", it) }
            put("weight", style.fontWeight?.weight ?: 400)
        }
    }

    private fun textOf(node: SemanticsNode) =
        node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }

    private fun childrenText(node: SemanticsNode): String? =
        node.children.filterNot { it.config.isMergingSemanticsOfDescendants }
            .flatMap { listOfNotNull(textOf(it), childrenText(it)) }
            .joinToString(" ").ifEmpty { null }

    private fun cornerOf(shape: Any?, size: Size): Double? {
        val outline = (shape as? Shape)?.createOutline(size, LayoutDirection.Ltr, composeDensity)
        val radius = (outline as? Outline.Rounded)?.roundRect?.topLeftCornerRadius?.x ?: return null
        return round2(radius.toDouble()).takeIf { it > 0 }
    }

    private fun addNode(node: JSONObject, frame: RectF, visible: RectF, alpha: Float, clipRound: JSONArray? = null) {
        node.put("z", nodes.length()).put("frame", rectArray(frame)).put("own", owner)
        clipRound?.let { node.put("clipRound", it) }
        if (visible != frame) node.put("visible", rectArray(visible))
        if (alpha < 0.999f) node.put("opacity", round2(alpha.toDouble()))
        nodes.put(node)
    }
}

/**
 * Hidden or internal members, looked up once: reflection lookups per node per frame cost more than the reads.
 * Null when a platform or Compose version does not have them; the recorder then reads less, it does not fail.
 */
internal object Hidden {
    val windowManager: Triple<Any, java.lang.reflect.Method, java.lang.reflect.Method>? by lazy {
        runCatching {
            val global = Class.forName("android.view.WindowManagerGlobal")
            Triple(global.getMethod("getInstance").invoke(null)!!, global.getMethod("getViewRootNames"),
                global.getMethod("getRootView", String::class.java))
        }.getOrNull()
    }

    val strokePaint: java.lang.reflect.Field? by lazy {
        runCatching { GradientDrawable::class.java.getDeclaredField("mStrokePaint").apply { isAccessible = true } }.getOrNull()
    }

    private var children: Pair<java.lang.reflect.Method, java.lang.reflect.Method>? = null

    /** LayoutNode.getZSortedChildren and MutableVector.asMutableList; LayoutNode is internal, so found from an instance. */
    fun layoutChildren(node: LayoutInfo): Pair<java.lang.reflect.Method, java.lang.reflect.Method>? {
        children?.let { return it }
        return runCatching {
            val getChildren = node.javaClass.getMethod("getZSortedChildren")
            val asList = getChildren.returnType.getMethod("asMutableList")
            (getChildren to asList).also { children = it }
        }.getOrNull()
    }
}

/** A modifier's inspector name and properties (needs isDebugInspectorInfoEnabled). */
internal fun inspect(modifier: Any): Pair<String, Map<String, Any?>>? {
    val value = modifier as? InspectableValue ?: return null
    val name = value.nameFallback ?: modifier::class.simpleName ?: return null
    return name to value.inspectableElements.associate { it.name to it.value }
}

internal fun colorOf(value: Any?): String? = when (value) {
    is Color -> hex(value.toArgb())
    is SolidColor -> hex(value.value.toArgb())
    else -> null
}

/** "#RRGGBBAA", or null when fully transparent. */
internal fun hex(argb: Int): String? {
    val alpha = argb ushr 24
    if (alpha == 0) return null
    return "#%06X%02X".format(argb and 0xFFFFFF, alpha)
}

private fun rectArray(rect: RectF) =
    JSONArray(listOf(rect.left, rect.top, rect.width(), rect.height()).map { round2(it.toDouble()) })

internal fun round2(value: Double) = Math.round(value * 100) / 100.0
