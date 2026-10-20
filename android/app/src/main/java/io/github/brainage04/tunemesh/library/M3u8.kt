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
     * Throws [PlaylistTooLargeException] past [maxBytes], [maxEntries] or [MAX_LINE_BYTES]; nothing is truncated.
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

        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            total += count
            if (total > maxBytes) throw PlaylistTooLargeException("larger than ${maxBytes / (1024 * 1024)} MiB")
            var start = 0
            for (i in 0 until count) {
                if (buffer[i] != '\n'.code.toByte()) continue
                line = append(line, lineLength, buffer, start, i)
                lineLength += i - start
                endLine()
                start = i + 1
            }
            line = append(line, lineLength, buffer, start, count)
            lineLength += count - start
        }
        if (lineLength > 0) endLine()
    }

    /** Appends `source[from, to)` to the first [length] bytes of [line], growing it up to [MAX_LINE_BYTES]. */
    private fun append(line: ByteArray, length: Int, source: ByteArray, from: Int, to: Int): ByteArray {
        val needed = length + (to - from)
        if (needed > MAX_LINE_BYTES) throw PlaylistTooLargeException("a line is longer than ${MAX_LINE_BYTES / 1024} KiB")
        val target = if (needed <= line.size) line else line.copyOf(maxOf(needed, minOf(line.size * 2, MAX_LINE_BYTES)))
        System.arraycopy(source, from, target, length, to - from)
        return target
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

    fun extinf(track: Track): String {
        val seconds = if (track.durationMs > 0) (track.durationMs + 500) / 1000 else -1
        return "$EXTINF$seconds,${track.artist} - ${track.title}".replace('\n', ' ').replace('\r', ' ')
    }

    /**
     * Serialises [entries]. Entries whose track is known get a fresh `#EXTINF`; missing ones keep the line they
     * were read with so nothing is lost when rewriting. An entry whose line or path holds a line break (a lone CR
     * read from an old file) is dropped, as tunemesh-core's `serialize` does: writers never write LF or CR in an
     * entry, and the line would turn into two.
     */
    fun write(entries: List<PlaylistEntry>, lookup: (String) -> Track?): String = buildString {
        append(HEADER).append('\n')
        for (entry in entries) {
            if (LibraryFormat.hasLineBreak(entry.raw) || entry.path?.let(LibraryFormat::hasLineBreak) == true) continue
            val track = entry.path?.let(lookup)
            val info = when {
                track != null -> extinf(track)
                entry.extinf != null -> entry.extinf.replace('\n', ' ').replace('\r', ' ')
                else -> "$EXTINF-1,${LibraryFormat.titleFromFileName(entry.raw.substringAfterLast('/'))}"
            }
            append(info).append('\n')
            append(if (entry.path != null && track != null) entryLine(entry.path) else entry.raw).append('\n')
        }
    }
}
