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
) {
    companion object {
        /** A tag text field as kept: trimmed and cut to [Limits.MAX_TAG_TEXT_BYTES]. */
        fun boundedText(value: String): String = LibraryFormat.trimWhiteSpace(Limits.cutUtf8(LibraryFormat.trimWhiteSpace(value), Limits.MAX_TAG_TEXT_BYTES))

        /** An artist field with at most [Limits.MAX_ARTISTS] artists (`;` or NUL separated); the rest is dropped. */
        fun boundedArtists(value: String): String {
            var count = 0
            for (i in value.indices) {
                if ((value[i] == ';' || value[i] == '\u0000') && ++count == Limits.MAX_ARTISTS) return value.substring(0, i)
            }
            return value
        }
    }
}

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
            // Bounded here too, so tags cached by an older version follow the limits as well.
            fun text(value: String?) = value?.let(RawTags::boundedText)?.takeIf { it.isNotEmpty() }
            val title = text(tags.title) ?: LibraryFormat.titleFromFileName(path.substringAfterLast('/'))
            val artist = text(tags.artist)?.let(RawTags::boundedArtists)?.takeIf { it.isNotBlank() }
                ?: LibraryFormat.fallbackArtist(path)
            val album = text(tags.album) ?: LibraryFormat.fallbackAlbum(path)
            val albumArtist = text(tags.albumArtist)?.let(RawTags::boundedArtists)?.takeIf { it.isNotBlank() }
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

/**
 * One line of a playlist. [path] is null when the entry does not point inside the library. Kept small, since a
 * playlist can hold hundreds of thousands of them: the entry line is stored only when it is not the canonical
 * `../<path>` that [raw] derives, and [extinf] only when the reader asked for it.
 */
/**
 * A playlist entry as a screen showed it: its [position] then and its [PlaylistEntry.key]. Changes find the entry
 * again in the file as it is now (tunemesh-core's desktop `EntryRef`).
 */
data class EntryRef(val position: Int, val key: String)

class PlaylistEntry private constructor(
    val path: String?,
    private val line: String?,
    val extinf: String?,
) {
    /** The entry line as read or written, relative to `Playlists/` unless absolute. */
    val raw: String get() = line ?: M3u8.entryLine(path!!)

    /** What identifies the entry when the file changes: its library path, or the line as written when it points outside. */
    val key: String get() = path ?: raw

    override fun equals(other: Any?): Boolean =
        other is PlaylistEntry && path == other.path && raw == other.raw && extinf == other.extinf

    override fun hashCode(): Int = (path.hashCode() * 31 + raw.hashCode()) * 31 + extinf.hashCode()

    override fun toString(): String = "PlaylistEntry(path=$path, raw=$raw, extinf=$extinf)"

    companion object {
        operator fun invoke(path: String?, raw: String, extinf: String? = null): PlaylistEntry =
            PlaylistEntry(path, raw.takeUnless { path != null && raw.length == path.length + 3 && raw == M3u8.entryLine(path) }, extinf)
    }
}

/**
 * A playlist as lists show it. It carries no entries, so the app state does not grow with playlist length;
 * [PlaylistContents] holds the entries of the playlist that is open.
 */
data class Playlist(
    val name: String,
    /** Number of entries, available or not. */
    val size: Int,
    /** The first few entries that point inside the library, in order, for the playlist's cover art. */
    val artPaths: List<String>,
    val modifiedMs: Long,
    /** Why the playlist cannot be opened or changed on this device (too large), or null. */
    val problem: String? = null,
) {
    val isLikedSongs: Boolean get() = name == LibraryFormat.LIKED_SONGS
}

/** The entries of one playlist, read when it is opened or changed. */
data class PlaylistContents(
    val name: String,
    val entries: List<PlaylistEntry>,
    val modifiedMs: Long,
) {
    val isLikedSongs: Boolean get() = name == LibraryFormat.LIKED_SONGS
}
