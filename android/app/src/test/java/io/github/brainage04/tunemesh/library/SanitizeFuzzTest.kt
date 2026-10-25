package io.github.brainage04.tunemesh.library

import com.code_intelligence.jazzer.api.FuzzedDataProvider
import com.code_intelligence.jazzer.junit.FuzzTest
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue

/**
 * Jazzer targets for the name sanitiser; see `fuzz` in app/build.gradle.kts. Every name written follows `docs/library-format.md` ("Audio
 * files"): no `/ \ : * ? " < > |`, control characters or unpaired surrogates, no leading `.`, no trailing `.` or space, at most 120 UTF-8
 * bytes, never empty.
 */
class SanitizeFuzzTest {
    @FuzzTest(maxDuration = "10s")
    fun component(data: FuzzedDataProvider) {
        assertWritable(LibraryFormat.sanitizeComponent(data.consumeRemainingAsString()))
    }

    /** A free-form stem with a fixed suffix keeps the suffix; any suffix still gives a writable name. */
    @FuzzTest(maxDuration = "10s")
    fun fileName(data: FuzzedDataProvider) {
        val suffix = data.consumeString(40)
        val stem = data.consumeRemainingAsString()
        assertWritable(LibraryFormat.sanitizeFileName(stem, suffix))
        val playlist = LibraryFormat.playlistFileName(stem)
        assertWritable(playlist)
        assertTrue(playlist.endsWith(LibraryFormat.PLAYLIST_EXTENSION), playlist)
        val audio = LibraryFormat.sanitizeFileName(stem, " [dQw4w9WgXcQ].opus")
        assertWritable(audio)
        assertTrue(audio.endsWith(" [dQw4w9WgXcQ].opus"), audio)
    }

    private fun assertWritable(name: String) {
        assertTrue(name.isNotEmpty())
        assertTrue(Limits.utf8Length(name) <= LibraryFormat.MAX_COMPONENT_BYTES, name)
        assertFalse(name.startsWith("."), name)
        assertFalse(name.endsWith(".") || name.endsWith(" "), name)
        var index = 0
        while (index < name.length) {
            val codePoint = name.codePointAt(index)
            assertFalse(Character.charCount(codePoint) == 1 && Character.isSurrogate(name[index]), name)
            assertFalse(codePoint < 0x80 && codePoint.toChar() in "/\\:*?\"<>|", name)
            assertFalse(Character.getType(codePoint) == Character.CONTROL.toInt(), name)
            index += Character.charCount(codePoint)
        }
    }
}
