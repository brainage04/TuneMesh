package io.github.brainage04.tunemesh.library

import java.net.URI

/** What a pasted line asks for; decides whether "Save playlists" applies to it. */
enum class InputKind { Playlist, Album, Single }

/** Turns pasted text (one link or search per line) into download requests; matches the desktop app. */
object DownloadInputs {
    /** Trimmed non-blank lines, duplicates within one paste dropped, in paste order. */
    fun lines(text: String): List<String> = text.lineSequence().map { it.trim() }.filter { it.isNotEmpty() }.distinct().toList()

    fun kind(line: String): InputKind {
        val uri = runCatching { URI(line) }.getOrNull() ?: return InputKind.Single
        if (uri.scheme?.lowercase() !in setOf("http", "https")) return InputKind.Single
        val host = uri.host?.lowercase()?.removePrefix("www.")?.removePrefix("m.") ?: return InputKind.Single
        val segments = uri.path.orEmpty().split('/').filter { it.isNotEmpty() }
        return when (host) {
            "open.spotify.com" -> {
                val kind = segments.dropWhile { it.startsWith("intl-") }.firstOrNull()
                when (kind) {
                    "playlist" -> InputKind.Playlist
                    "album" -> InputKind.Album
                    else -> InputKind.Single
                }
            }
            "youtube.com", "music.youtube.com", "youtu.be" -> {
                val list = queryParameter(uri, "list")
                when {
                    segments.firstOrNull() == "browse" && segments.getOrNull(1)?.startsWith("MPREb") == true -> InputKind.Album
                    list.isNullOrEmpty() -> InputKind.Single
                    list.startsWith("OLAK5uy_") -> InputKind.Album
                    else -> InputKind.Playlist
                }
            }
            else -> InputKind.Single
        }
    }

    /** "Save playlists" starts switched on only when a line is a playlist link; albums already show up as albums. */
    fun saveAsPlaylistsByDefault(lines: List<String>): Boolean = lines.any { kind(it) == InputKind.Playlist }

    /** `playlistFromSource` for one line: only collections get a playlist, and a chosen target playlist wins. */
    fun playlistFromSource(line: String, targetPlaylist: String?, saveAsPlaylists: Boolean): Boolean =
        saveAsPlaylists && targetPlaylist.isNullOrBlank() && kind(line) != InputKind.Single

    private fun queryParameter(uri: URI, name: String): String? =
        uri.rawQuery?.split('&')?.firstNotNullOfOrNull { pair ->
            val key = pair.substringBefore('=')
            if (key == name) java.net.URLDecoder.decode(pair.substringAfter('=', ""), "UTF-8") else null
        }
}
