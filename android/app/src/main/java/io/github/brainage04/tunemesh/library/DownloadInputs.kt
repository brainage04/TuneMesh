package io.github.brainage04.tunemesh.library

import java.util.Locale

/** What a pasted line asks for; decides whether "Save playlists" applies to it. */
enum class InputKind {
    Playlist,
    Album,
    Single,
}

/**
 * Turns pasted text (one link or search per line) into download requests, as the desktop app does. YouTube links follow
 * `docs/library-format.md` ("YouTube links"), which the worker uses to decide what to download.
 */
object DownloadInputs {
    /** Trimmed non-blank lines, duplicates within one paste dropped, in paste order. */
    fun lines(text: String): List<String> = text.lineSequence().map { it.trim() }.filter { it.isNotEmpty() }.distinct().toList()

    private val url = Regex("""https?://\S+""")

    /** What a share to TuneMesh asks for: the first URL in the shared [text], or the whole text, trimmed, as a search. */
    fun sharedInput(text: String): String {
        val trimmed = text.trim()
        return url.find(trimmed)?.value?.let(::trimTrailingPunctuation) ?: trimmed
    }

    private const val SENTENCE_END = ".,;:!?'\">"
    private const val OPENING = "([{"
    private const val CLOSING = ")]}"

    /**
     * [url] without the punctuation that ends the sentence around it (`.,;:!?'"` and closing brackets). A closing bracket stays when the
     * URL opened it, as in `https://en.wikipedia.org/wiki/Song_(band)`. Linear in the length of [url]: shared text has no length limit.
     */
    internal fun trimTrailingPunctuation(url: String): String {
        val opened = IntArray(OPENING.length)
        val closed = IntArray(CLOSING.length)
        for (char in url) {
            OPENING.indexOf(char).takeIf { it >= 0 }?.let { opened[it]++ }
            CLOSING.indexOf(char).takeIf { it >= 0 }?.let { closed[it]++ }
        }
        var end = url.length
        while (end > 0) {
            val char = url[end - 1]
            val bracket = CLOSING.indexOf(char)
            val keep =
                when {
                    char in SENTENCE_END -> false
                    bracket >= 0 -> opened[bracket] >= closed[bracket]
                    else -> true
                }
            if (keep) break
            if (bracket >= 0) closed[bracket]--
            end--
        }
        return url.substring(0, end)
    }

    fun kind(input: String): InputKind {
        val line = LibraryFormat.trimWhiteSpace(input)
        val lower = line.lowercase(Locale.ROOT)
        val schemeEnd =
            when {
                lower.startsWith("https://") -> 8
                lower.startsWith("http://") -> 7
                else -> return InputKind.Single
            }
        val hostEnd = lower.indexOfAny(charArrayOf('/', '?', '#'), schemeEnd).let { if (it < 0) lower.length else it }
        val host = lower.substring(schemeEnd, hostEnd).removePrefix("www.").removePrefix("m.")
        val path =
            if (hostEnd < lower.length && lower[hostEnd] == '/') {
                lower.substring(hostEnd + 1).substringBefore('?').substringBefore('#')
            } else {
                ""
            }
        return when (host) {
            "open.spotify.com" -> {
                // `/intl-de/playlist/…` and `/playlist/…`
                val segments = path.split('/').filter { it.isNotEmpty() }
                val first = segments.firstOrNull().orEmpty()
                when (if (first.startsWith("intl-")) segments.getOrNull(1) else first) {
                    "playlist" -> InputKind.Playlist
                    "album" -> InputKind.Album
                    else -> InputKind.Single
                }
            }
            "youtube.com",
            "music.youtube.com",
            "youtu.be" ->
                when {
                    path.startsWith("browse/mpreb") -> InputKind.Album
                    else -> youTubeList(queryParameter(line, "list"))
                }
            else -> InputKind.Single
        }
    }

    /** The `list=` rule of `docs/library-format.md` ("YouTube links"). */
    private fun youTubeList(list: String?): InputKind =
        when {
            list.isNullOrEmpty() -> InputKind.Single
            list.startsWith("OLAK5uy_") -> InputKind.Album
            list.startsWith("RDCLAK5uy_") -> InputKind.Playlist
            // An auto-generated mix: endless and personal, so only the video itself.
            list.startsWith("RD") -> InputKind.Single
            else -> InputKind.Playlist
        }

    /** "Save playlists" starts switched on only when a line is a playlist link; albums already show up as albums. */
    fun saveAsPlaylistsByDefault(lines: List<String>): Boolean = lines.any { kind(it) == InputKind.Playlist }

    /** `playlistFromSource` for one line: only collections get a playlist, and a chosen target playlist wins. */
    fun playlistFromSource(line: String, targetPlaylist: String?, saveAsPlaylists: Boolean): Boolean =
        saveAsPlaylists && targetPlaylist.isNullOrBlank() && kind(line) != InputKind.Single

    /** First `name=` value in the query (after the first `?`, before any `#`), case-sensitive and not decoded. */
    private fun queryParameter(url: String, name: String): String? {
        val query = url.substringAfter('?', "").takeIf { '?' in url } ?: return null
        return query.substringBefore('#').split('&').firstNotNullOfOrNull { pair ->
            if (pair.startsWith("$name=")) pair.substring(name.length + 1) else null
        }
    }
}
