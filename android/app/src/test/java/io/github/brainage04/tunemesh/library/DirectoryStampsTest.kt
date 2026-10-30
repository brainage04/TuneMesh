package io.github.brainage04.tunemesh.library

import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/** The periodic library check: a folder's stamp changes when a file in it comes, goes or is replaced as Syncthing replaces files. */
class DirectoryStampsTest {
    @get:Rule val temp = TemporaryFolder()

    private fun stamps(vararg directories: File) =
        DirectoryStamps().apply { directories.forEach { add(it.toPath(), DirectoryStamps.modifiedMs(it.toPath())) } }

    /** Sets the folder's modification time into the past (whole seconds), so a change within the clock's resolution still shows. */
    private fun File.aged(): File = apply { setLastModified(lastModified() / 1000 * 1000 - 10_000) }

    @Test
    fun unchangedFoldersAreUnchanged() {
        val album = temp.newFolder("Artist", "Album").aged()
        File(album, "a.mp3").writeText("a")
        album.aged()
        val stamps = stamps(album.parentFile!!.aged(), album)
        assertEquals(2, stamps.size)
        assertEquals(listOf(album.parentFile, album), stamps.files)
        assertFalse(stamps.changed())
    }

    @Test
    fun aNewDeletedOrReplacedFileChangesItsFolder() {
        val album = temp.newFolder("Album")
        val track = File(album, "a.mp3").apply { writeText("a") }
        album.aged()
        val stamps = stamps(album)
        File(album, "b.mp3").writeText("b")
        assertTrue("created", stamps.changed())

        album.aged()
        val afterCreate = stamps(album)
        track.delete()
        assertTrue("deleted", afterCreate.changed())

        album.aged()
        val afterDelete = stamps(album)
        val replacement = File(album, ".syncthing.b.mp3.tmp").apply { writeText("new") }
        album.aged()
        val beforeRename = stamps(album)
        Files.move(replacement.toPath(), File(album, "b.mp3").toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        assertTrue("replaced", beforeRename.changed())
        assertTrue(afterDelete.changed())
    }

    /** A change within the same second shows: stamps are compared to the millisecond (`File.lastModified` has seconds on Android). */
    @Test
    fun aChangeWithinTheSameSecondIsAChange() {
        val album = temp.newFolder("Album")
        val second = album.lastModified() / 1000 * 1000 - 10_000
        album.setLastModified(second + 100)
        val stamps = stamps(album)
        assertFalse(stamps.changed())
        album.setLastModified(second + 600)
        assertTrue(stamps.changed())
    }

    @Test
    fun aGoneFolderIsAChange() {
        val album = temp.newFolder("Album").aged()
        val stamps = stamps(album)
        album.deleteRecursively()
        assertTrue(stamps.changed())
        assertEquals(-1L, DirectoryStamps.modifiedMs(album.toPath()))
    }
}
