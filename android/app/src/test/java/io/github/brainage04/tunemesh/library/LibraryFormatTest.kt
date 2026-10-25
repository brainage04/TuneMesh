package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LibraryFormatTest {
    // The first five tests mirror tunemesh-core's sanitize.rs tests case for case.

    @Test
    fun replacesForbiddenAndControlCharacters() {
        assertEquals("AC_DC_ _Live_ _1991_____", LibraryFormat.sanitizeComponent("AC/DC: \"Live\" <1991>?|*\\"))
        assertEquals("tab_here_", LibraryFormat.sanitizeComponent("tab\there\u0007"))
    }

    @Test
    fun fixesTrailingDotSpaceAndLeadingDot() {
        assertEquals("Mr_", LibraryFormat.sanitizeComponent("Mr."))
        assertEquals("_..And Justice", LibraryFormat.sanitizeComponent("...And Justice"))
        assertEquals("x", LibraryFormat.sanitizeComponent("  x  "))
        assertEquals("_", LibraryFormat.sanitizeComponent(""))
        assertEquals("_", LibraryFormat.sanitizeComponent("."))
    }

    @Test
    fun limitsComponentTo120BytesOnCharBoundary() {
        val s = LibraryFormat.sanitizeComponent("é".repeat(100)) // 200 bytes
        assertTrue(s.toByteArray(Charsets.UTF_8).size <= LibraryFormat.MAX_COMPONENT_BYTES)
        assertEquals("é".repeat(60), s)
    }

    @Test
    fun fileNameKeepsIdSuffix() {
        val name = LibraryFormat.sanitizeFileName("a".repeat(300), " [dQw4w9WgXcQ].opus")
        assertEquals(LibraryFormat.MAX_COMPONENT_BYTES, name.toByteArray(Charsets.UTF_8).size)
        assertTrue(name.endsWith(" [dQw4w9WgXcQ].opus"))
        assertEquals("What_ Now..m3u8", LibraryFormat.sanitizeFileName("What? Now.", ".m3u8"))
        assertEquals("Song [x].mp3", LibraryFormat.sanitizeFileName("Song", " [x].mp3"))
    }

    @Test
    fun sanitisingIsIdempotent() {
        for (s in listOf("Mr.", "a/b", " .x. ", "Ünïcødé ☃", "ß".repeat(90))) {
            val once = LibraryFormat.sanitizeComponent(s)
            assertEquals(once, LibraryFormat.sanitizeComponent(once))
        }
    }

    /** Rust's `char::is_control` (Cc) includes C1 controls; `str::trim` removes Unicode White_Space only. */
    @Test
    fun matchesRustCharacterClasses() {
        assertEquals("a_b_c_d", LibraryFormat.sanitizeComponent("a\u0080b\u0085c\u009fd"))
        assertEquals("a_b_c", LibraryFormat.sanitizeComponent("a\u007fb\u001fc"))
        assertEquals("line_break", LibraryFormat.sanitizeComponent("line\nbreak\r"))
        // U+001C-001F are not White_Space: kept and replaced, where Kotlin's trim() would drop them.
        assertEquals("_x_", LibraryFormat.sanitizeComponent("\u001cx\u001f"))
        // U+0085, NBSP and the ideographic space are White_Space: trimmed at the ends.
        assertEquals("x", LibraryFormat.sanitizeComponent("\u0085\u00a0x\u3000\u2029"))
        // Format characters (Cf) such as the zero-width joiner stay, as in Rust.
        assertEquals("a\u200db", LibraryFormat.sanitizeComponent("a\u200db"))
        assertEquals("🎵_", LibraryFormat.sanitizeComponent("🎵\ud800"))
        assertEquals("🎵".repeat(30), LibraryFormat.sanitizeComponent("🎵".repeat(40)))
    }

    @Test
    fun playlistFileNamesMatchTheDesktop() {
        assertEquals("What_ Now..m3u8", LibraryFormat.playlistFileName("What? Now."))
        assertEquals("Road trip.m3u8", LibraryFormat.playlistFileName("  Road trip "))
        assertEquals("_hidden.m3u8", LibraryFormat.playlistFileName(".hidden"))
        assertEquals("_.m3u8", LibraryFormat.playlistFileName(" "))
        val long = LibraryFormat.playlistFileName("x".repeat(300))
        assertEquals(LibraryFormat.MAX_COMPONENT_BYTES, long.toByteArray().size)
        assertTrue(long.endsWith(".m3u8"))
    }

    @Test
    fun audioPathRules() {
        assertTrue(LibraryFormat.isAudioPath("Artist/Album/Song [id].opus"))
        assertTrue(LibraryFormat.isAudioPath("loose.MP3"))
        assertTrue(LibraryFormat.isAudioPath("a/b/c/d/deep.FLAC"))
        assertFalse(LibraryFormat.isAudioPath("Artist/Album/cover.jpg"))
        assertFalse(LibraryFormat.isAudioPath("Playlists/x.mp3"))
        assertFalse(LibraryFormat.isAudioPath(".tunemesh/requests/x.opus"))
        assertFalse(LibraryFormat.isAudioPath("Artist/.hidden/song.mp3"))
        assertFalse(LibraryFormat.isAudioPath("Artist/Album/.song.mp3"))
        assertFalse(LibraryFormat.isAudioPath("Artist/Album/Song.sync-conflict-20261006-101010-ABCDEFG.opus"))
        assertTrue(LibraryFormat.isAudioPath("Other/Playlists/song.ogg"))
    }

    @Test
    fun namesWithLineBreaksAreNotInTheLibrary() {
        assertFalse(LibraryFormat.isAudioPath("Artist/Album/Two\nLines.mp3"))
        assertFalse(LibraryFormat.isAudioPath("Artist/Album/Carriage\rReturn.mp3"))
        assertFalse(LibraryFormat.isAudioPath("Art\nist/Album/Song.mp3"))
        assertTrue(LibraryFormat.isExcludedDirectory("Art\nist"))
        assertTrue(LibraryFormat.isExcludedDirectory("Artist/Al\rbum"))
        assertFalse(LibraryFormat.isExcludedDirectory("Artist/Album"))
        // Other control characters are not line breaks: such files stay in the library.
        assertTrue(LibraryFormat.isAudioPath("Artist/Album/Tab\tHere.mp3"))
    }

    @Test
    fun metadataFallbacks() {
        val nested = Track.fromTags("Daft Punk/Discovery/One More Time [FGBhQbmPwH8].opus", RawTags(), 1, 2)
        assertEquals("One More Time", nested.title)
        assertEquals("Discovery", nested.album)
        assertEquals("Daft Punk", nested.artist)
        assertEquals("Daft Punk", nested.albumArtist)

        val loose = Track.fromTags("loose song.mp3", RawTags(), 1, 2)
        assertEquals("loose song", loose.title)
        assertEquals(LibraryFormat.UNKNOWN_ALBUM, loose.album)
        assertEquals(LibraryFormat.UNKNOWN_ARTIST, loose.artist)

        val oneLevel = Track.fromTags("Singles/x.flac", RawTags(), 1, 2)
        assertEquals("Singles", oneLevel.album)
        assertEquals(LibraryFormat.UNKNOWN_ARTIST, oneLevel.artist)

        val tagged =
            Track.fromTags(
                "a/b/c.opus",
                RawTags(title = "  Real  ", artist = "Artist One; Artist Two", album = "", albumArtist = null),
                1,
                2,
            )
        assertEquals("Real", tagged.title)
        assertEquals("b", tagged.album)
        assertEquals("Artist One; Artist Two", tagged.artist)
        assertEquals("Artist One", tagged.albumArtist)

        val withAlbumArtist = Track.fromTags("a/b/c.opus", RawTags(artist = "Feat", albumArtist = "Various Artists"), 1, 2)
        assertEquals("Various Artists", withAlbumArtist.albumArtist)
    }

    @Test
    fun titleKeepsBracketsThatAreTheWholeName() {
        assertEquals("[untitled]", LibraryFormat.titleFromFileName("[untitled].mp3"))
        assertEquals("Song", LibraryFormat.titleFromFileName("Song [abc-_12].webm"))
        assertEquals("no extension", LibraryFormat.titleFromFileName("no extension"))
        assertEquals(".hidden", LibraryFormat.titleFromFileName(".hidden"))
        assertEquals("a []", LibraryFormat.titleFromFileName("a [].mp3"))
        assertEquals("a", LibraryFormat.titleFromFileName("a\u00a0\u2003[x].mp3")) // Unicode White_Space, as Rust's trim_end
        assertEquals("a [x] [y", LibraryFormat.titleFromFileName("a [x] [y"))
        assertEquals("  [x]", LibraryFormat.titleFromFileName("  [x].opus"))
    }

    @Test
    fun searchIsCaseAndDiacriticInsensitive() {
        val tracks =
            listOf(
                Track.fromTags("Björk/Début/Human Behaviour.opus", RawTags(), 1, 1),
                Track.fromTags("Sigur Rós/Takk/Hoppípolla.opus", RawTags(), 1, 2),
            )
        val index = LibraryIndex(tracks)
        assertEquals(listOf("Human Behaviour"), index.searchTracks("bjork debut").map { it.title })
        assertEquals(listOf("Hoppípolla"), index.searchTracks("HOPPIPOLLA").map { it.title })
        assertEquals(listOf("Sigur Rós"), index.searchArtists("ros").map { it.name })
        assertEquals(listOf("Début"), index.searchAlbums("DÉBUT").map { it.title })
        assertTrue(index.searchTracks("   ").isEmpty())
    }

    @Test
    fun albumsGroupByAlbumArtistAndAlbum() {
        val tracks =
            listOf(
                Track.fromTags(
                    "x/Hits/1.opus",
                    RawTags(title = "One", artist = "A", album = "Hits", albumArtist = "VA", trackNumber = 2),
                    1,
                    10,
                ),
                Track.fromTags(
                    "x/Hits/2.opus",
                    RawTags(title = "Two", artist = "B", album = "Hits", albumArtist = "VA", trackNumber = 1),
                    1,
                    30,
                ),
                Track.fromTags("y/Hits/3.opus", RawTags(title = "Three", artist = "C", album = "Hits"), 1, 20),
            )
        val index = LibraryIndex(tracks)
        assertEquals(2, index.albums.size)
        val va = index.albums.first { it.artist == "VA" }
        assertEquals(listOf("Two", "One"), va.tracks.map { it.title })
        assertEquals(30, va.addedMs)
        assertEquals(listOf("C", "VA"), index.artists.map { it.name })
        assertEquals("VA", index.recentlyAdded.first().artist)
        assertEquals(listOf("VA", "C"), index.recentArtists.map { it.name })
    }
}
