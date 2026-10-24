package dev.agentddp.sample

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.zIndex
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import okhttp3.OkHttpClient
import okhttp3.Request
import kotlin.concurrent.thread

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        addSampleCommands()
        // A deep link (ddpsample://...) shows up as the last action, for `ddp open`.
        val link = intent?.data?.let { "link $it" } ?: "none"
        setContent { MaterialTheme { SampleScreen(link) } }
    }
}

class CoveredActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MaterialTheme { SampleScreen() } }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun SampleScreen(initialAction: String = "none") {
    var isSelected by remember { mutableStateOf(false) }
    var lastAction by remember { mutableStateOf(initialAction) }
    var menuOpen by remember { mutableStateOf(false) }
    var isFlashing by remember { mutableStateOf(false) }
    // Planted bug for the v1 proof: a border that exists only while animating.
    val flashBorder by animateDpAsState(if (isFlashing) 4.dp else 0.dp, tween(400), label = "flash")
    var tapCount by remember { mutableStateOf(0) }
    LaunchedEffect(tapCount) {
        if (tapCount == 0) return@LaunchedEffect
        isFlashing = true
        delay(400)
        isFlashing = false
    }
    val slow = tween<Color>(durationMillis = 2000)
    val fill by animateColorAsState(if (isSelected) Color.Blue else Color.White, slow, label = "fill")
    val borderColor by animateColorAsState(if (isSelected) Color(0xFFFFA500) else Color.Blue, slow, label = "border")
    val borderWidth by animateDpAsState(if (isSelected) 6.dp else 2.dp, tween(2000), label = "borderWidth")
    val shape = RoundedCornerShape(12.dp)

    Column(Modifier.fillMaxSize().padding(top = 60.dp), horizontalAlignment = androidx.compose.ui.Alignment.CenterHorizontally) {
        Box(
            Modifier
                .testTag("animatedButton")
                .background(fill, shape)
                .border(borderWidth, borderColor, shape)
                .clickable {
                    isSelected = !isSelected
                    tapCount++
                    Log.i("DDPSample", "animatedButton tapped, selected=$isSelected")
                }
                .padding(horizontal = 32.dp, vertical = 14.dp)
        ) {
            Text(if (isSelected) "Selected" else "Tap me", color = if (isSelected) Color.White else Color.Blue)
        }
        Spacer(Modifier.height(24.dp))
        Text(
            "Flash",
            Modifier.testTag("flashBox").border(flashBorder, Color.Red, RoundedCornerShape(8.dp)).padding(10.dp)
        )
        // a ring flash drawn in a Canvas, alpha read inside the draw lambda.
        val ringAlpha by animateFloatAsState(if (isFlashing) 1f else 0f, tween(400), label = "ring")
        Canvas(Modifier.testTag("ringFlash").size(60.dp)) {
            drawCircle(Color.DarkGray, radius = 6.dp.toPx())
            drawCircle(Color.Magenta, radius = 24.dp.toPx(), alpha = ringAlpha, style = Stroke(3.dp.toPx()))
        }
        Spacer(Modifier.height(24.dp))
        // picked vs unpicked FilterChip fill and border.
        // "Open" has no test tag on purpose, like most chips in a real app.
        var pickedChip by remember { mutableStateOf("Picked") }
        Row {
            listOf("Picked", "Open").forEach { label ->
                FilterChip(
                    selected = pickedChip == label,
                    onClick = { pickedChip = label },
                    label = { Text(label) },
                    modifier = (if (label == "Picked") Modifier.testTag("chipPicked") else Modifier)
                        .padding(horizontal = 4.dp)
                )
            }
        }
        Spacer(Modifier.height(16.dp))
        // a hatch that goes past its card's rounded corners.
        // hatchCard has the bug; clippedCard clips the same hatch to its shape.
        Row {
            listOf("hatchCard" to false, "clippedCard" to true).forEach { (tag, clipped) ->
                val cardShape = RoundedCornerShape(16.dp)
                Box(
                    Modifier.padding(horizontal = 8.dp).testTag(tag)
                        .then(if (clipped) Modifier.clip(cardShape) else Modifier)
                        .background(Color(0xFFEEEEEE), cardShape).size(120.dp, 60.dp)
                ) {
                    Canvas(Modifier.testTag("$tag-hatch").fillMaxSize()) {
                        for (x in 0..size.width.toInt() step 12.dp.toPx().toInt()) {
                            drawLine(Color.Gray, Offset(x.toFloat(), 0f), Offset(x.toFloat(), size.height), 2.dp.toPx())
                        }
                    }
                }
            }
        }
        // An app timeline on the app's own clock port: `ddp time pause/step/seek` freeze, step and rewind it.
        val clockMillis by produceState(0L) { while (true) withFrameMillis { value = sampleClockMillis() } }
        val phase = (clockMillis % 2000) / 2000f
        Box(Modifier.padding(top = 12.dp).width(120.dp)) {
            Box(Modifier.testTag("clockBar").background(Color(0xFFFF9800)).height(8.dp).width(20.dp + 100.dp * phase))
        }
        Spacer(Modifier.height(16.dp))
        LazyColumn(Modifier.fillMaxWidth()) {
            item {
                Box {
                    Text(
                        "Row with context menu",
                        Modifier.testTag("menuRow").fillMaxWidth()
                            .combinedClickable(onClick = {}, onLongClick = { menuOpen = true })
                            .padding(16.dp)
                    )
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                        DropdownMenuItem(text = { Text("Rename") }, onClick = { lastAction = "rename"; menuOpen = false })
                        DropdownMenuItem(text = { Text("Delete") }, onClick = { lastAction = "delete"; menuOpen = false })
                    }
                }
            }
            item {
                Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    // A plain Box with no semantics over a tagged label: only a walk of every layout node sees it.
                    Box {
                        Text("Under a box", Modifier.testTag("boxCovered").padding(4.dp))
                        Box(Modifier.matchParentSize().background(Color(0xFF9E9E9E)))
                    }
                    Spacer(Modifier.width(8.dp))
                    // zIndex: the blue box comes first in code but draws on top of the label.
                    Box {
                        Box(Modifier.zIndex(1f).size(48.dp).background(Color(0xFF3F51B5)))
                        Text("Low z", Modifier.testTag("zCovered").size(48.dp))
                    }
                    Spacer(Modifier.width(8.dp))
                    // Too small to tap comfortably (24 dp, Material asks for 48).
                    Box(Modifier.testTag("smallTarget").size(24.dp).background(Color.LightGray).clickable { lastAction = "small" })
                    Spacer(Modifier.width(8.dp))
                    Text("Faint text", Modifier.testTag("faintText"), color = Color(0xFFCCCCCC))
                }
            }
            item { Text("Last action: $lastAction", Modifier.padding(16.dp)) }
            item {
                // For `Input` typing: Enter submits and shows the text as the last action.
                var name by remember { mutableStateOf("") }
                TextField(
                    name, { name = it }, Modifier.testTag("nameField").fillMaxWidth().padding(horizontal = 16.dp),
                    placeholder = { Text("Name") }, singleLine = true,
                    keyboardActions = KeyboardActions(onDone = { lastAction = "submit $name" }),
                )
            }
            item {
                Text("Load", Modifier.testTag("netButton").clickable {
                    thread { lastAction = SampleNetwork.load() }
                }.padding(16.dp))
            }
        }
    }
}

/** The sample's network call to a local test server on the host; nothing leaves the machine. */
object SampleNetwork {
    const val DEV_SECRET = "dev-secret-123"
    private val client = observedHttpClient(OkHttpClient.Builder())

    fun load(): String = runCatching {
        val request = Request.Builder().url("http://10.0.2.2:18765/items.json?token=abc&page=1")
            .header("Authorization", "Bearer $DEV_SECRET").header("X-Client", "sample").build()
        client.newCall(request).execute().use { "loaded ${it.code}" }
    }.getOrDefault("load failed")
}
