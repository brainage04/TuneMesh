package io.github.brainage04.tunemesh.library

import java.text.Normalizer
import java.util.Locale

/** Rules from `docs/library-format.md` that do not touch the file system. */
object LibraryFormat {
    const val PLAYLISTS_DIR = "Playlists"
    const val STATE_DIR = ".tunemesh"
    const val PLAYLIST_EXTENSION = ".m3u8"
    const val LIKED_SONGS = "Liked Songs"
    const val UNKNOWN_ALBUM = "Unknown Album"
    const val UNKNOWN_ARTIST = "Unknown Artist"
    const val STIGNORE = "// TuneMesh\n(?d).~tunemesh-*\n"
    const val MAX_COMPONENT_BYTES = 120

    val AUDIO_EXTENSIONS = setOf("opus", "ogg", "oga", "m4a", "mp4", "aac", "mp3", "flac", "wav", "webm")
    val COVER_FILE_NAMES = listOf("cover.jpg", "cover.png", "folder.jpg", "folder.png")

    private val trailingSourceId = Regex("""\s*\[[^\[\]]+]$""")

    fun isConflictCopy(name: String): Boolean = name.contains(".sync-conflict-")

    fun isHidden(name: String): Boolean = name.startsWith(".")

    fun extension(name: String): String = name.substringAfterLast('.', "").lowercase(Locale.ROOT)

    /** True when a library-relative path (with `/` separators) names a track. */
    fun isAudioPath(relativePath: String): Boolean {
        val parts = relativePath.split('/')
        if (parts.isEmpty() || parts.any { it.isEmpty() || isHidden(it) }) return false
        if (parts.size > 1 && parts[0] == PLAYLISTS_DIR) return false
        val name = parts.last()
        return !isConflictCopy(name) && extension(name) in AUDIO_EXTENSIONS
    }

    /** True when a directory directly below the root (or deeper) must not be scanned. */
    fun isExcludedDirectory(relativePath: String): Boolean {
        val parts = relativePath.split('/')
        return parts.any { isHidden(it) } || parts.firstOrNull() == PLAYLISTS_DIR
    }

    /** File name without extension and without a trailing ` [<id>]`. */
    fun titleFromFileName(name: String): String {
        val stem = if (name.contains('.')) name.substringBeforeLast('.') else name
        val stripped = stem.replace(trailingSourceId, "")
        return stripped.ifBlank { stem }
    }

    fun fallbackAlbum(relativePath: String): String {
        val parts = relativePath.split('/')
        return if (parts.size >= 2) parts[parts.size - 2] else UNKNOWN_ALBUM
    }

    fun fallbackArtist(relativePath: String): String {
        val parts = relativePath.split('/')
        return if (parts.size >= 3) parts[parts.size - 3] else UNKNOWN_ARTIST
    }

    /** First artist of a possibly multi-valued tag (`;` or NUL separated). */
    fun firstArtist(artist: String): String =
        artist.split(';', '\u0000').map { it.trim() }.firstOrNull { it.isNotEmpty() } ?: artist.trim()

    /**
     * Makes one file or directory name safe for every TuneMesh device: forbidden and control characters
     * become `_`, a leading `.` (hidden, so ignored by readers) and a trailing `.` or space become `_`, and the
     * result is at most [maxBytes] UTF-8 bytes.
     */
    fun sanitizeComponent(name: String, maxBytes: Int = MAX_COMPONENT_BYTES): String {
        val replaced = buildString(name.length) {
            for (char in name) {
                append(if (char in "/\\:*?\"<>|" || char.code < 0x20 || char.code == 0x7f) '_' else char)
            }
        }
        var result = truncateUtf8(replaced, maxBytes)
        if (result.isEmpty()) return "_"
        if (result.startsWith(".")) result = "_" + result.substring(1)
        val trimmedEnd = result.trimEnd('.', ' ')
        if (trimmedEnd.length != result.length) {
            result = trimmedEnd + "_".repeat(result.length - trimmedEnd.length)
        }
        return result
    }

    private fun truncateUtf8(value: String, maxBytes: Int): String {
        if (value.toByteArray(Charsets.UTF_8).size <= maxBytes) return value
        val builder = StringBuilder()
        var bytes = 0
        var index = 0
        while (index < value.length) {
            val codePoint = value.codePointAt(index)
            val length = String(Character.toChars(codePoint)).toByteArray(Charsets.UTF_8).size
            if (bytes + length > maxBytes) break
            builder.appendCodePoint(codePoint)
            bytes += length
            index += Character.charCount(codePoint)
        }
        return builder.toString()
    }

    /** Playlist file name (`<Name>.m3u8`) for a user-entered playlist name. */
    fun playlistFileName(name: String): String =
        sanitizeComponent(name.trim(), MAX_COMPONENT_BYTES - PLAYLIST_EXTENSION.length) + PLAYLIST_EXTENSION

    /** Lowercase, accent-free form used for instant search. */
    fun searchKey(value: String): String =
        Normalizer.normalize(value, Normalizer.Form.NFD)
            .replace(Regex("\\p{M}+"), "")
            .lowercase(Locale.ROOT)
}
