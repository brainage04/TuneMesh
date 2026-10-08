package io.github.brainage04.tunemesh.library

import java.io.File
import java.net.URI

/** Which media URIs the playback service may play: only audio files inside the library. */
object LibraryUris {
    /**
     * True when [uri] is a `file://` URI naming a track file below [root], after resolving `..` and symlinks.
     * The playback service is exported to every media controller, so anything else (other files the app can
     * read with All files access, `content://` or network URIs) is refused.
     */
    fun isLibraryFile(uri: String, root: File): Boolean {
        val parsed = runCatching { URI(uri) }.getOrNull() ?: return false
        if (!parsed.scheme.equals("file", ignoreCase = true) || parsed.rawAuthority != null) return false
        val path = parsed.path ?: return false
        return try {
            val base = root.canonicalFile
            val file = File(path).canonicalFile
            val relative = file.relativeTo(base).invariantSeparatorsPath
            file.path.startsWith(base.path + File.separator) && LibraryFormat.isAudioPath(relative)
        } catch (_: Exception) {
            false
        }
    }
}
