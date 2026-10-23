package dev.agentddp.sample

import dev.agentddp.AgentDdpInterceptor
import dev.agentddp.AgentDdpNetwork
import okhttp3.OkHttpClient

/** Debug composition root for the sample's network: ddp sees every request, redacted. */
fun observedHttpClient(builder: OkHttpClient.Builder): OkHttpClient {
    AgentDdpNetwork.redact(SampleNetwork.DEV_SECRET)
    return builder.addInterceptor(AgentDdpInterceptor()).build()
}
