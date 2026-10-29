package dev.agentddp.sample

import dev.agentddp.AgentDdpCommands
import dev.agentddp.AgentDdpState

/** The sample's own values for `ddp state`: a mode an agent can set, and how many loads ran. */
object SampleState {
    var mode = "normal"
    var loads = 0
}

/** Debug: what an agent can ask the sample to do (`app.commands`, `app.run`) and read (`state.*`). */
fun addSampleCommands() {
    AgentDdpCommands.add("load") { SampleNetwork.load().also { SampleState.loads++ } }
    AgentDdpState.add("mode", get = { SampleState.mode }, set = { SampleState.mode = it as? String ?: SampleState.mode })
    AgentDdpState.add("loads", get = { SampleState.loads })
}
