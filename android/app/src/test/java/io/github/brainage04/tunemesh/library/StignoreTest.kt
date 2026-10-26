package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.IOException
import java.nio.file.Files
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/** Mirrors the tests of tunemesh-core's `stignore.rs`. */
class StignoreTest {
    @get:Rule val temp = TemporaryFolder()

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

    /** R3-Opus-7 (core's `keeps_the_users_bytes`): invalid UTF-8 in the user's lines was replaced by U+FFFD when the rule was appended. */
    @Test
    fun keepsTheUsersBytes() {
        val old = byteArrayOf(0x63, 0x61, 0x66, 0xe9.toByte(), 0x2f, 0x0a, 0xff.toByte(), 0x2a) + ".tmp".toByteArray()
        val new = Stignore.withRules(old)!!
        assertArrayEquals(old + "\n(?d).~tunemesh-*\n".toByteArray(), new)
        assertNull(Stignore.withRules(new))
        assertNull(Stignore.withRules(byteArrayOf(0xff.toByte(), 0x0d, 0x0a) + " (?d).~tunemesh-*\r\n".toByteArray()))

        val root = temp.newFolder("lib")
        val file = File(root, Stignore.FILE_NAME).apply { writeBytes(old) }
        assertTrue(Stignore.ensure(root))
        assertArrayEquals(new, file.readBytes())
    }

    /** "Limits": a `.stignore` over 1 MiB is left unchanged, and the refusal is an IOException the callers log. */
    @Test
    fun leavesAnOversizeFileAlone() {
        val root = temp.newFolder("lib")
        val file = File(root, Stignore.FILE_NAME).apply { writeBytes(ByteArray(Limits.MAX_STIGNORE_BYTES + 1) { 'x'.code.toByte() }) }
        assertThrows(TooLargeException::class.java) { Stignore.ensure(root) }
        assertEquals(Limits.MAX_STIGNORE_BYTES + 1L, file.length())
    }
}
