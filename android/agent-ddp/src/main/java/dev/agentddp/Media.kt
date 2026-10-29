package dev.agentddp

import android.view.View
import android.view.ViewGroup
import android.widget.VideoView
import org.json.JSONArray
import org.json.JSONObject

/**
 * `media.list`: the players the platform shows the library, never a guess: every `VideoView` in the activity's
 * window, through its public API. A Media3 or MediaPlayer player has no public way in without depending on that
 * library, so the app shows it as a value (`AgentDdpState.add`, see `ddp skills get app`).
 */
internal object Media {
    fun list(): JSONObject = onMain {
        val players = JSONArray()
        FrameRecorder.resumedActivity?.window?.decorView?.let { collect(it, players) }
        JSONObject().put("players", players)
    }

    private fun collect(view: View, players: JSONArray) {
        if (view is VideoView && view.isShown) {
            val place = IntArray(2).also(view::getLocationInWindow)
            players.put(JSONObject().put("source", "VideoView").put("state", if (view.isPlaying) "playing" else "paused")
                .put("position", view.currentPosition / 1000.0)
                .put("duration", view.duration.takeIf { it >= 0 }?.let { it / 1000.0 } ?: JSONObject.NULL)
                .put("frame", JSONArray(listOf(place[0], place[1], view.width, view.height))))
        }
        if (view is ViewGroup) for (index in 0 until view.childCount) collect(view.getChildAt(index), players)
    }
}
