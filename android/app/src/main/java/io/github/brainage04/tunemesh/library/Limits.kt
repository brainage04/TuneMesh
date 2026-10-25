package io.github.brainage04.tunemesh.library

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.attribute.BasicFileAttributes

/** A synced file larger than this device reads; it is skipped, never truncated or rewritten. */
class TooLargeException(message: String) : IOException(message)

/**
 * Every limit on what this app reads from the synced library folder and shows of it: `docs/library-format.md`, "Limits" (tunemesh-core:
 * `tunemesh_core::limits`). Any device that shares the folder can write these files, so every read is bounded. Sizes are UTF-8 bytes; text
 * is cut at a character boundary.
 */
object Limits {
    // ---- files -------------------------------------------------------------------------------------------------

    /** A request, status or `worker.json` file; a larger one is not read. */
    const val MAX_JSON_BYTES = 1024 * 1024

    /** `<root>/.stignore`; a larger one is left unchanged. */
    const val MAX_STIGNORE_BYTES = 1024 * 1024

    // ---- downloads ---------------------------------------------------------------------------------------------

    /** A request's `input` (also a status file's), cut when read. */
    const val MAX_REQUEST_INPUT_BYTES = 8 * 1024

    /** A request's `playlist`, cut when read. */
    const val MAX_REQUEST_PLAYLIST_BYTES = 1024

    /** Request and status files read: the newest ids (a request with its status counts once); older are counted. */
    const val MAX_DOWNLOADS_READ = 200

    /** A status `title`, each warning, each failure's `item` and `error`. */
    const val MAX_STATUS_TEXT_BYTES = 1024

    /** A status `message`. */
    const val MAX_STATUS_MESSAGE_BYTES = 4 * 1024

    /** Status `warnings` kept. */
    const val MAX_STATUS_WARNINGS = 50

    /** Status `failures` kept. */
    const val MAX_STATUS_FAILURES = 200

    /** Status `tracks` kept. */
    const val MAX_STATUS_TRACKS = 10_000

    // ---- playlists ---------------------------------------------------------------------------------------------

    /** A playlist file; a larger one is too large (listed, never opened, rewritten or truncated). */
    const val MAX_PLAYLIST_BYTES = 32L * 1024 * 1024

    /** Entries in one playlist. */
    const val MAX_PLAYLIST_ENTRIES = 250_000

    /** A playlist line, without its line ending. */
    const val MAX_PLAYLIST_LINE_BYTES = 32 * 1024

    /** Playlists listed, by name ignoring case; Liked Songs is not counted and always shown. */
    const val MAX_PLAYLISTS_LISTED = 1_000

    /** An `#EXTINF` label, cut when read and when written. */
    const val MAX_LABEL_BYTES = 1024

    // ---- audio files -------------------------------------------------------------------------------------------

    /** A tag text field (title, artist, album, album artist). */
    const val MAX_TAG_TEXT_BYTES = 1024

    /** Artists in one tag. */
    const val MAX_ARTISTS = 32

    /** A single tag item (picture or text frame); a larger one is not used. */
    const val MAX_TAG_ITEM_BYTES = 16 * 1024 * 1024

    /** An embedded or folder cover; a larger one is not shown. */
    const val MAX_COVER_BYTES = 16 * 1024 * 1024

    /** Pixels per side of a cover, as its image header states; a larger one is not shown. */
    const val MAX_COVER_SIDE = 5_000

    /** Folder levels below the root that are scanned. */
    const val MAX_DEPTH = 64

    /** Entries read from one folder; the rest of it is not scanned. */
    const val MAX_FOLDER_ENTRIES = 100_000

    /** Audio files in the library, in scan order. */
    const val MAX_AUDIO_FILES = 250_000

    // ---- display -----------------------------------------------------------------------------------------------

    /** A single-line text (title, name, label, path): characters shown. */
    const val SHOWN_LINE_CHARS = 300

    /** A message or warning: characters shown. */
    const val SHOWN_MESSAGE_CHARS = 2_000

    /** A message or warning: lines shown. */
    const val SHOWN_MESSAGE_LINES = 20

    /** Rows shown of a list read from a synced file, then "and N more". */
    const val SHOWN_LIST_ROWS = 200

    // ---- helpers -----------------------------------------------------------------------------------------------

    /** The bytes of [file], at most [max]. Throws [TooLargeException] past [max], also when it grows while read. */
    @Suppress("SpreadOperator") // copies an array of at most one option
    fun readBytes(file: File, max: Int, followLinks: Boolean = true): ByteArray {
        val options = if (followLinks) emptyArray() else arrayOf(LinkOption.NOFOLLOW_LINKS)
        val path = file.toPath()
        val attributes = Files.readAttributes(path, BasicFileAttributes::class.java, *options)
        if (!attributes.isRegularFile) throw IOException("$file is not a regular file")
        val size = attributes.size()
        if (size > max) throw TooLargeException("$file: larger than $max bytes")
        return Files.newInputStream(path, *options).use { input ->
            val output = ByteArrayOutputStream(size.toInt())
            val chunk = ByteArray(8192)
            while (output.size() <= max) {
                val read = input.read(chunk)
                if (read < 0) break
                output.write(chunk, 0, read)
            }
            if (output.size() > max) throw TooLargeException("$file: larger than $max bytes")
            output.toByteArray()
        }
    }

    /** [readBytes] as UTF-8 text. */
    fun readText(file: File, max: Int, followLinks: Boolean = true): String = String(readBytes(file, max, followLinks), Charsets.UTF_8)

    /** [text] cut to at most [maxBytes] UTF-8 bytes at a character boundary (an unpaired surrogate counts 3). */
    fun cutUtf8(text: String, maxBytes: Int): String {
        if (text.length * 3L <= maxBytes) return text
        var bytes = 0
        var end = 0
        while (end < text.length) {
            val codePoint = text.codePointAt(end)
            val chars = Character.charCount(codePoint)
            val length =
                when {
                    Character.isSurrogate(text[end]) && chars == 1 -> 3
                    codePoint < 0x80 -> 1
                    codePoint < 0x800 -> 2
                    codePoint < 0x10000 -> 3
                    else -> 4
                }
            if (bytes + length > maxBytes) break
            bytes += length
            end += chars
        }
        return if (end == text.length) text else text.substring(0, end)
    }

    /** UTF-8 length of [text] (an unpaired surrogate is written as U+FFFD, 3 bytes). */
    fun utf8Length(text: String): Long {
        var bytes = 0L
        var i = 0
        while (i < text.length) {
            val codePoint = text.codePointAt(i)
            val chars = Character.charCount(codePoint)
            bytes +=
                when {
                    Character.isSurrogate(text[i]) && chars == 1 -> 3
                    codePoint < 0x80 -> 1
                    codePoint < 0x800 -> 2
                    codePoint < 0x10000 -> 3
                    else -> 4
                }
            i += chars
        }
        return bytes
    }

    /** A single-line text as shown: line breaks as spaces, at most [SHOWN_LINE_CHARS] characters, else ending in `…`. */
    fun shownLine(text: String): String = cutChars(text, SHOWN_LINE_CHARS).replace('\n', ' ').replace('\r', ' ')

    /** A message or warning as shown: at most [SHOWN_MESSAGE_CHARS] characters and [SHOWN_MESSAGE_LINES] lines. */
    fun shownMessage(text: String): String {
        val cut = cutChars(text, SHOWN_MESSAGE_CHARS)
        var newlines = 0
        for (i in cut.indices) {
            if (cut[i] == '\n' && ++newlines == SHOWN_MESSAGE_LINES) return cut.substring(0, i) + "…"
        }
        return cut
    }

    /** [text] cut to [max] characters, not inside a surrogate pair, ending in `…` when it was longer. */
    internal fun cutChars(text: String, max: Int): String {
        if (text.length <= max) return text
        var end = max - 1
        if (Character.isHighSurrogate(text[end - 1])) end--
        return text.substring(0, end) + "…"
    }
}
