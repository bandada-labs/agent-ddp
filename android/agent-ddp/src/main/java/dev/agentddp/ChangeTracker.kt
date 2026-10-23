package dev.agentddp

/**
 * Which elements changed since the last capture, and whether only busy ones did. An element that keeps changing
 * for [busyAfterMs], never still for longer than [quietMs], is busy: a clock, a spinner, a loop. Captures can come
 * faster than an animation's own frames, so "changed in every capture" would miss them.
 */
internal class ChangeTracker(private val busyAfterMs: Long, private val quietMs: Long) {
    private var last = emptyMap<String, String>()
    private val changingSince = mutableMapOf<String, Long>()
    private val lastChangeAt = mutableMapOf<String, Long>()

    var onlyBusyChanging = false
        private set

    fun changed(signatures: Map<String, String>, now: Long): List<String> {
        val changed = (signatures.keys + last.keys).filter { signatures[it] != last[it] }
        last = signatures
        changed.forEach { key ->
            val previous = lastChangeAt[key]
            if (previous == null || now - previous > quietMs) changingSince[key] = now
            lastChangeAt[key] = now
        }
        // A capture that saw nothing new does not end a busy stretch: it only came between two of its frames.
        if (changed.isNotEmpty()) onlyBusyChanging = changed.all { now - changingSince.getValue(it) >= busyAfterMs }
        return changed
    }
}
