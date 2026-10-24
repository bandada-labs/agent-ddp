package dev.agentddp.sample

import dev.agentddp.AgentDdpCommands

/** Debug: what an agent can ask the sample to do (`app.commands`, `app.run`). */
fun addSampleCommands() {
    AgentDdpCommands.add("load") { SampleNetwork.load() }
}
