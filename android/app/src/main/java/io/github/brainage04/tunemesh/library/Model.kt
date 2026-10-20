package io.github.brainage04.tunemesh.library

import kotlinx.serialization.Serializable

/** Tags read from a file; null fields fall back to path-derived values. */
@Serializable
data class RawTags(
    val title: String? = null,
    val artist: String? = null,
    val album: String? = null,
    val albumArtist: String? = null,
    val trackNumber: Int? = null,
    val discNumber: Int? = null,
    val durationMs: Long = 0,
)

@Serializable
data class Track(
    /** Library-relative path with `/` separators; the track identity. */
    val path: String,
    val title: String,
    val artist: String,
    val album: String,
    val albumArtist: String,
    val trackNumber: Int?,
    val discNumber: Int?,
    val durationMs: Long,
    val size: Long,
    val modifiedMs: Long,
) {
    val albumKey: String get() = albumKey(albumArtist, album)
    val directory: String get() = path.substringBeforeLast('/', "")

    companion object {
        fun albumKey(albumArtist: String, album: String): String = "$albumArtist\u0000$album"

        fun fromTags(path: String, tags: RawTags, size: Long, modifiedMs: Long): Track {
            val title = tags.title?.takeIf { it.isNotBlank() }?.trim()
                ?: LibraryFormat.titleFromFileName(path.substringAfterLast('/'))
            val artist = tags.artist?.takeIf { it.isNotBlank() }?.trim() ?: LibraryFormat.fallbackArtist(path)
            val album = tags.album?.takeIf { it.isNotBlank() }?.trim() ?: LibraryFormat.fallbackAlbum(path)
            val albumArtist = tags.albumArtist?.takeIf { it.isNotBlank() }?.trim()
                ?: LibraryFormat.firstArtist(artist)
            return Track(
                path = path,
                title = title,
                artist = artist,
                album = album,
                albumArtist = albumArtist,
                trackNumber = tags.trackNumber,
                discNumber = tags.discNumber,
                durationMs = tags.durationMs,
                size = size,
                modifiedMs = modifiedMs,
            )
        }
    }
}

data class Album(
    val key: String,
    val title: String,
    val artist: String,
    val tracks: List<Track>,
    /** Newest file modification time, for "recently added". */
    val addedMs: Long,
)

data class Artist(
    val name: String,
    val albums: List<Album>,
    val tracks: List<Track>,
)

/** One line of a playlist. [path] is null when the entry does not point inside the library. */
data class PlaylistEntry(
    val path: String?,
    val raw: String,
    val extinf: String? = null,
)

data class Playlist(
    val name: String,
    val entries: List<PlaylistEntry>,
    val modifiedMs: Long,
) {
    val isLikedSongs: Boolean get() = name == LibraryFormat.LIKED_SONGS
}
