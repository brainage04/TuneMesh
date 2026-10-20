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
    const val MAX_COMPONENT_BYTES = 120

    val AUDIO_EXTENSIONS = setOf("opus", "ogg", "oga", "m4a", "mp4", "aac", "mp3", "flac", "wav", "webm")
    val COVER_FILE_NAMES = listOf("cover.jpg", "cover.png", "folder.jpg", "folder.png")


    fun isConflictCopy(name: String): Boolean = name.contains(".sync-conflict-")

    fun isHidden(name: String): Boolean = name.startsWith(".")

    /**
     * True when a name contains LF or CR. Such a file or directory is not part of the library (an m3u8 entry
     * line could not name it): scanners skip it and everything below it, and playlists never get such an entry.
     */
    fun hasLineBreak(name: String): Boolean = name.contains('\n') || name.contains('\r')

    fun extension(name: String): String = name.substringAfterLast('.', "").lowercase(Locale.ROOT)

    /** True when a library-relative path (with `/` separators) names a track. */
    fun isAudioPath(relativePath: String): Boolean {
        val parts = relativePath.split('/')
        if (parts.isEmpty() || parts.any { it.isEmpty() || isHidden(it) || hasLineBreak(it) }) return false
        if (parts.size > 1 && parts[0] == PLAYLISTS_DIR) return false
        val name = parts.last()
        return !isConflictCopy(name) && extension(name) in AUDIO_EXTENSIONS
    }

    /** True when a directory directly below the root (or deeper) must not be scanned. */
    fun isExcludedDirectory(relativePath: String): Boolean {
        val parts = relativePath.split('/')
        return parts.any { isHidden(it) || hasLineBreak(it) } || parts.firstOrNull() == PLAYLISTS_DIR
    }

    /** File name without extension and without a trailing ` [<id>]`: tunemesh-core's `title_from_file_name`. */
    fun titleFromFileName(name: String): String {
        val stem = name.substringBeforeLast('.', "").ifEmpty { name }
        if (stem.endsWith(']')) {
            val open = stem.lastIndexOf('[', stem.length - 2)
            if (open >= 0 && open + 1 < stem.length - 1 && ']' !in stem.substring(open + 1, stem.length - 1)) {
                val title = stem.substring(0, open).trimEnd(::isWhiteSpace)
                if (title.isNotEmpty()) return title
            }
        }
        return stem
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

    /*
     * Names written by TuneMesh, the same rules as tunemesh-core's sanitize.rs (keep the two in step):
     * trim Unicode White_Space, replace `/ \ : * ? " < > |` and control characters (category Cc: U+0000-001F,
     * U+007F-009F) with `_`, cut to MAX_COMPONENT_BYTES UTF-8 bytes on a character boundary, then replace a
     * leading `.` and each trailing `.` or space with `_`. Kotlin strings may also hold unpaired surrogates,
     * which Rust strings cannot; they become `_` as well.
     */

    /** Sanitises one path component (an artist, album or playlist name); `sanitize::component` in Rust. */
    fun sanitizeComponent(name: String): String {
        val result = fixEnds(truncateUtf8(replaceForbidden(trimWhiteSpace(name)), MAX_COMPONENT_BYTES))
        return result.ifEmpty { "_" }
    }

    /**
     * Sanitises a file name made of a free-form [stem] and a fixed [suffix] (for example `" [id].opus"` or
     * `".m3u8"`): the stem is shortened first so the suffix always survives. `sanitize::file_name` in Rust.
     */
    fun sanitizeFileName(stem: String, suffix: String): String {
        val safeSuffix = replaceForbidden(suffix)
        var safeStem = replaceForbidden(trimWhiteSpace(stem))
        if (safeStem.startsWith(".")) safeStem = "_" + safeStem.substring(1)
        if (safeStem.isEmpty()) safeStem = "_"
        val suffixBytes = safeSuffix.toByteArray(Charsets.UTF_8).size
        safeStem = truncateUtf8(safeStem, maxOf(MAX_COMPONENT_BYTES - suffixBytes, 1))
        return fixEnds(truncateUtf8(safeStem + safeSuffix, MAX_COMPONENT_BYTES))
    }

    private fun isForbidden(char: Char): Boolean =
        char in "/\\:*?\"<>|" || Character.getType(char) == Character.CONTROL.toInt()

    private fun replaceForbidden(value: String): String = buildString(value.length) {
        var index = 0
        while (index < value.length) {
            val char = value[index]
            val pair = char.isHighSurrogate() && index + 1 < value.length && value[index + 1].isLowSurrogate()
            when {
                pair -> append(char).append(value[index + 1])
                char.isSurrogate() || isForbidden(char) -> append('_')
                else -> append(char)
            }
            index += if (pair) 2 else 1
        }
    }

    /** Unicode White_Space, which Rust's `str::trim` removes (Kotlin's `trim` differs, e.g. U+001C, U+0085). */
    private fun isWhiteSpace(char: Char): Boolean = when (char.code) {
        in 0x09..0x0D, 0x20, 0x85, 0xA0, 0x1680, in 0x2000..0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 -> true
        else -> false
    }

    internal fun trimWhiteSpace(value: String): String = value.trim(::isWhiteSpace)

    /** A leading `.` and each trailing `.` or space become `_`. */
    private fun fixEnds(value: String): String {
        var result = value
        if (result.startsWith(".")) result = "_" + result.substring(1)
        val kept = result.trimEnd('.', ' ')
        return kept + "_".repeat(result.length - kept.length)
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

    /** Playlist file name (`<Name>.m3u8`) for a user-entered playlist name, as the desktop app and worker name it. */
    fun playlistFileName(name: String): String = sanitizeFileName(name, PLAYLIST_EXTENSION)

    /** Lowercase, accent-free form used for instant search. */
    fun searchKey(value: String): String =
        Normalizer.normalize(value, Normalizer.Form.NFD)
            .replace(Regex("\\p{M}+"), "")
            .lowercase(Locale.ROOT)
}
