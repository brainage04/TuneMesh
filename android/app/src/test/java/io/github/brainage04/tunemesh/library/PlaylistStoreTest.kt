package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class PlaylistStoreTest {
    @get:Rule
    val temp = TemporaryFolder()

    private fun track(path: String) = Track(path, path.substringAfterLast('/').substringBefore('.'), "Artist", "Album", "Artist", null, null, 60_000, 1, 1)

    private val a = track("Artist/Album/A [1].opus")
    private val b = track("Artist/Album/B [2].opus")
    private val c = track("Artist/Album/C [3].opus")
    private val tracks = listOf(a, b, c).associateBy { it.path }

    private fun store(root: File = temp.root) = PlaylistStore(root, lookup = tracks::get)

    private fun liked(root: File = temp.root) = File(root, "Playlists/Liked Songs.m3u8").readText()

    @Test
    fun likeInsertsAtTopAndCreatesFile() {
        val store = store()
        store.setLiked(a, true)
        store.setLiked(b, true)
        assertEquals(
            "#EXTM3U\n#EXTINF:60,Artist - B [2]\n../Artist/Album/B [2].opus\n#EXTINF:60,Artist - A [1]\n../Artist/Album/A [1].opus\n",
            liked(),
        )
        store.setLiked(a, true)
        assertEquals(listOf(b.path, a.path), store.read(LibraryFormat.LIKED_SONGS)!!.entries.map { it.path })
    }

    @Test
    fun unlikeRemovesEveryEntryAndKeepsMissingOnes() {
        File(temp.root, "Playlists").mkdirs()
        File(temp.root, "Playlists/Liked Songs.m3u8").writeText(
            "#EXTM3U\n../Artist/Album/A [1].opus\n#EXTINF:42,Gone - Away\n../Gone/X/Away [9].opus\n../Artist/Album/B [2].opus\n../Artist/Album/A [1].opus\n",
        )
        val store = store()
        store.setLiked(a, false)
        assertEquals(
            "#EXTM3U\n#EXTINF:42,Gone - Away\n../Gone/X/Away [9].opus\n#EXTINF:60,Artist - B [2]\n../Artist/Album/B [2].opus\n",
            liked(),
        )
        assertFalse(a.path in store.likedPaths())
        assertTrue(b.path in store.likedPaths())
    }

    @Test
    fun writesAreAtomicAndLeaveNoTemporaryFiles() {
        val store = store()
        val name = store.create("Road trip")
        store.add(name, listOf(a, b, c, a))
        store.move(name, EntryRef(0, a.path), EntryRef(2, c.path))
        store.remove(name, EntryRef(1, c.path))
        assertEquals(listOf(b.path, a.path, a.path), store.read(name)!!.entries.map { it.path })
        assertEquals(listOf("Road trip.m3u8"), File(temp.root, "Playlists").list()!!.toList())
    }

    @Test
    fun removeFallsBackToTheKeyWhenThePositionIsStale() {
        val store = store()
        val name = store.create("List")
        store.add(name, listOf(a, b, c))
        store.remove(name, EntryRef(0, c.path))
        assertEquals(listOf(a.path, b.path), store.read(name)!!.entries.map { it.path })
    }

    /** R2-astra-2, first case: another device reordered the playlist between showing and "Move up". */
    @Test
    fun moveFindsTheShownEntriesInTheChangedFile() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        val file = File(dir, "Mix.m3u8")
        file.writeText("../A.wav\n../B.wav\n../C.wav\n")
        val store = store()
        val shown = store.contents("Mix")!!.entries
        file.writeText("../C.wav\n../A.wav\n../B.wav\n")
        store.move("Mix", EntryRef(2, shown[2].key), EntryRef(1, shown[1].key)) // "Move up" on C
        // C (now first) moves to where B is now, as on the desktop; B is not the one moved.
        assertEquals(listOf("A.wav", "B.wav", "C.wav"), store.read("Mix")!!.entries.map { it.path })
    }

    /** R2-astra-2, second case: entries outside the library are told apart by their line, not a shared null. */
    @Test
    fun removeTellsOutsideEntriesApartByTheirLine() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        val file = File(dir, "Mix.m3u8")
        file.writeText("/outside/A.wav\n/outside/B.wav\n")
        val store = store()
        val shown = store.contents("Mix")!!.entries
        assertEquals(listOf(null, null), shown.map { it.path })
        file.writeText("/outside/B.wav\n/outside/A.wav\n")
        store.remove("Mix", EntryRef(1, shown[1].key))
        assertEquals(listOf("/outside/A.wav"), store.read("Mix")!!.entries.map { it.raw })
    }

    @Test
    fun changesToEntriesThatAreGoneFailWithoutWriting() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        val file = File(dir, "Mix.m3u8")
        file.writeText("../A.wav\n../B.wav\n")
        val store = store()
        val shown = store.contents("Mix")!!.entries
        file.writeText("../B.wav\n")
        val before = file.readText()
        assertThrows(PlaylistException::class.java) { store.remove("Mix", EntryRef(0, shown[0].key)) }
        assertThrows(PlaylistException::class.java) { store.move("Mix", EntryRef(1, shown[1].key), EntryRef(0, shown[0].key)) }
        assertEquals(before, file.readText())
        // Two shown rows with the same key are two different entries.
        file.writeText("../A.wav\n../A.wav\n../B.wav\n")
        store.move("Mix", EntryRef(2, "B.wav"), EntryRef(0, "A.wav"))
        assertEquals(listOf("B.wav", "A.wav", "A.wav"), store.read("Mix")!!.entries.map { it.path })
        assertEquals(listOf(1, 2), PlaylistStore.locate(store.read("Mix")!!.entries, listOf(EntryRef(1, "A.wav"), EntryRef(1, "A.wav"))))
    }

    @Test
    fun likedSongsCannotBeRenamedOrDeleted() {
        val store = store()
        store.setLiked(a, true)
        assertThrows(PlaylistException::class.java) { store.rename(LibraryFormat.LIKED_SONGS, "Other") }
        assertThrows(PlaylistException::class.java) { store.delete(LibraryFormat.LIKED_SONGS) }
        assertTrue(File(temp.root, "Playlists/Liked Songs.m3u8").exists())
    }

    @Test
    fun createSanitisesAndRenameMovesTheFile() {
        val store = store()
        val name = store.create("AC/DC: Best?")
        assertEquals("AC_DC_ Best_", name)
        assertThrows(PlaylistException::class.java) { store.create("AC/DC: Best?") }
        assertEquals("Best of AC_DC", store.rename(name, "Best of AC/DC"))
        assertEquals(listOf("Best of AC_DC"), store.list().map { it.name })
        store.delete("Best of AC_DC")
        assertTrue(store.list().isEmpty())
    }

    @Test
    fun namesNeverDifferOnlyInCase() {
        val store = store()
        store.create("Road Trip")
        assertThrows(PlaylistException::class.java) { store.create("road trip") }
        // Liked Songs is reserved even before its file exists.
        assertThrows(PlaylistException::class.java) { store.create("LIKED SONGS") }
        store.create("Drive")
        assertThrows(PlaylistException::class.java) { store.rename("Drive", "ROAD trip") }
        assertThrows(PlaylistException::class.java) { store.rename("Drive", "liked songs") }
        // Changing only the case of its own name is a rename, not a clash.
        assertEquals("road trip", store.rename("Road Trip", "road trip"))
        assertEquals(listOf("Drive", "road trip"), store.list().map { it.name })
    }

    @Test
    fun listIgnoresConflictCopiesAndPutsLikedSongsFirst() {
        val store = store()
        store.create("Zebra")
        store.create("apple")
        store.setLiked(a, true)
        File(temp.root, "Playlists/Zebra.sync-conflict-20261006-120000-ABCDEF.m3u8").writeText("#EXTM3U\n")
        assertEquals(listOf("Liked Songs", "apple", "Zebra"), store.list().map { it.name })
    }

    @Test
    fun likeAndUnlikeHelpers() {
        val existing = listOf(M3u8.entryFor(b), M3u8.entryFor(a), M3u8.entryFor(b))
        assertEquals(listOf(c.path, b.path, a.path, b.path), PlaylistStore.like(existing, c).map { it.path })
        assertEquals(existing, PlaylistStore.like(existing, a))
        assertEquals(listOf(a.path), PlaylistStore.unlike(existing, b.path).map { it.path })
    }

    @Test
    fun tracksWithLineBreaksInTheirPathAreNeverWritten() {
        val broken = track("Artist/Album/Two\nLines [4].opus")
        val carriage = track("Art\rist/Album/D [5].opus")
        val store = store()
        store.create("Mix")
        store.add("Mix", listOf(a, broken, carriage, b))
        assertEquals(listOf(a.path, b.path), store.read("Mix")!!.entries.map { it.path })
        store.setLiked(broken, true)
        assertEquals("#EXTM3U\n", liked())
        assertEquals(emptyList<PlaylistEntry>(), PlaylistStore.like(emptyList(), carriage))
    }

    @Test
    fun extinfNeverBreaksTheLine() {
        val tagged = Track("A/B/x.opus", "Two\nline\rtitle", "Art\r\nist", "B", "B", null, null, 1_000, 1, 1)
        assertEquals("#EXTINF:1,Art  ist - Two line title", M3u8.extinf(tagged))
    }

    @Test
    fun listKeepsSummariesNotEntries() {
        val store = store()
        val name = store.create("Mix")
        store.add(name, listOf(c, b, a, c))
        File(temp.root, "Playlists/Mix.m3u8").appendText("../Gone/x.opus\n")
        val mix = store.list().single()
        assertEquals(5, mix.size)
        assertEquals(listOf(c.path, b.path, a.path, c.path), mix.artPaths)
        assertSame(tracks.getValue(c.path).path, mix.artPaths[0])
        assertNull(mix.problem)
        val many = (1..20).map { track("Artist/Album/T$it [$it].opus") }
        val big = PlaylistStore(temp.root) { path -> many.firstOrNull { it.path == path } }
        big.add("Big", many)
        assertEquals(PlaylistStore.ART_PATHS, big.list().first { it.name == "Big" }.artPaths.size)
    }

    @Test
    fun aPlaylistBeyondTheLimitsIsListedButNeverRewritten() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        val huge = File(dir, "Huge.m3u8")
        huge.outputStream().buffered().use { out ->
            out.write("#EXTM3U\n".toByteArray())
            repeat(Limits.MAX_PLAYLIST_ENTRIES + 1) { out.write("../missing.wav\n".toByteArray()) }
        }
        val before = huge.readBytes()
        val store = store()
        val listed = store.list().single()
        assertEquals("Huge", listed.name)
        assertTrue(listed.problem!!.contains("more than ${Limits.MAX_PLAYLIST_ENTRIES} entries"))
        assertThrows(PlaylistTooLargeException::class.java) { store.contents("Huge") }
        assertThrows(PlaylistException::class.java) { store.add("Huge", listOf(a)) }
        assertThrows(PlaylistException::class.java) { store.remove("Huge", EntryRef(0, "missing.wav")) }
        assertThrows(PlaylistException::class.java) { store.move("Huge", EntryRef(0, "missing.wav"), EntryRef(1, "missing.wav")) }
        assertTrue(before.contentEquals(huge.readBytes()))

        File(dir, "Liked Songs.m3u8").writeBytes(before)
        assertThrows(PlaylistException::class.java) { store.setLiked(a, true) }
        assertThrows(PlaylistTooLargeException::class.java) { store.likedPaths() }
        assertTrue(before.contentEquals(File(dir, "Liked Songs.m3u8").readBytes()))
    }

    @Test
    fun aFileLargerThanTheByteLimitIsNotRead() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        java.io.RandomAccessFile(File(dir, "Sparse.m3u8"), "rw").use { it.setLength(Limits.MAX_PLAYLIST_BYTES + 1) }
        assertTrue(store().list().single().problem!!.contains("larger than 32 MiB"))
    }

    @Test
    fun contentsLeaveOutExtinfAndReadKeepsIt() {
        val store = store()
        store.add("Mix", listOf(a))
        assertNull(store.contents("Mix")!!.entries.single().extinf)
        assertEquals("#EXTINF:60,Artist - A [1]", store.read("Mix")!!.entries.single().extinf)
        assertNull(store.contents("Nothing"))
        assertEquals(emptySet<String>(), store.likedPaths())
    }

    /** docs/library-format.md: writers never write over the limits; adding to a full playlist fails. */
    @Test
    fun addingToAFullPlaylistFailsAndLeavesItAsItIs() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        val full = File(dir, "Full.m3u8")
        full.outputStream().buffered().use { out ->
            out.write("#EXTM3U\n".toByteArray())
            repeat(Limits.MAX_PLAYLIST_ENTRIES) { out.write("../missing.wav\n".toByteArray()) }
        }
        val before = full.readBytes()
        val store = store()
        assertEquals(Limits.MAX_PLAYLIST_ENTRIES, store.list().single().size)
        val error = assertThrows(PlaylistException::class.java) { store.add("Full", listOf(a)) }
        assertTrue(error.message!!.contains("would be too large"))
        assertTrue(before.contentEquals(full.readBytes()))
        store.remove("Full", EntryRef(0, "missing.wav"))
        assertEquals(Limits.MAX_PLAYLIST_ENTRIES - 1, store.list().single().size)
    }

    /** "Limits": Liked Songs and the first 1,000 other playlists by name; the rest are not read. */
    @Test
    fun listsLikedSongsAndTheFirstThousandByName() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        for (n in 0 until Limits.MAX_PLAYLISTS_LISTED + 50) File(dir, "p%04d.m3u8".format(n)).writeText("../A.wav\n")
        File(dir, "Liked Songs.m3u8").writeText("../A.wav\n")
        File(dir, "Zzz.m3u8").writeText("../A.wav\n")
        File(dir, "aaa.m3u8").writeText("../A.wav\n")
        val listed = store().list()
        assertEquals(Limits.MAX_PLAYLISTS_LISTED + 1, listed.size)
        assertEquals(listOf("Liked Songs", "aaa", "p0000"), listed.take(3).map { it.name })
        assertEquals("p0998", listed.last().name)
        assertFalse(listed.any { it.name == "Zzz" })
    }

    @Test
    fun unchangedPlaylistsAreNotReadAgain() {
        val dir = File(temp.root, "Playlists").apply { mkdirs() }
        val file = File(dir, "Mix.m3u8").apply { writeText("../A.wav\n") }
        val summaries = PlaylistSummaries()
        var reads = 0
        val lookup: (String) -> Track? = { reads++; null }
        val store = PlaylistStore(temp.root, summaries, lookup)
        fun readsFor(block: () -> Unit): Int {
            val before = reads
            block()
            return reads - before
        }
        assertTrue(readsFor { assertEquals(1, store.list().single().size) } > 0)
        assertEquals(0, readsFor { assertEquals(1, store.list().single().size) })
        file.writeText("../A.wav\n../B.wav\n")
        assertTrue(readsFor { assertEquals(2, store.list().single().size) } > 0)
        assertEquals(0, readsFor { store.list() })
        // Another lookup (a new library index) reads again, and so does everything after clear().
        assertTrue(readsFor { PlaylistStore(temp.root, summaries) { reads++; null }.list() } > 0)
        summaries.clear()
        assertTrue(readsFor { store.list() } > 0)
    }
}
