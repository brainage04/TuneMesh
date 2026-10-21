package io.github.brainage04.tunemesh.library

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException
import java.nio.file.Files

/** Mirrors the tests of tunemesh-core's `stignore.rs`. */
class StignoreTest {
    @get:Rule
    val temp = TemporaryFolder()

    @Test
    fun createsAppendsAndKeepsUserLines() {
        val root = temp.newFolder("lib")
        val file = File(root, Stignore.FILE_NAME)
        assertTrue(Stignore.ensure(root))
        assertEquals("// TuneMesh\n(?d).~tunemesh-*\n", file.readText())
        assertFalse(Stignore.ensure(root))

        file.writeText("#include shared\n*.tmp")
        assertTrue(Stignore.ensure(root))
        assertEquals("#include shared\n*.tmp\n(?d).~tunemesh-*\n", file.readText())
        file.writeText("  (?d).~tunemesh-*  \n")
        assertFalse(Stignore.ensure(root))
    }

    @Test
    fun refusesASymlinkedStignore() {
        val target = File(temp.newFolder("outside"), "x").apply { writeText("keep") }
        val root = temp.newFolder("lib")
        Files.createSymbolicLink(File(root, Stignore.FILE_NAME).toPath(), target.toPath())
        assertThrows(IOException::class.java) { Stignore.ensure(root) }
        assertEquals("keep", target.readText())
    }

    @Test
    fun refusesADirectoryAndLeavesNoTemporaryFiles() {
        val root = temp.newFolder("lib")
        File(root, Stignore.FILE_NAME).mkdir()
        assertThrows(IOException::class.java) { Stignore.ensure(root) }
        assertEquals(listOf(Stignore.FILE_NAME), root.list()!!.toList())
    }

    @Test
    fun keepsCrlfUserLinesAndAnEmptyFile() {
        val root = temp.newFolder("lib")
        val file = File(root, Stignore.FILE_NAME)
        file.writeText("(?d).~tunemesh-*\r\n")
        assertFalse(Stignore.ensure(root))
        file.writeText("")
        assertTrue(Stignore.ensure(root))
        assertEquals("(?d).~tunemesh-*\n", file.readText())
    }
}
