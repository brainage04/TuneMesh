package io.github.brainage04.tunemesh.data

import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RefreshGateTest {
    /** R2-astra-3: a refresh cancelled while it waits for the lock must not keep later refreshes out. */
    @Test
    fun cancelledWaitingRefreshDoesNotCloseTheGate() = runBlocking {
        val mutex = Mutex()
        val gate = RefreshGate(mutex)
        var runs = 0
        mutex.lock()
        val waiting = launch(start = CoroutineStart.UNDISPATCHED) { gate.run { runs++ } }
        assertTrue(gate.isQueued)
        waiting.cancel()
        waiting.join()
        assertFalse(gate.isQueued)
        mutex.unlock()
        gate.run { runs++ }
        gate.run { runs++ }
        assertEquals(2, runs)
        assertFalse(mutex.isLocked)
    }

    @Test
    fun requestsWhileOneWaitsJoinIt() = runBlocking {
        val mutex = Mutex()
        val gate = RefreshGate(mutex)
        var runs = 0
        mutex.lock()
        val first = launch(start = CoroutineStart.UNDISPATCHED) { gate.run { runs++ } }
        val second = launch(start = CoroutineStart.UNDISPATCHED) { gate.run { runs++ } }
        assertTrue(second.isCompleted)
        mutex.unlock()
        first.join()
        assertEquals(1, runs)
        // A failing refresh releases the lock and the gate.
        runCatching { gate.run { error("scan failed") } }
        gate.run { runs++ }
        assertEquals(2, runs)
    }
}
