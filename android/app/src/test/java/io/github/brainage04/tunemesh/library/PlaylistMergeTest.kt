package io.github.brainage04.tunemesh.library

import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/** Contract: "Playlist conflict copies". */
class PlaylistMergeTest {
    @get:Rule val temp = TemporaryFolder()

    private fun track(path: String) =
        Track(path, path.substringAfterLast('/').substringBefore('.'), "Artist", "Album", "Artist", null, null, 60_000, 1, 1)

    private val a = track("Artist/Album/A.opus")
    private val b = track("Artist/Album/B.opus")
    private val c = track("Artist/Album/C.opus")
    private val d = track("Artist/Album/D.opus")
    private val tracks = listOf(a, b, c, d).associateBy { it.path }
    private val now = 1_800_000_000_000L

    /** The shared cases in docs/test-vectors/playlist-merge/, byte for byte as tunemesh-core runs them. */
    @Test
    fun sharedTestVectors() {
        val cases = File(vectors, ".").listFiles { file -> file.isDirectory }!!.sortedBy { it.name }
        assertTrue("found ${cases.size} cases in $vectors", cases.size >= 8)
        for (case in cases) {
            fun parse(name: String): PlaylistEntries? {
                val file = File(case, "$name.m3u8")
                if (!file.isFile) return null
                val entries = PlaylistEntries.Builder(ROOT, keepExtinf = true)
                file.inputStream().use { M3u8.read(it, ROOT, keepExtinf = true) { entry -> entries.add(entry) } }
                return entries.build()
            }
            val merged = PlaylistMerge.merge(parse("base"), parse("winner")!!, parse("copy")!!)
            assertEquals(case.name, File(case, "merged.m3u8").readText(), M3u8.write(merged) { null })
        }
    }

    @Test
    fun conflictCopyNames() {
        assertEquals("Liked Songs", PlaylistMerge.conflictOf("Liked Songs.sync-conflict-20261026-101010-ABCDEF2.m3u8"))
        assertEquals("a.b", PlaylistMerge.conflictOf("a.b.sync-conflict-20261026-101010-ABCDEF2.m3u8"))
        assertNull(PlaylistMerge.conflictOf(".sync-conflict-20261026-101010-ABCDEF2.m3u8"))
        assertNull(PlaylistMerge.conflictOf("x.sync-conflict-20261026-101010-ABCDEF1.m3u8")) // 1 is not base32
        assertNull(PlaylistMerge.conflictOf("x.sync-conflict-20261026-101010-abcdef2.m3u8"))
        assertNull(PlaylistMerge.conflictOf("x.sync-conflict-2026102-101010-ABCDEF2.m3u8"))
        assertNull(PlaylistMerge.conflictOf("x.sync-conflict-20261026-101010-ABCDEF2.m3u8.tmp"))
        assertNull(PlaylistMerge.conflictOf("x.m3u8"))
    }

    /** Two devices, each with its library copy and its own bases; [sync] plays Syncthing. */
    private inner class Device(name: String) {
        val root: File = temp.newFolder(name)
        val bases = File(temp.root, "$name-bases")
        val store = PlaylistStore(root, bases = PlaylistBases(bases), lookup = tracks::get)

        fun file(playlist: String) = File(root, "Playlists/$playlist.m3u8")

        fun paths(playlist: String) = store.read(playlist)!!.entries.map { it.path }
    }

    private var made = 0

    private fun devices(playlist: String, vararg initial: Track): Pair<Device, Device> {
        val one = Device("one-${made++}")
        val two = Device("two-${made++}")
        // A third device made the playlist; both have seen it.
        val text = M3u8.write(initial.map(M3u8::entryFor)) { null }
        for (device in listOf(one, two)) device.file(playlist).apply { parentFile!!.mkdirs() }.writeText(text)
        return one to two
    }

    /**
     * Syncthing after both changed [playlist]: [winner]'s version is the playlist on both devices, the loser's version is a conflict copy
     * named after the loser, old enough to merge. Then both merge; the result must be the same bytes on both.
     */
    private fun conflict(playlist: String, winner: Device, loser: Device): String {
        val won = winner.file(playlist).readBytes()
        val lost = loser.file(playlist).readBytes()
        val copyName = "$playlist.sync-conflict-20261026-101010-LOSERID.m3u8"
        for (device in listOf(winner, loser)) {
            device.file(playlist).writeBytes(won)
            File(device.root, "Playlists/$copyName").apply {
                writeBytes(lost)
                setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
            }
        }
        assertEquals(setOf(playlist), winner.store.mergeConflicts(now))
        assertEquals(setOf(playlist), loser.store.mergeConflicts(now))
        for (device in listOf(winner, loser)) assertFalse(File(device.root, "Playlists/$copyName").exists())
        assertArrayEquals("both devices write the same bytes", winner.file(playlist).readBytes(), loser.file(playlist).readBytes())
        return winner.file(playlist).readText()
    }

    @Test
    fun unlikeOnOneDeviceAndLikeOnTheOther() {
        val (one, two) = devices(LibraryFormat.LIKED_SONGS, a, b)
        one.store.setLiked(b, false)
        two.store.setLiked(c, true)
        conflict(LibraryFormat.LIKED_SONGS, winner = one, loser = two)
        assertEquals(listOf(c.path, a.path), one.paths(LibraryFormat.LIKED_SONGS))
        // The other way round gives the same.
        val (three, four) = devices(LibraryFormat.LIKED_SONGS, a, b)
        three.store.setLiked(b, false)
        four.store.setLiked(c, true)
        conflict(LibraryFormat.LIKED_SONGS, winner = four, loser = three)
        assertEquals(listOf(c.path, a.path), four.paths(LibraryFormat.LIKED_SONGS))
    }

    @Test
    fun removalOnOneDeviceAndReorderOnTheOther() {
        val (one, two) = devices("Mix", a, b, c, d)
        one.store.remove("Mix", EntryRef(1, b.path))
        two.store.move("Mix", EntryRef(3, d.path), EntryRef(0, a.path))
        conflict("Mix", winner = one, loser = two)
        assertEquals(listOf(d.path, a.path, c.path), one.paths("Mix"))
    }

    @Test
    fun bothAdd() {
        val (one, two) = devices("Mix", a, b)
        one.store.add("Mix", listOf(c))
        two.store.add("Mix", listOf(d))
        conflict("Mix", winner = two, loser = one)
        assertEquals(listOf(a.path, b.path, c.path, d.path), two.paths("Mix"))
    }

    /** Several edits in a row keep the base they started from; after a merge the merged playlist is the base. */
    @Test
    fun baseFollowsTheContract() {
        val (one, two) = devices("Mix", a, b, c)
        one.store.remove("Mix", EntryRef(1, b.path))
        one.store.remove("Mix", EntryRef(1, c.path))
        two.store.add("Mix", listOf(d))
        conflict("Mix", winner = two, loser = one)
        assertEquals(listOf(a.path, d.path), one.paths("Mix"))
        // A second round of conflicts merges against the merged playlist.
        one.store.add("Mix", listOf(b))
        two.store.remove("Mix", EntryRef(0, a.path))
        conflict("Mix", winner = one, loser = two)
        assertEquals(listOf(d.path, b.path), one.paths("Mix"))
    }

    /** Without a base (cleared app data, first edit on this device) the merge keeps everything either side has: a removal comes back. */
    @Test
    fun withoutABaseRemovalsComeBack() {
        val (one, two) = devices("Mix", a, b)
        one.store.remove("Mix", EntryRef(1, b.path))
        two.store.add("Mix", listOf(c))
        one.bases.deleteRecursively()
        two.bases.deleteRecursively()
        conflict("Mix", winner = one, loser = two)
        assertEquals(listOf(a.path, b.path, c.path), one.paths("Mix"))
    }

    /** Copies too young, without their playlist, too large or with a merge over the limits stay as they are. */
    @Test
    fun copiesThatAreNotMergedStayAsTheyAre() {
        val one = Device("solo")
        val dir = File(one.root, "Playlists").apply { mkdirs() }
        File(dir, "Mix.m3u8").writeText("#EXTM3U\n../Artist/Album/A.opus\n")
        val young = File(dir, "Mix.sync-conflict-20261026-101010-AAAAAAA.m3u8").apply { writeText("../Artist/Album/B.opus\n") }
        young.setLastModified(now - PlaylistStore.CONFLICT_AGE_MS + 1)
        val orphan = File(dir, "Gone.sync-conflict-20261026-101010-AAAAAAA.m3u8").apply { writeText("../Artist/Album/B.opus\n") }
        orphan.setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
        val huge = File(dir, "Mix.sync-conflict-20261026-101011-AAAAAAA.m3u8")
        java.io.RandomAccessFile(huge, "rw").use { it.setLength(Limits.MAX_PLAYLIST_BYTES + 1) }
        huge.setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
        // R6-Opus-5: the copy over the limits is reported (it holds the base until someone deletes it); the young one is not due yet.
        val stuck = ArrayList<String>()
        assertEquals(emptySet<String>(), one.store.mergeConflicts(now) { stuck += it })
        assertEquals(listOf(huge.name), stuck)
        assertTrue(young.exists() && orphan.exists() && huge.exists())
        assertEquals("#EXTM3U\n../Artist/Album/A.opus\n", File(dir, "Mix.m3u8").readText())
        assertTrue(one.store.hasConflictCopies())
        assertTrue(one.store.list().none { LibraryFormat.isConflictCopy(it.name) })
    }

    /** Copies of one playlist merge one at a time in byte order, all against the base as it was before the first. */
    @Test
    fun severalCopiesMergeInNameOrder() {
        val (one, _) = devices("Mix", a)
        one.store.add("Mix", listOf(b)) // base: [a], this device: [a, b]
        val dir = File(one.root, "Playlists")
        for ((stamp, added) in listOf("101012" to d, "101011" to c)) {
            File(dir, "Mix.sync-conflict-20261026-$stamp-AAAAAAA.m3u8").apply {
                writeText(M3u8.write(listOf(M3u8.entryFor(a), M3u8.entryFor(added))) { null })
                setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
            }
        }
        assertEquals(setOf("Mix"), one.store.mergeConflicts(now))
        assertEquals(listOf(a.path, d.path, c.path, b.path), one.paths("Mix"))
        assertFalse(one.store.hasConflictCopies())
    }

    /**
     * One pass merges at most [PlaylistStore.MAX_COPIES_PER_PASS] copies of a playlist, the first in byte order; the next the rest, against
     * the same base, so their entries are not taken for removals.
     */
    @Test
    fun copiesMergeInBoundedPasses() {
        val (one, _) = devices("Mix", a)
        PlaylistBases(one.bases).merged("Mix.m3u8", one.file("Mix").readBytes())
        val dir = File(one.root, "Playlists")
        val n = PlaylistStore.MAX_COPIES_PER_PASS + 8
        val copy = { i: Int -> File(dir, "Mix.sync-conflict-20261026-${i.toString().padStart(6, '0')}-AAAAAAA.m3u8") }
        for (i in 0 until n) {
            copy(i).apply {
                writeText("#EXTM3U\n../Artist/Album/A.opus\n../Artist/Album/N$i.opus\n")
                setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
            }
        }
        assertEquals(setOf("Mix"), one.store.mergeConflicts(now))
        assertEquals(1 + PlaylistStore.MAX_COPIES_PER_PASS, one.paths("Mix").size)
        assertFalse(copy(0).exists())
        assertTrue(copy(n - 1).exists())
        assertEquals(setOf("Mix"), one.store.mergeConflicts(now))
        assertEquals(1 + n, one.paths("Mix").size)
        assertFalse(one.store.hasConflictCopies())
    }

    /**
     * Merging several copies in memory and writing once gives the bytes that writing the playlist after each copy and reading it back for
     * the next gives (contract: "each into the playlist as the one before left it"), with entries without `#EXTINF`, outside the root,
     * duplicated and with CRLF endings; the same inputs as tunemesh-core's `merging_in_memory_matches_writing_after_each_copy`.
     */
    @Test
    fun mergingInMemoryMatchesWritingAfterEachCopy() {
        val one = Device("chain")
        val dir = File(one.root, "Playlists").apply { mkdirs() }
        val root = one.root.absolutePath
        val base = "#EXTM3U\n../a.mp3\n#EXTINF:12,  Bee \n../b.mp3\n/elsewhere/x.mp3\n../a.mp3\n"
        val winner = "#EXTM3U\r\n../b.mp3\r\n/elsewhere/x.mp3\r\n../c%20d.mp3\r\n../a.mp3\r\n"
        val copies =
            listOf(
                "#EXTM3U\n../e.mp3\n../a.mp3\n#EXTINF:12,Bee\n../b.mp3\n/elsewhere/x.mp3\n../a.mp3\n",
                "#EXTM3U\n../a.mp3\n../b.mp3\n../f.mp3\n../a.mp3\n../a.mp3\n",
                "\uFEFF#EXTM3U\n#EXTINF:-1,G\n../g.mp3\n../b.mp3\n",
            )
        fun parse(text: String): PlaylistEntries {
            val entries = PlaylistEntries.Builder(root, keepExtinf = true)
            text.byteInputStream().use { M3u8.read(it, root, keepExtinf = true) { entry -> entries.add(entry) } }
            return entries.build()
        }
        File(dir, "P.m3u8").writeText(winner)
        PlaylistBases(one.bases).merged("P.m3u8", base.toByteArray())
        copies.forEachIndexed { i, text ->
            File(dir, "P.sync-conflict-20261019-10150$i-AAAAAAA.m3u8").apply {
                writeText(text)
                setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
            }
        }
        val baseEntries = parse(base)
        var expected = winner
        for (copy in copies) expected = M3u8.write(PlaylistMerge.merge(baseEntries, parse(expected), parse(copy))) { null }
        assertEquals(setOf("P"), one.store.mergeConflicts(now))
        assertEquals(expected, File(dir, "P.m3u8").readText())
        assertFalse(one.store.hasConflictCopies())
    }

    /** An entry with a CR inside its line, which writers leave out, is left out between copies too (same inputs as tunemesh-core). */
    @Test
    fun lineBreakEntriesAreDroppedBetweenCopies() {
        val one = Device("cr")
        val dir = File(one.root, "Playlists").apply { mkdirs() }
        val root = one.root.absolutePath
        val start = "#EXTM3U\n../a.mp3\n../b.mp3\n"
        val copies = listOf("#EXTM3U\n../a.mp3\n../x\ry.mp3\n../b.mp3\n", "#EXTM3U\n../x\ry.mp3\n../n.mp3\n../a.mp3\n../b.mp3\n")
        fun parse(text: String): PlaylistEntries {
            val entries = PlaylistEntries.Builder(root, keepExtinf = true)
            text.byteInputStream().use { M3u8.read(it, root, keepExtinf = true) { entry -> entries.add(entry) } }
            return entries.build()
        }
        File(dir, "P.m3u8").writeText(start)
        PlaylistBases(one.bases).merged("P.m3u8", start.toByteArray())
        copies.forEachIndexed { i, text ->
            File(dir, "P.sync-conflict-20261019-10150$i-AAAAAAA.m3u8").apply {
                writeText(text)
                setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
            }
        }
        val base = parse(start)
        var expected = start
        for (copy in copies) expected = M3u8.write(PlaylistMerge.merge(base, parse(expected), parse(copy))) { null }
        one.store.mergeConflicts(now)
        assertEquals(expected, File(dir, "P.m3u8").readText())
    }

    /** The shared cases in docs/test-vectors/playlist-copies/, byte for byte as tunemesh-core runs them. */
    @Test
    fun sharedCopyPassVectors() {
        val cases = File(vectors, "../playlist-copies").listFiles { file -> file.isDirectory }!!.sortedBy { it.name }
        assertTrue("found ${cases.size} cases", cases.isNotEmpty())
        for (case in cases) {
            val one = Device("copies-${case.name}")
            val dir = File(one.root, "Playlists").apply { mkdirs() }
            File(case, "Mix.m3u8").copyTo(File(dir, "Mix.m3u8"))
            PlaylistBases(one.bases).merged("Mix.m3u8", File(case, "base.m3u8").readBytes())
            var pass = 1
            while (File(case, "pass-$pass").isDirectory) {
                for (copy in File(case, "pass-$pass").listFiles()!!) {
                    copy.copyTo(File(dir, copy.name), overwrite = true).setLastModified(now - PlaylistStore.CONFLICT_AGE_MS)
                }
                one.store.mergeConflicts(now)
                val expected = File(case, "pass-$pass-expected")
                val what = "${case.name} pass $pass"
                assertArrayEquals("$what: playlist", File(expected, "Mix.m3u8").readBytes(), File(dir, "Mix.m3u8").readBytes())
                assertArrayEquals("$what: base", File(expected, "base.m3u8").readBytes(), File(one.bases, "Mix.m3u8").readBytes())
                val left = dir.list()!!.filter { it != "Mix.m3u8" }.sorted()
                assertEquals("$what: copies left", File(expected, "left.txt").readLines(), left)
                pass++
            }
        }
    }

    companion object {
        private const val ROOT = "/library"

        /** `docs/test-vectors/playlist-merge/`, from the app module's directory (where Gradle runs unit tests). */
        private val vectors = File(System.getProperty("user.dir"), "../../docs/test-vectors/playlist-merge")
    }
}
