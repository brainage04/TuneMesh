package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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

    private fun store(root: File = temp.root) = PlaylistStore(root, tracks::get)

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
        assertFalse(store.isLiked(a.path))
        assertTrue(store.isLiked(b.path))
    }

    @Test
    fun writesAreAtomicAndLeaveNoTemporaryFiles() {
        val store = store()
        val name = store.create("Road trip")
        store.add(name, listOf(a, b, c, a))
        store.move(name, 0, 2)
        store.removeAt(name, 1, c.path)
        assertEquals(listOf(b.path, a.path, a.path), store.read(name)!!.entries.map { it.path })
        assertEquals(listOf("Road trip.m3u8"), File(temp.root, "Playlists").list()!!.toList())
    }

    @Test
    fun removeAtFallsBackToPathWhenIndexIsStale() {
        val store = store()
        val name = store.create("List")
        store.add(name, listOf(a, b, c))
        store.removeAt(name, 0, c.path)
        assertEquals(listOf(a.path, b.path), store.read(name)!!.entries.map { it.path })
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
}
