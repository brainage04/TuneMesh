package io.github.brainage04.tunemesh.library

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.nio.file.NoSuchFileException

/**
 * `<root>/.stignore` (contract: "`.stignore`"): the Syncthing ignore rules every device needs so partial files are never synced. A port of
 * tunemesh-core's `stignore.rs`; keep the two in step.
 */
object Stignore {
    const val FILE_NAME = ".stignore"
    private const val HEADER = "// TuneMesh"

    /** The rules from `docs/library-format.md`. */
    val RULES = listOf("(?d).~tunemesh-*")

    /**
     * Makes sure `<root>/.stignore` contains every rule in [RULES] (see [withRules]). Nothing is written when no rule is missing. A
     * `.stignore` that is a symlink or not a regular file is refused with an [IOException], one over [Limits.MAX_STIGNORE_BYTES] with a
     * [TooLargeException]. Returns whether the file was written.
     */
    fun ensure(root: File): Boolean {
        val file = File(root, FILE_NAME)
        val text = withRules(read(file)) ?: return false
        AtomicFiles.write(file, text)
        return true
    }

    /**
     * The `.stignore` contents [ensure] writes for [existing] (null: no file), or null when no rule is missing: missing rules are appended
     * (exact match after trimming), the user's lines and their order are kept byte for byte (invalid UTF-8 included), and a `// TuneMesh`
     * header is added only when the file is created. tunemesh-core's `with_rules`.
     */
    fun withRules(existing: ByteArray?): ByteArray? {
        val missing = LinkedHashSet(RULES)
        if (existing != null) {
            var start = 0
            for (i in 0..existing.size) {
                if (i < existing.size && existing[i] != '\n'.code.toByte()) continue
                // Only a line holding a missing rule's bytes can be that rule after trimming; others are not decoded.
                if (missing.any { contains(existing, start, i, it) }) {
                    missing -= LibraryFormat.trimWhiteSpace(String(existing, start, i - start, Charsets.UTF_8))
                }
                start = i + 1
            }
        }
        if (missing.isEmpty()) return null
        val text = ByteArrayOutputStream((existing?.size ?: 0) + HEADER.length + missing.sumOf { it.length + 1 } + 2)
        if (existing == null) {
            text.write("$HEADER\n".toByteArray(Charsets.UTF_8))
        } else {
            text.write(existing)
            if (existing.isNotEmpty() && existing.last() != '\n'.code.toByte()) text.write('\n'.code)
        }
        missing.forEach { text.write("$it\n".toByteArray(Charsets.UTF_8)) }
        return text.toByteArray()
    }

    /** Whether `bytes[from, to)` contains [rule] (ASCII). */
    private fun contains(bytes: ByteArray, from: Int, to: Int, rule: String): Boolean {
        val last = to - rule.length
        var at = from
        while (at <= last) {
            var i = 0
            while (i < rule.length && bytes[at + i] == rule[i].code.toByte()) i++
            if (i == rule.length) return true
            at++
        }
        return false
    }

    /** The file's bytes, or null when it does not exist. Never follows a symlink. */
    private fun read(file: File): ByteArray? =
        try {
            Limits.readBytes(file, Limits.MAX_STIGNORE_BYTES, followLinks = false)
        } catch (_: NoSuchFileException) {
            null
        }
}
