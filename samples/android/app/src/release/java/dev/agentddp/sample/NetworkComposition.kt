package dev.agentddp.sample

import okhttp3.OkHttpClient

/** Release: the plain client. ddp is not in this build. */
fun observedHttpClient(builder: OkHttpClient.Builder): OkHttpClient = builder.build()
