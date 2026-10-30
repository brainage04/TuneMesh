package io.github.brainage04.spotless.library

/**
 * Albums, artists and search keys derived from a scan. Immutable; a scan that changes nothing keeps the index it had.
 *
 * Stored compactly, as [PlaylistEntries] are: every text once, as UTF-8 in one buffer with end offsets (an artist or album name is stored
 * once for all its tracks), numbers in int and long arrays, tracks, albums and artists as ordinals, and open-addressing int tables for the
 * lookups by path, album key and artist name. The tracks go album by album, so an album is a range of them. 100,000 tracks take about 12 MB
 * in some 35 arrays instead of about 1.4 million objects, so the garbage collector has little to mark and nothing large to move, and the
 * scanner saves and loads the arrays as they are ([LibraryIndexFile]). [Track], [Album] and [Artist] objects are made when asked for: the
 * lists ([tracks], [albums], [artists], search results) make one per element read.
 */
@Suppress("TooManyFunctions") // the index's accessors, one per field
class LibraryIndex
@Suppress("LongParameterList") // the index's arrays, saved and loaded as they are
internal constructor(
    internal val text: ByteArray,
    internal val textEnds: IntArray,
    /** Per text, its search key ([LibraryFormat.searchKey]) as UTF-8; empty for paths. */
    internal val keys: ByteArray,
    internal val keyEnds: IntArray,
    internal val trackPath: IntArray,
    internal val trackTitle: IntArray,
    internal val trackArtist: IntArray,
    internal val trackAlbum: IntArray,
    internal val trackNumbers: IntArray,
    internal val discNumbers: IntArray,
    /** [HAS_TRACK_NUMBER] and [HAS_DISC_NUMBER] per track. */
    internal val trackFlags: ByteArray,
    internal val durations: LongArray,
    internal val sizes: LongArray,
    internal val modified: LongArray,
    /** Track ordinals + 1 by path hash. */
    internal val pathSlots: IntArray,
    /** Track ordinals in title order, for search results. */
    internal val byTitle: IntArray,
    internal val albumTitle: IntArray,
    internal val albumArtist: IntArray,
    /** Album `a` is the tracks `albumStart[a] until albumStart[a + 1]`. */
    internal val albumStart: IntArray,
    internal val albumAdded: LongArray,
    internal val albumDuration: LongArray,
    /** Album ordinals + 1 by key hash. */
    internal val albumSlots: IntArray,
    /** Album ordinals, newest first. */
    internal val recentAlbums: IntArray,
    internal val artistName: IntArray,
    /** Artist `r` has the albums `artistAlbums[artistAlbumStart[r] until artistAlbumStart[r + 1]]`. */
    internal val artistAlbumStart: IntArray,
    internal val artistAlbums: IntArray,
    /** Per entry of [artistAlbums], how many tracks its artist has up to and including that album. */
    internal val artistTrackEnds: IntArray,
    /** Artist ordinals + 1 by name hash. */
    internal val artistSlots: IntArray,
    /** The [HOME_ROW] artists with the newest albums, newest first. */
    internal val recentArtistOrder: IntArray,
) {
    val size: Int
        get() = trackPath.size

    // ---- texts ---------------------------------------------------------------------------------------------------

    private fun textStart(id: Int) = if (id == 0) 0 else textEnds[id - 1]

    internal fun string(id: Int): String {
        val start = textStart(id)
        return String(text, start, textEnds[id] - start, Charsets.UTF_8)
    }

    // ---- tracks --------------------------------------------------------------------------------------------------

    /** All tracks, album by album. */
    val tracks: List<Track> = TrackList(null, 0, size)

    /** Library-relative path of track [ordinal]. */
    fun path(ordinal: Int): String = string(trackPath[ordinal])

    /** The track at [ordinal] (0 until [size]). */
    fun track(ordinal: Int): Track {
        val album = trackAlbum[ordinal]
        val flags = trackFlags[ordinal].toInt()
        return Track(
            path = string(trackPath[ordinal]),
            title = string(trackTitle[ordinal]),
            artist = string(trackArtist[ordinal]),
            album = string(albumTitle[album]),
            albumArtist = string(albumArtist[album]),
            trackNumber = trackNumbers[ordinal].takeIf { flags and HAS_TRACK_NUMBER != 0 },
            discNumber = discNumbers[ordinal].takeIf { flags and HAS_DISC_NUMBER != 0 },
            durationMs = durations[ordinal],
            size = sizes[ordinal],
            modifiedMs = modified[ordinal],
        )
    }

    /** The first ordinal + 1 in [slots] (an open-addressing table) from [hash] on that [matches], or -1 at an empty slot. */
    private inline fun find(slots: IntArray, hash: Int, matches: (Int) -> Boolean): Int {
        var slot = hash and (slots.size - 1)
        var ordinal = slots[slot] - 1
        while (ordinal >= 0 && !matches(ordinal)) {
            slot = (slot + 1) and (slots.size - 1)
            ordinal = slots[slot] - 1
        }
        return ordinal
    }

    /** Whether text [id] is [bytes]. */
    private fun textIs(id: Int, bytes: ByteArray) = Utf8.equal(text, textStart(id), textEnds[id], bytes, 0, bytes.size)

    /** The ordinal of the track at library-relative [path], or -1. */
    fun indexOf(path: String): Int {
        val bytes = path.toByteArray(Charsets.UTF_8)
        return find(pathSlots, Utf8.hash(bytes, 0, bytes.size)) { textIs(trackPath[it], bytes) }
    }

    /** Duration of track [ordinal], without making the track. */
    fun durationMs(ordinal: Int): Long = durations[ordinal]

    /** The track at library-relative [path], or null. */
    fun track(path: String): Track? = indexOf(path).takeIf { it >= 0 }?.let(::track)

    /** Size and modification time of track [ordinal], as the scanner compares them. */
    internal fun isUnchanged(ordinal: Int, size: Long, modifiedMs: Long) = sizes[ordinal] == size && modified[ordinal] == modifiedMs

    /** The tracks at [ordinals] (all of them when null) between [from] and [to]. */
    private inner class TrackList(private val ordinals: IntArray?, private val from: Int, private val to: Int) :
        AbstractList<Track>(), RandomAccess {
        override val size: Int
            get() = to - from

        override fun get(index: Int): Track {
            if (index < 0 || index >= size) throw IndexOutOfBoundsException("$index of $size")
            return track(ordinals?.get(from + index) ?: (from + index))
        }
    }

    /** The tracks at [ordinals], as a list that makes each when read. */
    fun tracksAt(ordinals: IntArray): List<Track> = TrackList(ordinals, 0, ordinals.size)

    // ---- albums --------------------------------------------------------------------------------------------------

    /** Albums by title, ignoring case. */
    val albums: List<Album> = AlbumList(null)

    /** Albums, newest first. */
    val recentlyAdded: List<Album> = AlbumList(recentAlbums)

    /** The album with [Album.key] [key], or null. */
    fun album(key: String): Album? {
        val bytes = key.toByteArray(Charsets.UTF_8)
        return find(albumSlots, Utf8.hash(bytes, 0, bytes.size)) { albumKeyEquals(it, bytes) }.takeIf { it >= 0 }?.let { Album(this, it) }
    }

    /** Whether album [ordinal]'s key (`<artist> NUL <title>`) is [key]. */
    private fun albumKeyEquals(ordinal: Int, key: ByteArray): Boolean {
        val artist = albumArtist[ordinal]
        val title = albumTitle[ordinal]
        val artistLength = textEnds[artist] - textStart(artist)
        return artistLength + 1 + (textEnds[title] - textStart(title)) == key.size &&
            Utf8.equal(text, textStart(artist), textEnds[artist], key, 0, artistLength) &&
            key[artistLength] == 0.toByte() &&
            Utf8.equal(text, textStart(title), textEnds[title], key, artistLength + 1, key.size)
    }

    internal fun albumTracks(ordinal: Int): List<Track> = TrackList(null, albumStart[ordinal], albumStart[ordinal + 1])

    internal fun albumTrackCount(ordinal: Int): Int = albumStart[ordinal + 1] - albumStart[ordinal]

    private inner class AlbumList(private val ordinals: IntArray?) : AbstractList<Album>(), RandomAccess {
        override val size: Int
            get() = ordinals?.size ?: albumTitle.size

        override fun get(index: Int): Album {
            if (index < 0 || index >= size) throw IndexOutOfBoundsException("$index of $size")
            return Album(this@LibraryIndex, ordinals?.get(index) ?: index)
        }
    }

    // ---- artists -------------------------------------------------------------------------------------------------

    /** Album artists by name, ignoring case. */
    val artists: List<Artist> = ArtistList(null)

    /** The [HOME_ROW] artists with the newest albums, newest first. */
    val recentArtists: List<Artist> = ArtistList(recentArtistOrder)

    /** The artist named [name], or null. */
    fun artist(name: String): Artist? {
        val bytes = name.toByteArray(Charsets.UTF_8)
        return find(artistSlots, Utf8.hash(bytes, 0, bytes.size)) { textIs(artistName[it], bytes) }
            .takeIf { it >= 0 }
            ?.let { Artist(this, it) }
    }

    internal fun artistAlbumList(ordinal: Int): List<Album> =
        AlbumList(artistAlbums.copyOfRange(artistAlbumStart[ordinal], artistAlbumStart[ordinal + 1]))

    internal fun artistTracks(ordinal: Int): List<Track> = ArtistTracks(ordinal)

    /** An artist's tracks, album after album ([artistTrackEnds] counts them per artist). */
    private inner class ArtistTracks(artist: Int) : AbstractList<Track>(), RandomAccess {
        private val first = artistAlbumStart[artist]
        private val last = artistAlbumStart[artist + 1]

        override val size: Int = if (last == first) 0 else artistTrackEnds[last - 1]

        override fun get(index: Int): Track {
            if (index < 0 || index >= size) throw IndexOutOfBoundsException("$index of $size")
            // The first album whose tracks end after index.
            var low = first
            var high = last - 1
            while (low < high) {
                val middle = (low + high) ushr 1
                if (artistTrackEnds[middle] > index) high = middle else low = middle + 1
            }
            val start = if (low == first) 0 else artistTrackEnds[low - 1]
            return track(albumStart[artistAlbums[low]] + index - start)
        }
    }

    private inner class ArtistList(private val ordinals: IntArray?) : AbstractList<Artist>(), RandomAccess {
        override val size: Int
            get() = ordinals?.size ?: artistName.size

        override fun get(index: Int): Artist {
            if (index < 0 || index >= size) throw IndexOutOfBoundsException("$index of $size")
            return Artist(this@LibraryIndex, ordinals?.get(index) ?: index)
        }
    }

    // ---- search --------------------------------------------------------------------------------------------------

    /** Tracks whose title, artist, album or album artist contain every word of [query] ([LibraryFormat.searchKey]), by title. */
    fun searchTracks(query: String): List<Track> {
        val matches = Matches.of(query, this) ?: return emptyList()
        val found = IntArray(byTitle.size)
        var count = 0
        for (ordinal in byTitle) {
            val album = trackAlbum[ordinal]
            if (matches.all(trackTitle[ordinal], trackArtist[ordinal], albumTitle[album], albumArtist[album])) found[count++] = ordinal
        }
        return tracksAt(found.copyOf(count))
    }

    /** Albums whose title or artist contain every word of [query], by title. */
    fun searchAlbums(query: String): List<Album> {
        val matches = Matches.of(query, this) ?: return emptyList()
        return AlbumList((albumTitle.indices).filter { matches.all(albumTitle[it], albumArtist[it]) }.toIntArray())
    }

    /** Artists whose name contains every word of [query], by name. */
    fun searchArtists(query: String): List<Artist> {
        val matches = Matches.of(query, this) ?: return emptyList()
        return ArtistList((artistName.indices).filter { matches.all(artistName[it]) }.toIntArray())
    }

    fun searchPlaylists(query: String, playlists: List<Playlist>): List<Playlist> {
        val words = words(query)
        if (words.isEmpty()) return emptyList()
        return playlists.filter { playlist -> LibraryFormat.searchKey(playlist.name).let { key -> words.all { key.contains(it) } } }
    }

    /** Which words of a query each text contains: one bit per word, worked out once per text. */
    private class Matches(private val index: LibraryIndex, private val words: List<ByteArray>) {
        private val all = if (words.size == Long.SIZE_BITS) -1L else (1L shl words.size) - 1
        private val masks = LongArray(index.textEnds.size)
        private val known = BooleanArray(index.textEnds.size)

        private fun mask(id: Int): Long {
            if (!known[id]) {
                val start = if (id == 0) 0 else index.keyEnds[id - 1]
                var mask = 0L
                for ((bit, word) in words.withIndex()) {
                    if (Utf8.contains(index.keys, start, index.keyEnds[id], word)) mask = mask or (1L shl bit)
                }
                masks[id] = mask
                known[id] = true
            }
            return masks[id]
        }

        fun all(id: Int): Boolean = mask(id) == all

        fun all(first: Int, second: Int): Boolean = mask(first) or mask(second) == all

        fun all(first: Int, second: Int, third: Int, fourth: Int): Boolean =
            mask(first) == all || (mask(first) or mask(second) or mask(third) or mask(fourth)) == all

        companion object {
            /** Null when [query] has no words. Words past the 64th are left out (a search box holds a few). */
            fun of(query: String, index: LibraryIndex): Matches? {
                val words = words(query).take(Long.SIZE_BITS).map { it.toByteArray(Charsets.UTF_8) }
                return if (words.isEmpty()) null else Matches(index, words)
            }
        }
    }

    companion object {
        /** Albums and artists in a row on Home. */
        const val HOME_ROW = 20

        internal const val NONE = -1
        internal const val HAS_TRACK_NUMBER = 1
        internal const val HAS_DISC_NUMBER = 2

        val EMPTY: LibraryIndex = LibraryIndexBuilder().build()

        /** An index of [tracks] (a path that comes twice keeps its first track). */
        fun of(tracks: Iterable<Track>): LibraryIndex = LibraryIndexBuilder().apply { tracks.forEach(::add) }.build()

        private fun words(query: String): List<String> = LibraryFormat.searchKey(query).split(Regex("\\s+")).filter { it.isNotEmpty() }
    }
}

/** An album of a [LibraryIndex]: equal to the same album of the same index. Its texts are read when made, its tracks when asked for. */
class Album internal constructor(private val index: LibraryIndex, private val ordinal: Int) {
    val title: String = index.string(index.albumTitle[ordinal])
    val artist: String = index.string(index.albumArtist[ordinal])

    /** `<artist> NUL <title>`: what identifies the album ([Track.albumKey]). */
    val key: String
        get() = Track.albumKey(artist, title)

    val tracks: List<Track>
        get() = index.albumTracks(ordinal)

    val trackCount: Int
        get() = index.albumTrackCount(ordinal)

    /** Newest file modification time, for "recently added". */
    val addedMs: Long
        get() = index.albumAdded[ordinal]

    /** The tracks' total duration. */
    val durationMs: Long
        get() = index.albumDuration[ordinal]

    override fun equals(other: Any?): Boolean = other is Album && other.index === index && other.ordinal == ordinal

    override fun hashCode(): Int = ordinal

    override fun toString(): String = "Album($artist / $title)"
}

/** An album artist of a [LibraryIndex]: equal to the same artist of the same index. */
class Artist internal constructor(private val index: LibraryIndex, private val ordinal: Int) {
    val name: String = index.string(index.artistName[ordinal])

    /** Albums by title. */
    val albums: List<Album>
        get() = index.artistAlbumList(ordinal)

    /** Tracks, album after album. */
    val tracks: List<Track>
        get() = index.artistTracks(ordinal)

    override fun equals(other: Any?): Boolean = other is Artist && other.index === index && other.ordinal == ordinal

    override fun hashCode(): Int = ordinal

    override fun toString(): String = "Artist($name)"
}
