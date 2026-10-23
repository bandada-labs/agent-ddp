package dev.agentddp.sample

import dev.agentddp.AgentDdpTime

/** Debug: the sample's clock port reads ddp's controllable clock, so `ddp time` can pause and rewind it. */
fun sampleClockMillis(): Long = AgentDdpTime.elapsedMillis()
