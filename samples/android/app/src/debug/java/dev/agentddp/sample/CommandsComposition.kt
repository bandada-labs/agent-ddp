package dev.agentddp.sample

import dev.agentddp.AgentDdpCommands
import dev.agentddp.AgentDdpEvents
import dev.agentddp.AgentDdpState
import android.os.Handler
import android.os.Looper

/** The sample's own values for `ddp state`: a mode an agent can set, and how many loads ran. */
object SampleState {
    var mode = "normal"
    var loads = 0
}

/** Debug: what an agent can ask the sample to do (`app.commands`, `app.run`) and read (`state.*`). */
fun addSampleCommands() {
    AgentDdpCommands.add("load") {
        SampleNetwork.load().also { answer ->
            SampleState.loads++
            AgentDdpEvents.send("loaded", mapOf("loads" to SampleState.loads, "answer" to answer))
        }
    }
    // For `ddp perf`: the main thread busy for 200 ms, about 11 refreshes missed at 60 Hz.
    AgentDdpCommands.add("jank") {
        Handler(Looper.getMainLooper()).post { Thread.sleep(200) }
        null
    }
    // For `ddp crash`: an uncaught exception on the main thread.
    AgentDdpCommands.add("crash") {
        Handler(Looper.getMainLooper()).post { throw IllegalStateException("the sample crashed on purpose") }
        null
    }
    AgentDdpState.add("mode", get = { SampleState.mode }, set = { SampleState.mode = it as? String ?: SampleState.mode })
    AgentDdpState.add("loads", get = { SampleState.loads })
}
