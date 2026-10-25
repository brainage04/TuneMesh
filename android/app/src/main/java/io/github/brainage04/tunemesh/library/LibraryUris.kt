package io.github.brainage04.tunemesh.library

import java.io.File
import java.net.URI

/**
 * Which files the exported playback service and cover provider may touch: only audio files inside the library. Both are reachable from
 * other apps, so anything else (other files the app can read with All files access, `content://` or network URIs) is refused.
 */
object LibraryUris {
    /** True when [uri] is a `file://` URI naming a track file below [root], after resolving `..` and symlinks. */
    fun isLibraryFile(uri: String, root: File): Boolean = libraryPath(uri, root) != null

    /** The library-relative path of the track a `file://` [uri] names, or null when it is not a track below [root]. */
    fun libraryPath(uri: String, root: File): String? {
        val parsed = runCatching { URI(uri) }.getOrNull() ?: return null
        if (!parsed.scheme.equals("file", ignoreCase = true) || parsed.rawAuthority != null) return null
        val path = parsed.path ?: return null
        return relativeTrackPath(File(path), root)
    }

    /**
     * The canonical library-relative path for a relative [path] as other components pass it around, or null when it is absolute, leaves
     * [root] (`..`, symlinks) or does not name an audio file the library would index.
     */
    fun resolvePath(path: String, root: File): String? {
        if (path.isEmpty() || path.startsWith("/") || path.contains('\u0000')) return null
        return relativeTrackPath(File(root, path), root)
    }

    private fun relativeTrackPath(file: File, root: File): String? =
        try {
            val base = root.canonicalFile
            val canonical = file.canonicalFile
            if (!canonical.path.startsWith(base.path + File.separator)) {
                null
            } else {
                canonical.relativeTo(base).invariantSeparatorsPath.takeIf(LibraryFormat::isAudioPath)
            }
        } catch (_: Exception) {
            null
        }
}
