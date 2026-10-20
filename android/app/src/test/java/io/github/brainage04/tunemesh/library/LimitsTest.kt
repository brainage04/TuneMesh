package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException
import java.nio.file.Files

class LimitsTest {
    @get:Rule
    val temp = TemporaryFolder()

    @Test
    fun boundedReadsRefuseLargerFilesAndSymlinksWhenAsked() {
        val file = File(temp.root, "a.json").apply { writeBytes(ByteArray(100) { 'x'.code.toByte() }) }
        assertEquals(100, Limits.readBytes(file, 100).size)
        assertThrows(TooLargeException::class.java) { Limits.readBytes(file, 99) }
        val link = File(temp.root, "link.json")
        Files.createSymbolicLink(link.toPath(), file.toPath())
        assertEquals(100, Limits.readText(link, 100).length)
        assertThrows(IOException::class.java) { Limits.readText(link, 100, followLinks = false) }
        assertThrows(IOException::class.java) { Limits.readText(temp.root, 100) }
    }

    @Test
    fun cutUtf8StopsAtACharacterBoundary() {
        assertEquals("ab", Limits.cutUtf8("ab", 2))
        assertEquals("a", Limits.cutUtf8("aé", 2))
        assertEquals("aé", Limits.cutUtf8("aé", 3))
        assertEquals("", Limits.cutUtf8("🎵", 3))
        assertEquals("🎵", Limits.cutUtf8("🎵x", 4))
        assertEquals(1024, Limits.utf8Length(Limits.cutUtf8("é".repeat(600), 1024)))
        assertEquals(3, Limits.utf8Length("\uD800"))
    }

    @Test
    fun shownTextsFollowTheDisplayLimits() {
        assertEquals("short", Limits.shownLine("short"))
        val line = Limits.shownLine("x".repeat(1000))
        assertEquals(Limits.SHOWN_LINE_CHARS, line.length)
        assertTrue(line.endsWith("…"))
        val message = Limits.shownMessage("line\n".repeat(100))
        assertEquals(Limits.SHOWN_MESSAGE_LINES, message.lines().size)
        assertTrue(message.endsWith("…"))
        assertEquals(Limits.SHOWN_MESSAGE_CHARS, Limits.shownMessage("y".repeat(10_000)).length)
        assertEquals("a\nb", Limits.shownMessage("a\nb"))
    }

    @Test
    fun tagsAreCutAndArtistsLimited() {
        val artists = (1..40).joinToString("; ") { "Artist $it" }
        val track = Track.fromTags(
            "Huge/Tags/big tag.mp3",
            RawTags(title = "t".repeat(1024 * 1024), artist = artists, album = "a".repeat(2000)),
            1,
            1,
        )
        assertEquals(Limits.MAX_TAG_TEXT_BYTES, track.title.length)
        assertEquals(Limits.MAX_TAG_TEXT_BYTES, track.album.length)
        assertEquals((1..32).joinToString("; ") { "Artist $it" }, track.artist)
        assertEquals("Artist 1", track.albumArtist)
        val fallback = Track.fromTags("A/B/Song [x].mp3", RawTags(title = "   "), 1, 1)
        assertEquals("Song", fallback.title)
    }
}
