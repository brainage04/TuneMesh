package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LibraryFormatTest {
    @Test
    fun sanitiseReplacesForbiddenAndControlCharacters() {
        assertEquals("a_b_c_d_e_f_g_h_i_j_k", LibraryFormat.sanitizeComponent("a/b\\c:d*e?f\"g<h>i|j\u0001k"))
        assertEquals("tab_here_", LibraryFormat.sanitizeComponent("tab\there\u007f"))
    }

    @Test
    fun sanitiseReplacesTrailingDotsAndSpaces() {
        assertEquals("Hello wor__", LibraryFormat.sanitizeComponent("Hello wor. "))
        assertEquals("Mr_", LibraryFormat.sanitizeComponent("Mr."))
        assertEquals("_", LibraryFormat.sanitizeComponent(""))
        assertEquals("_hidden", LibraryFormat.sanitizeComponent(".hidden"))
        assertEquals("_", LibraryFormat.sanitizeComponent("."))
    }

    @Test
    fun sanitiseLimitsUtf8BytesWithoutSplittingCharacters() {
        val name = "é".repeat(100) // 200 bytes
        val result = LibraryFormat.sanitizeComponent(name)
        assertEquals(60, result.length)
        assertTrue(result.toByteArray(Charsets.UTF_8).size <= 120)
        val emoji = "🎵".repeat(40) // 4 bytes each
        assertEquals(30 * 2, LibraryFormat.sanitizeComponent(emoji).length)
        assertTrue(LibraryFormat.playlistFileName("x".repeat(300)).toByteArray().size <= 120)
        assertTrue(LibraryFormat.playlistFileName("x".repeat(300)).endsWith(".m3u8"))
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

        val tagged = Track.fromTags(
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
    }

    @Test
    fun searchIsCaseAndDiacriticInsensitive() {
        val tracks = listOf(
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
        val tracks = listOf(
            Track.fromTags("x/Hits/1.opus", RawTags(title = "One", artist = "A", album = "Hits", albumArtist = "VA", trackNumber = 2), 1, 10),
            Track.fromTags("x/Hits/2.opus", RawTags(title = "Two", artist = "B", album = "Hits", albumArtist = "VA", trackNumber = 1), 1, 30),
            Track.fromTags("y/Hits/3.opus", RawTags(title = "Three", artist = "C", album = "Hits"), 1, 20),
        )
        val index = LibraryIndex(tracks)
        assertEquals(2, index.albums.size)
        val va = index.albums.first { it.artist == "VA" }
        assertEquals(listOf("Two", "One"), va.tracks.map { it.title })
        assertEquals(30, va.addedMs)
        assertEquals(listOf("C", "VA"), index.artists.map { it.name })
        assertEquals("VA", index.recentlyAdded.first().artist)
    }
}
