package dev.agentddp

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ChangeTrackerTest {
    @Test
    fun anElementThatKeepsChangingBecomesBusy() {
        val tracker = ChangeTracker(busyAfterMs = 2_000, quietMs = 250)
        // Changes in two of every three captures, like an animation slower than the capture rate.
        (0 until 150).filter { it % 3 != 2 }.forEach { tracker.changed(mapOf("clock" to "$it"), it * 16L) }
        assertTrue(tracker.onlyBusyChanging)
    }

    @Test
    fun aCaptureWithNothingNewKeepsTheBusyState() {
        val tracker = ChangeTracker(busyAfterMs = 2_000, quietMs = 250)
        (0 until 150).forEach { tracker.changed(mapOf("clock" to "$it"), it * 16L) }
        assertEquals(emptyList<String>(), tracker.changed(mapOf("clock" to "149"), 150 * 16L))
        assertTrue(tracker.onlyBusyChanging)
    }

    @Test
    fun anotherElementChangingEndsTheBusyState() {
        val tracker = ChangeTracker(busyAfterMs = 2_000, quietMs = 250)
        (0 until 150).forEach { tracker.changed(mapOf("clock" to "$it", "button" to "off"), it * 16L) }
        assertTrue(tracker.onlyBusyChanging)
        assertEquals(listOf("clock", "button"), tracker.changed(mapOf("clock" to "150", "button" to "on"), 150 * 16L))
        assertFalse(tracker.onlyBusyChanging)
    }

    @Test
    fun aPauseLongerThanTheQuietGapStartsOver() {
        val tracker = ChangeTracker(busyAfterMs = 2_000, quietMs = 250)
        (0 until 150).forEach { tracker.changed(mapOf("clock" to "$it"), it * 16L) }
        tracker.changed(mapOf("clock" to "149"), 3_000)
        tracker.changed(mapOf("clock" to "again"), 3_400)
        assertFalse(tracker.onlyBusyChanging)
    }
}
