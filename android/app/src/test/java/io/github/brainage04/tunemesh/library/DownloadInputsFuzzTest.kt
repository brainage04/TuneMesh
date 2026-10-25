package io.github.brainage04.tunemesh.library

import com.code_intelligence.jazzer.api.FuzzedDataProvider
import com.code_intelligence.jazzer.junit.FuzzTest
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue

/** Jazzer targets for pasted and shared download input; see `fuzz` in app/build.gradle.kts. */
class DownloadInputsFuzzTest {
    /** Pasted text: trimmed, distinct, non-blank lines, each classified without failing, and the request flags follow the kinds. */
    @FuzzTest(maxDuration = "10s")
    fun pastedText(data: FuzzedDataProvider) {
        val target = if (data.consumeBoolean()) data.consumeString(16) else null
        val lines = DownloadInputs.lines(data.consumeRemainingAsString())
        assertEquals(lines.distinct(), lines)
        for (line in lines) {
            assertTrue(line.isNotEmpty() && line == line.trim() && '\n' !in line, line)
            val kind = DownloadInputs.kind(line)
            if (DownloadInputs.playlistFromSource(line, target, saveAsPlaylists = true)) {
                assertTrue(kind != InputKind.Single && target.isNullOrBlank())
            }
        }
        assertEquals(lines.any { DownloadInputs.kind(it) == InputKind.Playlist }, DownloadInputs.saveAsPlaylistsByDefault(lines))
    }

    /** Shared text: the input is the trimmed text, or a link found in it with only trailing punctuation dropped. */
    @FuzzTest(maxDuration = "10s")
    fun sharedText(data: FuzzedDataProvider) {
        val text = data.consumeRemainingAsString()
        val input = DownloadInputs.sharedInput(text)
        assertTrue(text.trim().contains(input))
        if (input.startsWith("http://") || input.startsWith("https://")) {
            val link = Regex("""https?://\S+""").find(text.trim())?.value
            if (link != null && link.startsWith(input)) {
                assertTrue(link.substring(input.length).all { it in ".,;:!?'\">)]}" })
            }
        }
        DownloadInputs.kind(input)
    }
}
