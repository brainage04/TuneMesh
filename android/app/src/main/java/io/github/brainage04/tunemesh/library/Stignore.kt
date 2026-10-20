package io.github.brainage04.tunemesh.library

import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.attribute.BasicFileAttributes

/**
 * `<root>/.stignore` (contract: "`.stignore`"): the Syncthing ignore rules every device needs so partial files
 * are never synced. A port of tunemesh-core's `stignore.rs`; keep the two in step.
 */
object Stignore {
    const val FILE_NAME = ".stignore"
    private const val HEADER = "// TuneMesh"

    /** The rules from `docs/library-format.md`. */
    val RULES = listOf("(?d).~tunemesh-*")
    private const val MAX_BYTES = 1024 * 1024

    /**
     * Makes sure `<root>/.stignore` contains every rule in [RULES]: missing ones are appended (exact match after
     * trimming), the user's lines and their order are kept, and a `// TuneMesh` header is added only when the
     * file is created. Nothing is written when no rule is missing. A `.stignore` that is a symlink or not a
     * regular file is refused with an [IOException]. Returns whether the file was written.
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
    private fun read(file: File): String? {
        val attributes = try {
            Files.readAttributes(file.toPath(), BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
        } catch (_: NoSuchFileException) {
            return null
        }
        if (!attributes.isRegularFile) throw IOException("$file is not a regular file")
        if (attributes.size() > MAX_BYTES) throw IOException("$file: larger than $MAX_BYTES bytes")
        return Files.newInputStream(file.toPath(), LinkOption.NOFOLLOW_LINKS).use { input ->
            // Bounded read: the file may grow between the size check and here.
            val output = java.io.ByteArrayOutputStream(attributes.size().toInt())
            val chunk = ByteArray(8192)
            while (output.size() <= MAX_BYTES) {
                val read = input.read(chunk)
                if (read < 0) break
                output.write(chunk, 0, read)
            }
            if (output.size() > MAX_BYTES) throw IOException("$file: larger than $MAX_BYTES bytes")
            output.toString(Charsets.UTF_8.name())
        }
    }
}
