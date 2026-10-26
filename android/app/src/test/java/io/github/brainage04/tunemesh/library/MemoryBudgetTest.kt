package io.github.brainage04.tunemesh.library

import io.github.brainage04.tunemesh.HostileFiles
import java.io.File
import java.lang.management.ManagementFactory
import java.time.Instant
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/**
 * Per synced format, what a reader keeps of files with every field and list at or past its limit ([HostileFiles]), at the most files it
 * reads: within a fixed share of the bytes read and an absolute cap (contract: "Limits"). [RetainedSize] estimates the retained heap.
 */
class MemoryBudgetTest {
    @get:Rule val temp = TemporaryFolder()

    private fun report(format: String, retained: Long, fileBytes: Long, what: String = "retained") {
        println(
            "MemoryBudget $format: $what $retained B for $fileBytes B of files (" +
                String.format(Locale.ROOT, "%.4f", retained.toDouble() / fileBytes) +
                ")"
        )
    }

    /** R3-Opus-1: 200 requests and 200 statuses of just under 1 MiB each; the list keeps what the cards show. */
    @Test
    fun requestsAndStatuses() {
        val root = temp.newFolder("lib")
        val fileBytes = HostileFiles.writeDownloads(root, Limits.MAX_DOWNLOADS_READ + 20)
        val list = DownloadStore(root).list()
        assertEquals(Limits.MAX_DOWNLOADS_READ, list.items.size)
        assertEquals(20, list.older)
        val retained = RetainedSize.of(list)
        report("requests+statuses", retained, fileBytes)
        assertTrue("$retained B", retained <= fileBytes / 16)
        assertTrue("$retained B", retained <= 20L * 1024 * 1024)

        // Each format on its own: requests only, then statuses only.
        File(root, ".tunemesh/status").deleteRecursively()
        val requestsOnly = RetainedSize.of(DownloadStore(root).list())
        report("requests", requestsOnly, fileBytes / 2)
        assertTrue("$requestsOnly B", requestsOnly <= Limits.MAX_DOWNLOADS_READ * 4L * 1024)
        val statusRoot = temp.newFolder("statuses")
        HostileFiles.writeDownloads(statusRoot, Limits.MAX_DOWNLOADS_READ)
        File(statusRoot, ".tunemesh/requests").deleteRecursively()
        val statusesOnly = RetainedSize.of(DownloadStore(statusRoot).list())
        report("statuses", statusesOnly, fileBytes / 2)
        assertTrue("$statusesOnly B", statusesOnly <= 20L * 1024 * 1024)
    }

    @Test
    fun workerJson() {
        val root = temp.newFolder("lib")
        val text = HostileFiles.worker()
        File(root, ".tunemesh").mkdirs()
        File(root, ".tunemesh/worker.json").writeText(text)
        val info = DownloadStore(root).worker()!!
        val fresh = info.copy(heartbeatAt = "2026-10-06T05:00:00Z")
        val downloader = Downloader.of(fresh, Instant.parse("2026-10-06T05:01:00Z"))
        assertTrue(downloader is Downloader.Active)
        val retained = RetainedSize.of(info, downloader)
        report("worker.json", retained, text.length.toLong())
        assertTrue("$retained B", retained <= 8L * 1024)
    }

    /**
     * A playlist at the entry limit and one at the byte limit whose every label is over its limit, as a screen and as a writer reads them.
     */
    @Test
    fun playlists() {
        val root = temp.newFolder("lib")
        val dir = File(root, LibraryFormat.PLAYLISTS_DIR).apply { mkdirs() }
        val store = PlaylistStore(root) { null }
        for ((name, text) in listOf("Entries" to HostileFiles.entryPlaylist(), "Labels" to HostileFiles.labelPlaylist())) {
            val file = File(dir, "$name.m3u8").apply { writeText(text) }
            val bytes = file.length()
            assertTrue(bytes <= Limits.MAX_PLAYLIST_BYTES)
            val screen = RetainedSize.of(store.contents(name))
            val writer = RetainedSize.of(store.read(name))
            val summaries = RetainedSize.of(store.list())
            report("playlist $name (screen)", screen, bytes)
            report("playlist $name (writer)", writer, bytes)
            report("playlist $name (summary)", summaries, bytes)
            assertTrue("$screen B", screen <= 2 * bytes)
            assertTrue("$writer B", writer <= 3 * bytes)
            assertTrue("$summaries B", summaries <= 4L * 1024)
            file.delete()
        }
    }

    /** `.stignore` keeps nothing; upkeep of one at the 1 MiB limit allocates a few times its size. */
    @Test
    fun stignore() {
        val root = temp.newFolder("lib")
        val line = "// rule with é and \u0001 bytes\n"
        val text = line.repeat((Limits.MAX_STIGNORE_BYTES - 64) / line.toByteArray().size)
        // Once on a small file first, so class loading is not counted.
        Stignore.ensure(temp.newFolder("warm-up"))
        File(root, Stignore.FILE_NAME).writeText(text)
        val threads = ManagementFactory.getThreadMXBean() as com.sun.management.ThreadMXBean
        val thread = Thread.currentThread().id
        val before = threads.getThreadAllocatedBytes(thread)
        assertTrue(Stignore.ensure(root))
        val allocated = threads.getThreadAllocatedBytes(thread) - before
        val bytes = text.toByteArray().size.toLong()
        report(".stignore", allocated, bytes, what = "allocated")
        assertTrue("$allocated B", allocated <= 6 * bytes)
    }
}
