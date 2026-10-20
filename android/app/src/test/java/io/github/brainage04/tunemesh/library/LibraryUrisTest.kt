package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files

class LibraryUrisTest {
    @get:Rule
    val temp = TemporaryFolder()

    private val root get() = File(temp.root, "Music/TuneMesh").also { it.mkdirs() }

    /** `file://` URI the way Android's `Uri.fromFile` writes it: path percent-encoded. */
    private fun uri(file: File) = file.toURI().toString()

    @Test
    fun acceptsAudioFilesInsideTheLibrary() {
        val track = File(root, "Sigur Rós/Ágætis byrjun/Svefn-g-englar [abc].opus")
        assertTrue(LibraryUris.isLibraryFile(uri(track), root))
        assertTrue(LibraryUris.isLibraryFile("file://${root.absolutePath}/A/B/c%20%5Bx%5D.mp3", root))
    }

    @Test
    fun refusesEverythingElse() {
        val outside = File(temp.root, "Music/secret.mp3")
        val sibling = File(temp.root, "Music/TuneMeshOther/a.mp3")
        for (rejected in listOf(
            uri(outside),
            uri(sibling),
            "file://${root.absolutePath}/../secret.mp3",
            "file://${root.absolutePath}/A/notes.txt",
            "file://${root.absolutePath}/.tunemesh/requests/x.mp3",
            "file://${root.absolutePath}/Playlists/x.mp3",
            "file://otherhost${root.absolutePath}/a.mp3",
            "file:///data/data/io.github.brainage04.tunemesh/shared_prefs/settings.xml",
            "content://media/external/audio/media/1",
            "https://example.com/a.mp3",
            "not a uri",
            "",
        )) {
            assertFalse(rejected, LibraryUris.isLibraryFile(rejected, root))
        }
    }

    @Test
    fun refusesSymlinksOutOfTheLibrary() {
        val target = File(temp.root, "elsewhere.mp3").also { it.writeText("x") }
        val link = File(root, "link.mp3")
        Files.createSymbolicLink(link.toPath(), target.toPath())
        assertFalse(LibraryUris.isLibraryFile(uri(link), root))
    }

    @Test
    fun libraryPathIsTheCanonicalRelativePath() {
        val track = File(root, "A/B/c [x].mp3")
        assertEquals("A/B/c [x].mp3", LibraryUris.libraryPath(uri(track), root))
        assertEquals("A/B/c.mp3", LibraryUris.libraryPath("file://${root.absolutePath}/A/X/../B/c.mp3", root))
        assertNull(LibraryUris.libraryPath("file://${root.absolutePath}/../secret.mp3", root))
    }

    /** The cover provider's `path` parameter (audit finding 7). */
    @Test
    fun resolvePathKeepsCoverRequestsInsideTheLibrary() {
        File(root.parentFile, "secret.mp3").writeText("x")
        assertEquals("A/B/Song.opus", LibraryUris.resolvePath("A/B/Song.opus", root))
        assertEquals("A/B/Song.opus", LibraryUris.resolvePath("A/./C/../B/Song.opus", root))
        for (rejected in listOf(
            "../secret.mp3",
            "../../Music/secret.mp3",
            "A/../../secret.mp3",
            "${root.absolutePath}/A/B/Song.opus",
            "/etc/passwd",
            "A/B/notes.txt",
            ".tunemesh/requests/x.mp3",
            "Playlists/x.mp3",
            "A/.hidden.mp3",
            "A/B/Song.sync-conflict-20261006-050137-ABCDEFG.opus",
            "A/B/Song\u0000.opus",
            "",
        )) {
            assertNull(rejected, LibraryUris.resolvePath(rejected, root))
        }
        val target = File(temp.root, "elsewhere.mp3").also { it.writeText("x") }
        Files.createSymbolicLink(File(root, "link.mp3").toPath(), target.toPath())
        assertNull(LibraryUris.resolvePath("link.mp3", root))
    }
}
