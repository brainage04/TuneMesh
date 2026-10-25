package io.github.brainage04.tunemesh.data

import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.sync.Mutex

/**
 * Runs refreshes under [mutex], merging requests: while one refresh waits for the mutex and has not started yet, further requests join it
 * instead of queueing a refresh each.
 */
class RefreshGate(private val mutex: Mutex) {
    private val queued = AtomicBoolean(false)

    /** Whether a refresh is waiting for the mutex (for tests). */
    val isQueued: Boolean
        get() = queued.get()

    suspend fun run(block: () -> Unit) {
        if (!queued.compareAndSet(false, true)) return
        var locked = false
        try {
            mutex.lock()
            locked = true
        } finally {
            // Cancelled while waiting: this request still owns the flag, so the next one must be able to start.
            if (!locked) queued.set(false)
        }
        try {
            queued.set(false)
            block()
        } finally {
            mutex.unlock()
        }
    }
}
