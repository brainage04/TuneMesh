package io.github.brainage04.tunemesh.library

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
     * Makes sure `<root>/.stignore` contains every rule in [RULES]: missing ones are appended (exact match after trimming), the user's
     * lines and their order are kept, and a `// TuneMesh` header is added only when the file is created. Nothing is written when no rule is
     * missing. A `.stignore` that is a symlink or not a regular file is refused with an [IOException]. Returns whether the file was
     * written.
     */
    fun ensure(root: File): Boolean {
        val file = File(root, FILE_NAME)
        val existing = read(file)
        val present = existing?.split('\n')?.map(LibraryFormat::trimWhiteSpace).orEmpty()
        val missing = RULES.filter { it !in present }
        if (missing.isEmpty()) return false
        val text = StringBuilder()
        if (existing == null) {
            text.append(HEADER).append('\n')
        } else {
            text.append(existing)
            if (existing.isNotEmpty() && !existing.endsWith('\n')) text.append('\n')
        }
        missing.forEach { text.append(it).append('\n') }
        AtomicFiles.write(file, text.toString())
        return true
    }

    /** The file's text, or null when it does not exist. Never follows a symlink. */
    private fun read(file: File): String? =
        try {
            Limits.readText(file, Limits.MAX_STIGNORE_BYTES, followLinks = false)
        } catch (_: NoSuchFileException) {
            null
        }
}
