package io.github.brainage04.tunemesh.library

import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream

/** A playlist file beyond what this device reads ([M3u8.MAX_BYTES], [M3u8.MAX_ENTRIES], [M3u8.MAX_LINE_BYTES]). */
class PlaylistTooLargeException(message: String) : IOException(message)

/** Reads and writes `Playlists/<Name>.m3u8` files as described in the library contract. */
object M3u8 {
    private const val HEADER = "#EXTM3U"
    private const val EXTINF = "#EXTINF:"

    /** Largest playlist file read, as tunemesh-core (`MAX_PLAYLIST_BYTES`). */
    const val MAX_BYTES = 32L * 1024 * 1024

    /** Most entries read from one playlist; a phone's heap holds this many with room to spare. */
    const val MAX_ENTRIES = 250_000

    /** Longest line read; a library path is at most a few KiB. */
    const val MAX_LINE_BYTES = 32 * 1024

    /**
     * Parses playlist [text] into entries (with their `#EXTINF` lines). [root] is the absolute library root; it is
     * only used to resolve absolute entries.
     */
    fun parse(text: String, root: String): List<PlaylistEntry> {
        val entries = ArrayList<PlaylistEntry>()
        read(ByteArrayInputStream(text.toByteArray(Charsets.UTF_8)), root, keepExtinf = true) { entries += it }
        return entries
    }

    /**
     * Streams the entries of a playlist file to [onEntry], one line at a time, without holding the file in memory.
     * Lines are read as tunemesh-core's `parse` reads them: a leading BOM goes, LF ends a line, trailing CRs go,
     * blank lines (Unicode White_Space only) and `#` lines are skipped, and an entry line is resolved exactly as
     * written. [canonical] may return an existing string equal to a resolved path (the library index's), so
     * entries share it instead of holding a copy. `#EXTINF` lines are kept only with [keepExtinf]: a writer needs
     * them for entries whose track is missing, a screen does not.
     *
     * Throws [PlaylistTooLargeException] past [maxBytes], [maxEntries] or [MAX_LINE_BYTES] (counted in bytes, without
     * the line ending: LF and any CRs before it); nothing is truncated.
     */
    fun read(
        input: InputStream,
        root: String,
        keepExtinf: Boolean,
        maxBytes: Long = MAX_BYTES,
        maxEntries: Int = MAX_ENTRIES,
        canonical: (String) -> String? = { null },
        onEntry: (PlaylistEntry) -> Unit,
    ) {
        val buffer = ByteArray(64 * 1024)
        var line = ByteArray(256)
        var lineLength = 0
        var total = 0L
        var first = true
        var entries = 0
        var pendingExtinf: String? = null

        fun endLine() {
            var end = lineLength
            while (end > 0 && line[end - 1] == '\r'.code.toByte()) end--
            var text = String(line, 0, end, Charsets.UTF_8)
            lineLength = 0
            if (first) {
                first = false
                text = text.removePrefix("\uFEFF")
            }
            if (LibraryFormat.trimWhiteSpace(text).isEmpty()) return
            if (text.startsWith("#")) {
                if (text.startsWith(EXTINF)) pendingExtinf = if (keepExtinf) text else null
                return
            }
            if (++entries > maxEntries) throw PlaylistTooLargeException("more than $maxEntries entries")
            val path = resolve(text, root)?.let { canonical(it) ?: it }
            onEntry(PlaylistEntry(path, text, pendingExtinf))
            pendingExtinf = null
        }

        /** Adds `buffer[from, to)` to the line. Past [MAX_LINE_BYTES] only CRs may follow (a CRLF ending), unstored. */
        fun append(from: Int, to: Int) {
            val keep = minOf(to - from, MAX_LINE_BYTES - lineLength)
            for (i in from + keep until to) {
                if (buffer[i] != '\r'.code.toByte()) throw PlaylistTooLargeException("a line is longer than ${MAX_LINE_BYTES / 1024} KiB")
            }
            if (keep <= 0) return
            if (lineLength + keep > line.size) line = line.copyOf(minOf(maxOf(lineLength + keep, line.size * 2), MAX_LINE_BYTES))
            System.arraycopy(buffer, from, line, lineLength, keep)
            lineLength += keep
        }

        var pending = false
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            total += count
            if (total > maxBytes) throw PlaylistTooLargeException("larger than ${maxBytes / (1024 * 1024)} MiB")
            var start = 0
            for (i in 0 until count) {
                if (buffer[i] != '\n'.code.toByte()) continue
                append(start, i)
                endLine()
                pending = false
                start = i + 1
            }
            append(start, count)
            if (start < count) pending = true
        }
        if (pending) endLine()
    }

    /**
     * Library-relative path for an entry line, or null when it points outside the library: tunemesh-core's
     * `resolve`. The line is used as written (not trimmed). An absolute line must start with [root] and `/`; a
     * relative one is relative to `Playlists/`. `""` and `.` components are skipped, `..` goes up and may not
     * leave the root.
     */
    fun resolve(line: String, root: String): String? {
        val rest: String
        val relative: Boolean
        if (line.startsWith("/")) {
            val trimmedRoot = root.trimEnd('/')
            if (!line.startsWith(trimmedRoot) || !line.startsWith("/", trimmedRoot.length)) return null
            rest = line.substring(trimmedRoot.length + 1)
            relative = false
        } else {
            rest = line
            relative = true
        }
        // The common case without a temporary per component: `../<clean path>` (or a clean absolute path).
        if (relative && rest.startsWith("../") && isClean(rest, 3)) return rest.substring(3)
        if (!relative && isClean(rest, 0)) return rest
        val segments = ArrayList<String>()
        if (relative) segments += LibraryFormat.PLAYLISTS_DIR
        for (segment in rest.split('/')) {
            when (segment) {
                "", "." -> Unit
                ".." -> if (segments.isEmpty()) return null else segments.removeAt(segments.lastIndex)
                else -> segments += segment
            }
        }
        return segments.takeIf { it.isNotEmpty() }?.joinToString("/")
    }

    /** Whether `value[from..]` is non-empty and none of its `/`-separated components is empty, `.` or `..`. */
    private fun isClean(value: String, from: Int): Boolean {
        if (from >= value.length) return false
        var start = from
        while (true) {
            val slash = value.indexOf('/', start)
            val end = if (slash < 0) value.length else slash
            val length = end - start
            if (length == 0) return false
            if (value[start] == '.' && (length == 1 || (length == 2 && value[start + 1] == '.'))) return false
            if (slash < 0) return true
            start = slash + 1
        }
    }

    /** Entry line for a library-relative path: relative to `Playlists/`, not URL-encoded. */
    fun entryLine(path: String): String = "../$path"

    /** Whether a library-relative path can be a playlist entry: no component may contain a line break. */
    fun canWrite(path: String): Boolean = !LibraryFormat.hasLineBreak(path)

    fun entryFor(track: Track): PlaylistEntry = PlaylistEntry(path = track.path, raw = entryLine(track.path), extinf = extinf(track))

    /** Longest `#EXTINF` label writers write, in UTF-8 bytes (cut at a character boundary). */
    const val MAX_LABEL_BYTES = 1024

    fun extinf(track: Track): String = "$EXTINF${durationSeconds(track) ?: -1},${cleanLabel("${track.artist} - ${track.title}")}"

    private fun durationSeconds(track: Track): Long? = if (track.durationMs > 0) (track.durationMs + 500) / 1000 else null

    /** [label] cut to [MAX_LABEL_BYTES] at a character boundary, line breaks replaced: tunemesh-core's `clean_label`. */
    fun cleanLabel(label: String): String {
        var end = 0
        var bytes = 0
        while (end < label.length) {
            val codePoint = label.codePointAt(end)
            val length = utf8Length(codePoint)
            if (bytes + length > MAX_LABEL_BYTES) break
            bytes += length
            end += Character.charCount(codePoint)
        }
        return label.substring(0, end).replace('\n', ' ').replace('\r', ' ')
    }

    private fun utf8Length(codePoint: Int): Int = when {
        codePoint < 0x80 -> 1
        codePoint < 0x800 -> 2
        codePoint < 0x10000 -> 3
        else -> 4
    }

    private fun utf8Length(text: String): Long {
        var bytes = 0L
        var i = 0
        while (i < text.length) {
            val codePoint = text.codePointAt(i)
            // An unpaired surrogate is written as U+FFFD (3 bytes).
            bytes += if (Character.isSurrogate(text[i]) && Character.charCount(codePoint) == 1) 3 else utf8Length(codePoint)
            i += Character.charCount(codePoint)
        }
        return bytes
    }

    private val durationPattern = Regex("[+-]?[0-9]+")

    /** Duration and label of an `#EXTINF` line, as tunemesh-core's `parse_extinf` reads them. */
    private fun parseExtinf(line: String): Pair<Long?, String?> {
        val info = line.removePrefix(EXTINF)
        val duration = LibraryFormat.trimWhiteSpace(info.substringBefore(','))
            .takeIf(durationPattern::matches)?.toLongOrNull()?.takeIf { it >= 0 }?.coerceAtMost(MAX_DURATION)
        val label = LibraryFormat.trimWhiteSpace(info.substringAfter(',', "")).takeIf { it.isNotEmpty() }
        return duration to label
    }

    /** Longest duration kept from an `#EXTINF` line (tunemesh-core stores it in 31 bits). */
    private const val MAX_DURATION = (1L shl 31) - 2

    /**
     * Serialises [entries] as tunemesh-core's `serialize`: `#EXTINF` and entry line per entry, LF endings. A known
     * track gets its own duration and `Artist - Title`; otherwise the duration and label read with the entry are
     * kept (label cut to [MAX_LABEL_BYTES]), else the file name. An entry inside the library is written as
     * `../<path>`, one outside it as read. An entry with a line break (a lone CR read from an old file) is dropped:
     * the line would turn into two.
     *
     * A playlist over the limits ([MAX_ENTRIES] entries, a line over [MAX_LINE_BYTES], [MAX_BYTES] in all) is not
     * written: [PlaylistTooLargeException], so readers can always open what writers write.
     */
    fun write(entries: List<PlaylistEntry>, lookup: (String) -> Track?): String {
        if (entries.size > MAX_ENTRIES) throw PlaylistTooLargeException("more than $MAX_ENTRIES entries")
        if (entries.any { utf8Length(it.path ?: it.raw) + 3 > MAX_LINE_BYTES }) {
            throw PlaylistTooLargeException("a line is longer than ${MAX_LINE_BYTES / 1024} KiB")
        }
        var bytes = 0L
        val text = buildString {
            fun line(value: String) {
                append(value).append('\n')
                bytes += utf8Length(value) + 1
                if (bytes > MAX_BYTES) throw PlaylistTooLargeException("larger than ${MAX_BYTES / (1024 * 1024)} MiB")
            }
            line(HEADER)
            for (entry in entries) {
                val path = entry.path
                if (LibraryFormat.hasLineBreak(path ?: entry.raw)) continue
                val track = path?.let(lookup)
                val (readDuration, readLabel) = entry.extinf?.let(::parseExtinf) ?: (null to null)
                val duration = track?.let(::durationSeconds) ?: readDuration
                val label = when {
                    track != null -> cleanLabel("${track.artist} - ${track.title}")
                    readLabel != null -> cleanLabel(readLabel)
                    else -> LibraryFormat.titleFromFileName((path ?: entry.raw).substringAfterLast('/'))
                }
                line("$EXTINF${duration ?: -1},$label")
                line(if (path != null) entryLine(path) else entry.raw)
            }
        }
        return text
    }
}
