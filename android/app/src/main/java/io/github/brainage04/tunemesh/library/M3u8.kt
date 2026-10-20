package io.github.brainage04.tunemesh.library

/** Reads and writes `Playlists/<Name>.m3u8` files as described in the library contract. */
object M3u8 {
    private const val HEADER = "#EXTM3U"
    private const val EXTINF = "#EXTINF:"

    /**
     * Parses playlist [text]. [root] is the absolute library root without a trailing slash; it is only used
     * to resolve absolute entries.
     */
    fun parse(text: String, root: String): List<PlaylistEntry> {
        val entries = mutableListOf<PlaylistEntry>()
        var pendingExtinf: String? = null
        for (rawLine in text.removePrefix("\uFEFF").split('\n')) {
            // As Rust's str::lines plus trim_end_matches('\r'): CRLF endings go, a CR inside a line stays.
            val line = rawLine.trimEnd('\r')
            if (line.isBlank()) continue
            if (line.startsWith("#")) {
                if (line.startsWith(EXTINF)) pendingExtinf = line
                continue
            }
            entries += PlaylistEntry(path = resolve(line.trim(), root), raw = line, extinf = pendingExtinf)
            pendingExtinf = null
        }
        return entries
    }

    /** Library-relative path for an entry line, or null when it points outside the library. */
    fun resolve(line: String, root: String): String? {
        val trimmedRoot = root.trimEnd('/')
        val combined = when {
            line.startsWith("/") -> {
                if (!line.startsWith("$trimmedRoot/")) return null
                line.removePrefix("$trimmedRoot/")
            }
            else -> "${LibraryFormat.PLAYLISTS_DIR}/$line"
        }
        val segments = ArrayDeque<String>()
        for (segment in combined.split('/')) {
            when (segment) {
                "", "." -> Unit
                ".." -> if (segments.isEmpty()) return null else segments.removeLast()
                else -> segments.addLast(segment)
            }
        }
        return segments.takeIf { it.isNotEmpty() }?.joinToString("/")
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
