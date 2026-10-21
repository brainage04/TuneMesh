package io.github.brainage04.tunemesh.data

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.sync.Mutex
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Runs refreshes under [mutex], merging requests: while one refresh waits for the mutex and has not started yet,
 * further requests join it instead of queueing a refresh each.
 */
class RefreshGate(private val mutex: Mutex) {
    private val queued = AtomicBoolean(false)

    /** Whether a refresh is waiting for the mutex (for tests). */
    val isQueued: Boolean get() = queued.get()

    suspend fun run(block: () -> Unit) {
        if (!queued.compareAndSet(false, true)) return
        try {
            mutex.lock()
        } catch (cancelled: CancellationException) {
            // Cancelled while waiting: this request still owns the flag, so the next one must be able to start.
            queued.set(false)
            throw cancelled
        }
        try {
            queued.set(false)
            block()
        } finally {
            mutex.unlock()
        }
    }
}
