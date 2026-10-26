package io.github.brainage04.tunemesh.library

import java.io.ByteArrayOutputStream
import kotlin.random.Random
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The compact library index against the plain lists it replaced, its saved form, and the scanner's reuse of unchanged tracks. */
class LibraryIndexTest {
    private val random = Random(7)

    private val words =
        listOf("Début", "debut", "Hoppípolla", "Ágætis", "byrjun", "Σίγμα", "🎵 Notes", "a", "B", "c", "Live", "live", "Ωmega")

    private fun text() = List(random.nextInt(1, 3)) { words.random(random) }.joinToString(" ")

    /** Tracks with shared artists and albums, case variants, missing numbers and non-ASCII texts. */
    private fun library(count: Int): List<Track> =
        List(count) { i ->
            val artist = "Artist ${random.nextInt(12)}" + if (random.nextInt(5) == 0) " Ünï" else ""
            val album = "Album ${text()}"
            Track(
                path = "$artist/$album/${i.toString().padStart(4, '0')} ${text()}.mp3",
                title = text(),
                artist = artist,
                album = album,
                albumArtist = if (random.nextInt(4) == 0) "Various" else artist,
                trackNumber = random.nextInt(20).takeIf { it > 2 },
                discNumber = random.nextInt(3).takeIf { it > 0 },
                durationMs = random.nextLong(1_000, 600_000),
                size = random.nextLong(1, 1 shl 30),
                modifiedMs = random.nextLong(1, 2_000_000_000_000),
            )
        }

    private val trackOrder: Comparator<Track> =
        compareBy<Track>({ it.discNumber ?: 0 }, { it.trackNumber ?: Int.MAX_VALUE }).thenBy(String.CASE_INSENSITIVE_ORDER) { it.path }

    /** What the list-based index did, for the parts whose order is defined. */
    @Test
    fun matchesTheListIndex() {
        val tracks = library(1500)
        val index = LibraryIndex.of(tracks)
        assertEquals(tracks.size, index.size)
        assertEquals(tracks.toSet(), index.tracks.toSet())
        for (track in tracks) assertEquals(track, index.track(track.path))
        assertNull(index.track("Nope/x.mp3"))
        assertEquals(-1, index.indexOf("Nope/x.mp3"))

        val byKey = tracks.groupBy { it.albumKey }
        assertEquals(byKey.size, index.albums.size)
        val titles = index.albums.map { it.title }
        assertEquals(titles.sortedWith(String.CASE_INSENSITIVE_ORDER), titles)
        for (album in index.albums) {
            assertEquals(byKey.getValue(album.key).sortedWith(trackOrder), album.tracks)
            assertEquals(album.tracks.size, album.trackCount)
            assertEquals(album.tracks.maxOf { it.modifiedMs }, album.addedMs)
            assertEquals(album.tracks.sumOf { it.durationMs }, album.durationMs)
            assertEquals(album, index.album(album.key))
        }
        assertNull(index.album("nobody\u0000nothing"))
        val added = index.recentlyAdded.map { it.addedMs }
        assertEquals(added.sortedDescending(), added)
        assertEquals(index.albums.toSet(), index.recentlyAdded.toSet())

        val byArtist = index.albums.groupBy { it.artist }
        assertEquals(byArtist.keys.sortedWith(String.CASE_INSENSITIVE_ORDER), index.artists.map { it.name })
        for (artist in index.artists) {
            assertEquals(byArtist.getValue(artist.name), artist.albums)
            assertEquals(artist.albums.flatMap { it.tracks }, artist.tracks)
            assertEquals(artist, index.artist(artist.name))
        }
        val newest = index.artists.associateWith { artist -> artist.albums.maxOf { it.addedMs } }
        assertEquals(
            index.artists.sortedByDescending { newest.getValue(it) }.take(LibraryIndex.HOME_ROW),
            index.recentArtists,
        )
    }

    @Test
    fun searchFindsWhatTheListIndexFound() {
        val tracks = library(800)
        val index = LibraryIndex.of(tracks)
        for (query in listOf("debut", "DÉBUT a", "σίγμα", "🎵", "live b", "artist 3", "various", "ü", "nothing at all", "  ")) {
            val words = LibraryFormat.searchKey(query).split(Regex("\\s+")).filter { it.isNotEmpty() }
            fun matches(text: String) = words.isNotEmpty() && LibraryFormat.searchKey(text).let { key -> words.all { key.contains(it) } }
            val expected = tracks.filter { matches("${it.title}\n${it.artist}\n${it.album}\n${it.albumArtist}") }
            val found = index.searchTracks(query)
            assertEquals(query, expected.toSet(), found.toSet())
            assertEquals(query, found.map { it.title }.sortedWith(String.CASE_INSENSITIVE_ORDER), found.map { it.title })
            assertEquals(query, index.albums.filter { matches("${it.title}\n${it.artist}") }, index.searchAlbums(query))
            assertEquals(query, index.artists.filter { matches(it.name) }, index.searchArtists(query))
        }
    }

    @Test
    fun emptyIndex() {
        val empty = LibraryIndex.EMPTY
        assertEquals(0, empty.size)
        assertTrue(empty.tracks.isEmpty() && empty.albums.isEmpty() && empty.artists.isEmpty() && empty.recentArtists.isEmpty())
        assertNull(empty.track("a.mp3"))
        assertNull(empty.album("a\u0000b"))
        assertNull(empty.artist("a"))
        assertTrue(empty.searchTracks("a").isEmpty())
    }

    @Test
    fun savedIndexReadsBackForItsRootOnly() {
        val index = LibraryIndex.of(library(500))
        val bytes = ByteArrayOutputStream().also { LibraryIndexFile.write(index, "/lib", it) }.toByteArray()
        fun read(data: ByteArray, root: String = "/lib") = LibraryIndexFile.read(data.inputStream(), root, data.size.toLong())
        val back = read(bytes)!!
        assertEquals(index.tracks, back.tracks)
        assertEquals(index.albums.map { it.key }, back.albums.map { it.key })
        assertEquals(index.artists.map { it.name }, back.artists.map { it.name })
        assertEquals(index.searchTracks("debut"), back.searchTracks("debut"))
        assertNull(read(bytes, "/other"))
        assertNull(read(bytes.copyOf(bytes.size - 1)))
        assertNull(read(bytes + 0))
        for (at in listOf(20, bytes.size / 2, bytes.size - 5)) {
            val damaged = bytes.copyOf().also { it[at] = (it[at] + 1).toByte() }
            assertNull("byte $at", read(damaged))
        }
        val emptyBytes = ByteArrayOutputStream().also { LibraryIndexFile.write(LibraryIndex.EMPTY, "/lib", it) }.toByteArray()
        assertEquals(0, read(emptyBytes)!!.size)
    }

    @Test
    fun rescanKeepsUnchangedTracksAndTheIndexWhenNothingChanged() {
        val tracks = library(300)
        val previous = LibraryIndex.of(tracks)
        fun found(of: List<Track>) = FoundFiles().apply { of.forEach { add(it.path, it.size, it.modifiedMs) } }
        val read = ArrayList<String>()
        fun tags(path: String): RawTags {
            read += path
            return RawTags(title = "Read $path")
        }
        assertSame(previous, LibraryScan.index(previous, found(tracks.shuffled(random)), ::tags))
        assertTrue(read.isEmpty())

        val changed = tracks[5].copy(modifiedMs = tracks[5].modifiedMs + 1)
        val added = Track.fromTags("New/Album/n.mp3", RawTags(title = "Read New/Album/n.mp3"), 9, 9)
        val now = tracks.drop(1).map { if (it.path == changed.path) changed else it } + added
        val index = LibraryScan.index(previous, found(now), ::tags)
        assertEquals(setOf(changed.path, added.path), read.toSet())
        assertNull(index.track(tracks[0].path))
        assertEquals("Read ${changed.path}", index.track(changed.path)?.title)
        assertEquals(added, index.track(added.path))
        for (track in tracks.drop(1).filter { it.path != changed.path }) assertEquals(track, index.track(track.path))
        // Kept tracks keep their search keys.
        assertEquals(
            previous.searchTracks("debut").filter { it.path != tracks[0].path && it.path != changed.path }.toSet(),
            index.searchTracks("debut").filter { it.path != changed.path }.toSet(),
        )
    }

    @Test
    fun theJsonTagCacheOfEarlierVersionsIsConverted() {
        val cache =
            LibraryScan.JsonTagCache(
                "/lib",
                mapOf(
                    "A/B/one.mp3" to LibraryScan.JsonTagCacheEntry(10, 20, RawTags(title = "One", artist = "A", trackNumber = 1)),
                    "A/B/two [x].flac" to LibraryScan.JsonTagCacheEntry(11, 21, RawTags()),
                    "A/B/cover.jpg" to LibraryScan.JsonTagCacheEntry(1, 1, RawTags()),
                ),
            )
        val text = Json.encodeToString(LibraryScan.JsonTagCache.serializer(), cache)
        val index = LibraryScan.fromJsonTagCache(text, "/lib")!!
        assertEquals(
            setOf(
                Track.fromTags("A/B/one.mp3", RawTags(title = "One", artist = "A", trackNumber = 1), 10, 20),
                Track.fromTags("A/B/two [x].flac", RawTags(), 11, 21),
            ),
            index.tracks.toSet(),
        )
        assertNull(LibraryScan.fromJsonTagCache(text, "/other"))
        assertNull(LibraryScan.fromJsonTagCache("{", "/lib"))
    }
}
