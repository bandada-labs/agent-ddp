package dev.agentddp

import android.os.Handler
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.unit.Dp
import org.json.JSONObject
import java.util.concurrent.ExecutionException
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit

/**
 * Read-only property reads for `screen.value` (`ddp eval <element> <path>`): `<modifier>.<property>` from the Compose
 * modifier inspector (border.width, graphicsLayer.alpha, background.color) or `semantics.<Key>`
 * (semantics.Selected, semantics.StateDescription) on the semantics node with that test tag or text.
 */
internal object LiveEval {
    fun evaluate(name: String, path: String): JSONObject {
        val (scope, property) = path.split(".", limit = 2).takeIf { it.size == 2 }
            ?: return error("path is <modifier>.<property> or semantics.<Key>, e.g. border.width")
        val root = FrameRecorder.resumedActivity?.window?.decorView ?: return error("no activity in the foreground")
        val node = composeRoots(root).firstNotNullOfOrNull { find(it.semanticsOwner.unmergedRootSemanticsNode, name) }
            ?: return error("no Compose element '$name' (tag it with Modifier.testTag)")
        val value = if (scope == "semantics") {
            node.config.firstOrNull { it.key.name == property }?.value
                ?: return error("no semantics '$property'; has: ${node.config.joinToString { it.key.name }}")
        } else {
            val modifiers = node.layoutInfo.getModifierInfo().mapNotNull { inspect(it.modifier) }
            val properties = modifiers.firstOrNull { it.first == scope }?.second
                ?: return error("no modifier '$scope'; has: ${modifiers.joinToString { it.first }}")
            if (property !in properties) return error("'$scope' has: ${properties.keys.joinToString()}")
            properties[property]
        }
        return JSONObject().put("type", "value").put("path", path).put("value", jsonValue(value))
    }

    private fun composeRoots(view: View): List<ViewRootForTest> = when (view) {
        is ViewRootForTest -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { composeRoots(view.getChildAt(it)) }
        else -> emptyList()
    }

    private fun find(node: SemanticsNode, name: String): SemanticsNode? {
        val tag = node.config.getOrNull(SemanticsProperties.TestTag)
        val text = node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }
        if (tag == name || text == name) return node
        return node.children.firstNotNullOfOrNull { find(it, name) }
    }

    private fun jsonValue(value: Any?): Any = when (value) {
        null -> JSONObject.NULL
        is Number, is Boolean, is String -> value
        is Dp -> value.value
        is Color -> hex(value.toArgb()) ?: "transparent"
        else -> colorOf(value) ?: value.toString()
    }

    private fun error(message: String) = JSONObject().put("type", "error").put("error", message)
}

/** Runs [block] on the main thread and waits for it, like a capture does. Called from socket threads. */
internal fun <T> onMain(block: () -> T): T =
    if (Looper.myLooper() == Looper.getMainLooper()) block()
    else try {
        FutureTask(block).also(Handler(Looper.getMainLooper())::post).get(5, TimeUnit.SECONDS)
    } catch (wrapped: ExecutionException) {
        throw wrapped.cause ?: wrapped
    }
