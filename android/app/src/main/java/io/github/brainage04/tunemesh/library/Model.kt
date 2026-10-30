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
        fun boundedText(value: String): String =
            LibraryFormat.trimWhiteSpace(Limits.cutUtf8(LibraryFormat.trimWhiteSpace(value), Limits.MAX_TAG_TEXT_BYTES))

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
    val albumKey: String
        get() = albumKey(albumArtist, album)

    val directory: String
        get() = path.substringBeforeLast('/', "")

    companion object {
        fun albumKey(albumArtist: String, album: String): String = "$albumArtist\u0000$album"

        fun fromTags(path: String, tags: RawTags, size: Long, modifiedMs: Long): Track {
            // Bounded here too, so tags cached by an older version follow the limits as well.
            fun text(value: String?) = value?.let(RawTags::boundedText)?.takeIf { it.isNotEmpty() }
            val title = text(tags.title) ?: LibraryFormat.titleFromFileName(path.substringAfterLast('/'))
            val artist = text(tags.artist)?.let(RawTags::boundedArtists)?.takeIf { it.isNotBlank() } ?: LibraryFormat.fallbackArtist(path)
            val album = text(tags.album) ?: LibraryFormat.fallbackAlbum(path)
            val albumArtist =
                text(tags.albumArtist)?.let(RawTags::boundedArtists)?.takeIf { it.isNotBlank() } ?: LibraryFormat.firstArtist(artist)
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

/**
 * A playlist entry as a screen showed it: its [position] then and its [PlaylistEntry.key]. Changes find the entry again in the file as it
 * is now (tunemesh-core's desktop `EntryRef`).
 */
data class EntryRef(val position: Int, val key: String)

/**
 * One line of a playlist. [path] is null when the entry does not point inside the library. Playlists hold their entries in
 * [PlaylistEntries]; a PlaylistEntry is made when one is needed (a visible row, a write).
 */
class PlaylistEntry
private constructor(
    val path: String?,
    private val line: String?,
    val extinf: String?,
    /** The library index [ordinal] belongs to (set by the reader), so [PlaylistEntries] can keep the ordinal instead of the path. */
    internal val library: LibraryIndex?,
    /** [path]'s track in [library], or -1. */
    internal val ordinal: Int,
) {
    /** The entry line as read or written, relative to `Playlists/` unless absolute. */
    val raw: String
        get() = line ?: M3u8.entryLine(checkNotNull(path) { "an entry without a stored line is a library path" })

    /** What identifies the entry when the file changes: its library path, or the line as written when it points outside. */
    val key: String
        get() = path ?: raw

    override fun equals(other: Any?): Boolean = other is PlaylistEntry && path == other.path && raw == other.raw && extinf == other.extinf

    override fun hashCode(): Int = (path.hashCode() * 31 + raw.hashCode()) * 31 + extinf.hashCode()

    @Suppress("NullableToStringCall") // debug text: a missing path or #EXTINF shows as null
    override fun toString(): String = "PlaylistEntry(path=$path, raw=$raw, extinf=$extinf)"

    companion object {
        operator fun invoke(path: String?, raw: String, extinf: String? = null): PlaylistEntry =
            read(path, raw, extinf, null, LibraryIndex.NONE)

        internal fun read(path: String?, raw: String, extinf: String?, library: LibraryIndex?, ordinal: Int): PlaylistEntry =
            PlaylistEntry(
                path,
                raw.takeUnless { path != null && raw.length == path.length + 3 && raw == M3u8.entryLine(path) },
                extinf,
                library.takeIf { ordinal >= 0 },
                if (library != null) ordinal else LibraryIndex.NONE,
            )
    }

    /** Whether [raw] is `../<path>`, so it need not be stored. */
    internal val isCanonical: Boolean
        get() = line == null
}

/**
 * The entries of one playlist, stored compactly: up to 250,000 of them stay in memory while a playlist is open. Per entry there is one int
 * (the ordinal of its track in the [library] index it was read with, or -1) and one end offset into a UTF-8 buffer holding the entry line
 * when it must be kept: always for an entry that is not a library track, and for a library track whose line is not `../<path>`. A library
 * entry therefore costs 8 bytes, any other entry 8 bytes plus its line's UTF-8 bytes. `#EXTINF` lines are kept only for writers
 * ([hasExtinf]).
 *
 * [get] makes a [PlaylistEntry] on demand; [path], [raw], [key] and [track] read one field without making it.
 */
@Suppress("TooManyFunctions") // one accessor per field of an entry, without making it
class PlaylistEntries
@Suppress("LongParameterList") // the compact arrays
private constructor(
    private val root: String,
    /** The index [tracks] refer to; null when none does. */
    val library: LibraryIndex?,
    private val tracks: IntArray,
    private val ends: IntArray,
    private val bytes: ByteArray,
    private val extinfs: Array<String?>?,
    override val size: Int,
) : AbstractList<PlaylistEntry>(), RandomAccess {
    val hasExtinf: Boolean
        get() = extinfs != null

    private fun start(index: Int) = if (index == 0) 0 else ends[index - 1]

    /** The stored line of entry [index]; empty when the entry is `../<path>` of its track ([isCanonical]). */
    private fun storedLine(index: Int): String {
        val start = start(index)
        return String(bytes, start, ends[index] - start, Charsets.UTF_8)
    }

    /** Whether entry [index] is a library track whose line is `../<path>`, so no line is stored. */
    private fun isCanonical(index: Int): Boolean = tracks[index] >= 0 && start(index) == ends[index]

    private fun check(index: Int) {
        if (index < 0 || index >= size) throw IndexOutOfBoundsException("$index of $size")
    }

    /** The path of entry [index]'s library track (null when it has none in [library]). */
    private fun libraryPath(index: Int): String? = tracks[index].takeIf { it >= 0 }?.let { checkNotNull(library).path(it) }

    /** The ordinal of entry [index]'s track in [library], or -1 (not a library track, or read without the index). */
    fun track(index: Int): Int {
        check(index)
        return tracks[index]
    }

    /** Library-relative path of entry [index], or null when it points outside the library. */
    fun path(index: Int): String? {
        check(index)
        return libraryPath(index) ?: M3u8.resolve(storedLine(index), root)
    }

    /** The entry line as read or written. */
    fun raw(index: Int): String {
        check(index)
        return if (isCanonical(index)) M3u8.entryLine(checkNotNull(libraryPath(index))) else storedLine(index)
    }

    /** [PlaylistEntry.key] of entry [index]. */
    fun key(index: Int): String {
        check(index)
        libraryPath(index)?.let {
            return it
        }
        val line = storedLine(index)
        return M3u8.resolve(line, root) ?: line
    }

    fun extinf(index: Int): String? {
        check(index)
        return extinfs?.get(index)
    }

    override fun get(index: Int): PlaylistEntry {
        check(index)
        val extinf = extinfs?.get(index)
        val path = libraryPath(index)
        if (path != null) {
            val raw = if (isCanonical(index)) M3u8.entryLine(path) else storedLine(index)
            return PlaylistEntry.read(path, raw, extinf, library, tracks[index])
        }
        val line = storedLine(index)
        return PlaylistEntry.read(M3u8.resolve(line, root), line, extinf, null, LibraryIndex.NONE)
    }

    /** A builder for a changed copy, with the same root, index and `#EXTINF` choice. */
    fun builder(capacity: Int = size + 1) = Builder(root, hasExtinf, capacity, library)

    /** Collects entries; [library] is the index the track ordinals of the result refer to. */
    class Builder(private val root: String, keepExtinf: Boolean, capacity: Int = 16, private val library: LibraryIndex? = null) {
        private var tracks = IntArray(maxOf(capacity, 4))
        private var ends = IntArray(tracks.size)
        private var bytes = ByteArray(64)
        private var extinfs: Array<String?>? = if (keepExtinf) arrayOfNulls(tracks.size) else null
        private var length = 0
        private var size = 0

        private fun grow() {
            if (size < tracks.size) return
            val capacity = tracks.size * 2
            tracks = tracks.copyOf(capacity)
            ends = ends.copyOf(capacity)
            extinfs = extinfs?.copyOf(capacity)
        }

        private fun append(source: ByteArray, from: Int, count: Int) {
            if (length + count > bytes.size) bytes = bytes.copyOf(maxOf(length + count, bytes.size * 2))
            System.arraycopy(source, from, bytes, length, count)
            length += count
        }

        private fun append(line: String) {
            val encoded = line.toByteArray(Charsets.UTF_8)
            append(encoded, 0, encoded.size)
        }

        private fun push(track: Int, extinf: String?) {
            tracks[size] = track
            ends[size] = length
            extinfs?.set(size, extinf)
            size++
        }

        /**
         * Adds [entry]. [track] is the ordinal of its path in [library], or -1 when it has none; entries read with that index know it
         * themselves.
         */
        fun add(entry: PlaylistEntry, track: Int = if (entry.library === library) entry.ordinal else LibraryIndex.NONE): Builder {
            grow()
            if (track < 0 || !entry.isCanonical) append(entry.raw)
            push(track, entry.extinf)
            return this
        }

        /** Adds entry [index] of [from] as it is stored (an entry of another index is looked up in this one by its path). */
        fun add(from: PlaylistEntries, index: Int): Builder {
            from.check(index)
            grow()
            val start = from.start(index)
            val track = from.tracks[index]
            when {
                from.library === library || track < 0 -> {
                    append(from.bytes, start, from.ends[index] - start)
                    push(if (from.library === library) track else LibraryIndex.NONE, from.extinfs?.get(index))
                }
                else -> {
                    val path = checkNotNull(from.library).path(track)
                    val mine = library?.indexOf(path) ?: LibraryIndex.NONE
                    if (from.isCanonical(index)) {
                        if (mine < 0) append(M3u8.entryLine(path))
                    } else {
                        append(from.bytes, start, from.ends[index] - start)
                    }
                    push(mine, from.extinfs?.get(index))
                }
            }
            return this
        }

        /** Adds every entry of [from] in [range]. */
        fun addAll(from: PlaylistEntries, range: IntRange = 0 until from.size): Builder {
            for (index in range) add(from, index)
            return this
        }

        fun build(): PlaylistEntries =
            PlaylistEntries(root, library, tracks.copyOf(size), ends.copyOf(size), bytes.copyOf(length), extinfs?.copyOf(size), size)
    }

    companion object {
        fun empty(root: String, keepExtinf: Boolean, library: LibraryIndex? = null): PlaylistEntries =
            Builder(root, keepExtinf, 0, library).build()
    }
}

/**
 * A playlist as lists show it. It carries no entries, so the app state does not grow with playlist length; [PlaylistContents] holds the
 * entries of the playlist that is open.
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
    val isLikedSongs: Boolean
        get() = name == LibraryFormat.LIKED_SONGS
}

/**
 * The entries of one playlist, read when it is opened or changed. Equal only to itself: comparing two copies of a large playlist entry by
 * entry would cost more than treating a re-read as a change.
 */
class PlaylistContents(
    val name: String,
    val entries: PlaylistEntries,
    val modifiedMs: Long,
) {
    val isLikedSongs: Boolean
        get() = name == LibraryFormat.LIKED_SONGS
}
