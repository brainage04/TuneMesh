package io.github.brainage04.tunemesh.library

import com.code_intelligence.jazzer.api.FuzzedDataProvider
import com.code_intelligence.jazzer.junit.FuzzTest
import java.io.ByteArrayInputStream
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue

/** Jazzer targets for the playlist reader and writer; see `fuzz` in app/build.gradle.kts. */
class M3u8FuzzTest {
    /**
     * Any text: parsing throws nothing but [PlaylistTooLargeException], every library path stays inside the library, what is written reads
     * back as the same entries, and writing that again gives the same text.
     */
    @FuzzTest(maxDuration = "10s")
    fun parseWriteRoundTrip(data: FuzzedDataProvider) {
        val text = data.consumeRemainingAsString()
        val entries =
            try {
                M3u8.parse(text, ROOT)
            } catch (_: PlaylistTooLargeException) {
                return
            }
        for (entry in entries) entry.path?.let(::assertInsideLibrary)
        val written =
            try {
                M3u8.write(entries) { null }
            } catch (_: PlaylistTooLargeException) {
                return
            }
        val again = M3u8.parse(written, ROOT)
        val kept = entries.filterNot { LibraryFormat.hasLineBreak(it.path ?: it.raw) }
        assertEquals(kept.map { it.path }, again.map { it.path })
        assertEquals(written, M3u8.write(again) { null })
    }

    /**
     * Any bytes under random limits: the streaming reader stops with [PlaylistTooLargeException] past them and otherwise reads the same
     * entries as [M3u8.parse]; [PlaylistEntries] stores and gives back each of them unchanged, shared or not.
     */
    @FuzzTest(maxDuration = "10s")
    fun readWithLimits(data: FuzzedDataProvider) {
        val maxEntries = data.consumeInt(0, 50)
        val maxBytes = data.consumeLong(0, 4096)
        val share = data.consumeBoolean()
        val bytes = data.consumeRemainingAsBytes()
        val builder = PlaylistEntries.Builder(ROOT, keepExtinf = true)
        try {
            M3u8.read(ByteArrayInputStream(bytes), ROOT, keepExtinf = true, maxBytes, maxEntries, canonical = { if (share) it else null }) {
                builder.add(it)
            }
        } catch (_: PlaylistTooLargeException) {
            return
        }
        val stored = builder.build()
        assertTrue(stored.size <= maxEntries)
        assertTrue(bytes.size <= maxBytes)
        val parsed = M3u8.parse(String(bytes, Charsets.UTF_8), ROOT)
        assertEquals(parsed.size, stored.size)
        for (index in parsed.indices) {
            val entry = parsed[index]
            assertEquals(entry, stored[index])
            assertEquals(entry.path, stored.path(index))
            assertEquals(entry.raw, stored.raw(index))
            assertEquals(entry.key, stored.key(index))
            assertEquals(entry.extinf, stored.extinf(index))
        }
    }

    private fun assertInsideLibrary(path: String) {
        assertFalse(path.startsWith("/"), path)
        assertTrue(path.split('/').none { it.isEmpty() || it == "." || it == ".." }, path)
    }

    private companion object {
        const val ROOT = "/storage/emulated/0/Music/TuneMesh"
    }
}
