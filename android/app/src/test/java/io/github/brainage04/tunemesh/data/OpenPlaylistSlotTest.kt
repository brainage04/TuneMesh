package io.github.brainage04.tunemesh.data

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The closePlaylist/reloadOpenPlaylist race (round 3): a load that started before a close never puts its entries back. */
class OpenPlaylistSlotTest {
    /** What LibraryRepository keeps in its state: the open playlist's entries. */
    @Volatile private var shown: String? = null

    @Test
    fun aLoadThatStartedBeforeACloseIsDropped() {
        val slot = OpenPlaylistSlot()
        assertTrue(slot.open("A"))
        val ticket = slot.current
        assertTrue(slot.close("A") { shown = null })
        assertFalse(slot.ifCurrent(ticket) { shown = "entries of A" })
        assertNull(shown)
    }

    @Test
    fun aLoadForAPlaylistOpenedAgainIsDroppedButOneStillOpenApplies() {
        val slot = OpenPlaylistSlot()
        slot.open("A")
        val stale = slot.current
        slot.close("A") { shown = null }
        slot.open("A")
        assertFalse("the screen was closed and opened again meanwhile", slot.ifCurrent(stale) { shown = "old A" })
        val fresh = slot.current
        assertFalse("opening the open playlist again keeps its generation", slot.open("A"))
        assertTrue(slot.ifCurrent(fresh) { shown = "A" })
        assertEquals("A", shown)
        assertFalse("another playlist's close does nothing", slot.close("B") { shown = null })
        assertEquals("A", shown)
    }

    /** The interleaving of the race: the load checked that A is open, then the screen closed before the load applied its entries. */
    @Test
    fun aCloseDuringTheApplyRunsAfterIt() {
        val slot = OpenPlaylistSlot()
        slot.open("A")
        val ticket = slot.current
        val applying = CountDownLatch(1)
        val release = CountDownLatch(1)
        val loader = thread {
            slot.ifCurrent(ticket) {
                applying.countDown()
                release.await(5, TimeUnit.SECONDS)
                shown = "entries of A"
            }
        }
        assertTrue(applying.await(5, TimeUnit.SECONDS))
        val closer = thread { slot.close("A") { shown = null } }
        Thread.sleep(50)
        release.countDown()
        loader.join(5_000)
        closer.join(5_000)
        assertNull("the close cleared what the load applied", shown)
        assertFalse(slot.ifCurrent(ticket) { shown = "again" })
    }
}
