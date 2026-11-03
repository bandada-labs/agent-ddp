package dev.agentddp

import androidx.compose.runtime.tooling.CompositionData
import androidx.compose.ui.R
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.tooling.data.CallGroup
import androidx.compose.ui.tooling.data.Group
import androidx.compose.ui.tooling.data.NodeGroup
import androidx.compose.ui.tooling.data.UiToolingDataApi
import androidx.compose.ui.tooling.data.asTree
import org.json.JSONArray
import org.json.JSONObject
import java.util.IdentityHashMap
import kotlin.math.absoluteValue

/**
 * Where an element comes from in the app's code (`screen.source`, `ddp source`): the composable calls above its
 * layout node, innermost first, each with the file and line it is called from, and the node's modifier chain.
 *
 * The data is Compose's own: in Debug builds the compiler writes source information for every composable call, and
 * the composition keeps it in its slot table when inspection tables are on (the library turns them on at start, the
 * way Layout Inspector does). ui-tooling-data, Google's parser for it, is compileOnly: the app brings its own copy
 * (Android Studio's template has `ui-tooling` for @Preview, which pulls it in), so its version follows the app's
 * Compose. Without it, nothing is guessed.
 */
@OptIn(UiToolingDataApi::class)
internal object SourceLocations {
    private const val NO_TOOLING = "add debugImplementation(\"androidx.compose.ui:ui-tooling-data\") to the app"

    /**
     * The compiler tags each call with abs(package.hashCode()) of the file that makes it. These are the androidx
     * packages with composables in Compose BOM 2024.12 (read from a built app), so a call from inside a library shows
     * as one. ponytail: a package added later is shown as a plain call, never hidden; add it here when it appears.
     */
    private val androidxPackages = listOf(
        "androidx.activity.compose", "androidx.compose.animation", "androidx.compose.animation.core",
        "androidx.compose.foundation", "androidx.compose.foundation.contextmenu", "androidx.compose.foundation.gestures",
        "androidx.compose.foundation.gestures.snapping", "androidx.compose.foundation.interaction",
        "androidx.compose.foundation.layout", "androidx.compose.foundation.lazy", "androidx.compose.foundation.lazy.grid",
        "androidx.compose.foundation.lazy.layout", "androidx.compose.foundation.lazy.staggeredgrid",
        "androidx.compose.foundation.pager", "androidx.compose.foundation.selection", "androidx.compose.foundation.text",
        "androidx.compose.foundation.text.input", "androidx.compose.foundation.text.selection",
        "androidx.compose.material", "androidx.compose.material.internal", "androidx.compose.material.pullrefresh",
        "androidx.compose.material.ripple", "androidx.compose.material3", "androidx.compose.material3.carousel",
        "androidx.compose.material3.internal", "androidx.compose.material3.pulltorefresh", "androidx.compose.runtime",
        "androidx.compose.runtime.internal", "androidx.compose.runtime.saveable", "androidx.compose.ui.graphics",
        "androidx.compose.ui.graphics.vector", "androidx.compose.ui.input.pointer", "androidx.compose.ui.layout",
        "androidx.compose.ui.platform", "androidx.compose.ui.res", "androidx.compose.ui.text",
        "androidx.compose.ui.tooling", "androidx.compose.ui.viewinterop", "androidx.compose.ui.window",
        "androidx.lifecycle.compose", "androidx.navigation.compose",
    ).associateBy { it.hashCode().absoluteValue }

    /** Main thread only. The element named by id or label, as `screen.hit` names it. */
    fun of(name: String): JSONObject {
        val capture = FrameCapture.capture()
        val index = (0 until capture.elements.length()).firstOrNull { i ->
            val element = capture.elements.getJSONObject(i)
            element.optString("id") == name || element.optString("label") == name
        } ?: throw IllegalArgumentException("No element '$name'")
        val target = capture.targets[index]
        val layout = target.layout ?: return none("a View, not a Compose element: Android keeps no call position for Views")

        @Suppress("UNCHECKED_CAST")
        val tables = runCatching { target.view.getTag(R.id.inspection_slot_table_set) as? Set<CompositionData> }.getOrNull()
        if (tables.isNullOrEmpty()) {
            Hooks.failed("compose.inspectionTables", "no inspection_slot_table_set tag on the compose view")
            return none("Compose kept no inspection tables for this screen (it was set up before ddp started)")
        }
        Hooks.ok("compose.inspectionTables")
        val trees = try {
            tables.toList().map { it.asTree() }
        } catch (missing: LinkageError) {
            Hooks.failed("compose.sourceInformation", missing.toString())
            return none(NO_TOOLING)
        } catch (failed: RuntimeException) {
            Hooks.failed("compose.sourceInformation", failed.toString())
            return none("ui-tooling-data could not read the composition: $failed")
        }
        Hooks.ok("compose.sourceInformation")

        val parents = IdentityHashMap<Group, Group>()
        val groupOfNode = IdentityHashMap<Any, NodeGroup>()
        val pending = ArrayDeque(trees)
        while (pending.isNotEmpty()) {
            val group = pending.removeLast()
            if (group is NodeGroup) groupOfNode[group.node] = group
            group.children.forEach { child -> parents[child] = group; pending.addLast(child) }
        }

        // Up the groups of the composition that holds the node, then on in the composition that hosts that one
        // (a LazyColumn item is its own composition): its top node's parent layout node is in the host's table.
        val calls = JSONArray()
        var located = false
        var node: LayoutInfo? = layout
        var hops = 0
        while (node != null && hops++ < 10_000) {
            val start = groupOfNode[node]
            if (start == null) {
                node = node.parentInfo
                continue
            }
            var group: Group? = start
            var top: NodeGroup = start
            while (group != null) {
                if (group is NodeGroup) top = group
                if (group is CallGroup) call(group)?.let { row ->
                    calls.put(row)
                    if (row.has("line")) located = true
                }
                group = parents[group]
            }
            node = (top.node as? LayoutInfo)?.parentInfo
        }
        if (!located) return none("the app was built without Compose source information (a release build, or the compiler's option is off)")
        return JSONObject().put("status", "ok").put("calls", calls).put("modifiers", modifiers(layout))
    }

    private fun call(group: CallGroup): JSONObject? {
        val name = group.name ?: return null
        val row = JSONObject().put("name", name)
        val at = group.location ?: return row
        at.sourceFile?.let { row.put("file", it) }
        // The compiler writes lines from 0; ui-tooling-data hands them counted from 1, as the file shows them.
        row.put("line", at.lineNumber).put("offset", at.offset).put("length", at.length)
        androidxPackages[at.packageHash]?.let { row.put("library", it) }
        return row
    }

    /** The node's modifiers in chain order, with the values the Compose inspector gives (plain values only). */
    private fun modifiers(layout: LayoutInfo): JSONArray = JSONArray(layout.getModifierInfo().map { info ->
        val inspected = inspect(info.modifier)
        val row = JSONObject().put("name", inspected?.first ?: info.modifier.javaClass.simpleName)
        if (inspected == null) row.put("inspector", false)
        inspected?.second?.forEach { (key, value) -> plainValue(value)?.let { row.put(key, it) } }
        row
    })

    private fun none(why: String) = JSONObject().put("status", "none").put("why", why)
}
