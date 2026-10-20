package io.github.brainage04.tunemesh.library

import java.util.Locale

/** What a pasted line asks for; decides whether "Save playlists" applies to it. */
enum class InputKind { Playlist, Album, Single }

/**
 * Turns pasted text (one link or search per line) into download requests, as the desktop app does. YouTube links
 * follow `docs/library-format.md` ("YouTube links"), which the worker uses to decide what to download.
 */
object DownloadInputs {
    /** Trimmed non-blank lines, duplicates within one paste dropped, in paste order. */
    fun lines(text: String): List<String> = text.lineSequence().map { it.trim() }.filter { it.isNotEmpty() }.distinct().toList()

    fun kind(input: String): InputKind {
        val line = LibraryFormat.trimWhiteSpace(input)
        val lower = line.lowercase(Locale.ROOT)
        val schemeEnd = when {
            lower.startsWith("https://") -> 8
            lower.startsWith("http://") -> 7
            else -> return InputKind.Single
        }
        val hostEnd = lower.indexOfAny(charArrayOf('/', '?', '#'), schemeEnd).let { if (it < 0) lower.length else it }
        val host = lower.substring(schemeEnd, hostEnd).removePrefix("www.").removePrefix("m.")
        val path = if (hostEnd < lower.length && lower[hostEnd] == '/') {
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
            "youtube.com", "music.youtube.com", "youtu.be" -> when {
                path.startsWith("browse/mpreb") -> InputKind.Album
                else -> youTubeList(queryParameter(line, "list"))
            }
            else -> InputKind.Single
        }
    }

    /** The `list=` rule of `docs/library-format.md` ("YouTube links"). */
    private fun youTubeList(list: String?): InputKind = when {
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
