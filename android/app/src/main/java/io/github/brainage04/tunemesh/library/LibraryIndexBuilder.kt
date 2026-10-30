package io.github.brainage04.tunemesh.library

/**
 * Builds a [LibraryIndex]: tracks are added one by one, from tags ([add]) or copied from an earlier index without decoding their texts
 * ([copy]); [build] groups them into albums and artists and sorts everything once. A path added twice keeps its first track.
 */
internal class LibraryIndexBuilder {
    private val texts = Texts()
    private var count = 0
    private var paths = IntArray(INITIAL)
    private var titles = IntArray(INITIAL)
    private var artists = IntArray(INITIAL)
    private var albums = IntArray(INITIAL)
    private var albumArtists = IntArray(INITIAL)
    private var trackNumbers = IntArray(INITIAL)
    private var discNumbers = IntArray(INITIAL)
    private var flags = ByteArray(INITIAL)
    private var durations = LongArray(INITIAL)
    private var sizes = LongArray(INITIAL)
    private var modified = LongArray(INITIAL)

    val size: Int
        get() = count

    fun add(track: Track) =
        add(
            track.path,
            track.title,
            track.artist,
            track.album,
            track.albumArtist,
            track.trackNumber,
            track.discNumber,
            track.durationMs,
            track.size,
            track.modifiedMs,
        )

    @Suppress("LongParameterList") // the fields of a track
    fun add(
        path: String,
        title: String,
        artist: String,
        album: String,
        albumArtist: String,
        trackNumber: Int?,
        discNumber: Int?,
        durationMs: Long,
        size: Long,
        modifiedMs: Long,
    ) {
        val pathId = texts.intern(path, key = false)
        if (texts.isPath(pathId)) return
        val flag =
            (if (trackNumber != null) LibraryIndex.HAS_TRACK_NUMBER else 0) or (if (discNumber != null) LibraryIndex.HAS_DISC_NUMBER else 0)
        push(
            pathId,
            texts.intern(title, key = true),
            texts.intern(artist, key = true),
            texts.intern(album, key = true),
            texts.intern(albumArtist, key = true),
            trackNumber ?: 0,
            discNumber ?: 0,
            flag,
            durationMs,
            size,
            modifiedMs,
        )
    }

    /** Adds track [ordinal] of [from], copying its texts and their search keys as bytes. */
    fun copy(from: LibraryIndex, ordinal: Int) {
        val pathId = texts.copy(from, from.trackPath[ordinal], key = false)
        if (texts.isPath(pathId)) return
        val album = from.trackAlbum[ordinal]
        push(
            pathId,
            texts.copy(from, from.trackTitle[ordinal], key = true),
            texts.copy(from, from.trackArtist[ordinal], key = true),
            texts.copy(from, from.albumTitle[album], key = true),
            texts.copy(from, from.albumArtist[album], key = true),
            from.trackNumbers[ordinal],
            from.discNumbers[ordinal],
            from.trackFlags[ordinal].toInt(),
            from.durations[ordinal],
            from.sizes[ordinal],
            from.modified[ordinal],
        )
    }

    @Suppress("LongParameterList") // the fields of a track
    private fun push(
        path: Int,
        title: Int,
        artist: Int,
        album: Int,
        albumArtist: Int,
        trackNumber: Int,
        discNumber: Int,
        flag: Int,
        duration: Long,
        size: Long,
        modifiedMs: Long,
    ) {
        if (count == paths.size) grow()
        texts.markPath(path)
        paths[count] = path
        titles[count] = title
        artists[count] = artist
        albums[count] = album
        albumArtists[count] = albumArtist
        trackNumbers[count] = trackNumber
        discNumbers[count] = discNumber
        flags[count] = flag.toByte()
        durations[count] = duration
        sizes[count] = size
        modified[count] = modifiedMs
        count++
    }

    private fun grow() {
        val capacity = paths.size * 2
        paths = paths.copyOf(capacity)
        titles = titles.copyOf(capacity)
        artists = artists.copyOf(capacity)
        albums = albums.copyOf(capacity)
        albumArtists = albumArtists.copyOf(capacity)
        trackNumbers = trackNumbers.copyOf(capacity)
        discNumbers = discNumbers.copyOf(capacity)
        flags = flags.copyOf(capacity)
        durations = durations.copyOf(capacity)
        sizes = sizes.copyOf(capacity)
        modified = modified.copyOf(capacity)
    }

    @Suppress("CyclomaticComplexMethod", "LongMethod") // one pass per derived table, in order
    fun build(): LibraryIndex {
        val text = texts.bytes()
        val textEnds = texts.ends()
        val (keys, keyEnds) = texts.keys()
        fun start(id: Int) = if (id == 0) 0 else textEnds[id - 1]
        val ignoringCase = Utf8.CaseInsensitive(text)
        fun compareText(a: Int, b: Int): Int = ignoringCase.compare(start(a), textEnds[a], start(b), textEnds[b])
        fun compareBytes(a: Int, b: Int): Int {
            val aStart = start(a)
            val bStart = start(b)
            val common = minOf(textEnds[a] - aStart, textEnds[b] - bStart)
            for (i in 0 until common) {
                val order = (text[aStart + i].toInt() and BYTE) - (text[bStart + i].toInt() and BYTE)
                if (order != 0) return order
            }
            return (textEnds[a] - aStart) - (textEnds[b] - bStart)
        }

        // Albums: one per (album artist, title), by title ignoring case.
        val albumOf = IntArray(count)
        val albumIds = LongIntTable()
        var albumTitles = IntArray(INITIAL)
        var albumArtistIds = IntArray(INITIAL)
        var albumCount = 0
        for (t in 0 until count) {
            val key = (albumArtists[t].toLong() shl Int.SIZE_BITS) or (albums[t].toLong() and INT_MASK)
            var id = albumIds.get(key)
            if (id < 0) {
                id = albumCount++
                albumIds.put(key, id)
                if (id == albumTitles.size) {
                    albumTitles = albumTitles.copyOf(id * 2)
                    albumArtistIds = albumArtistIds.copyOf(id * 2)
                }
                albumTitles[id] = albums[t]
                albumArtistIds[id] = albumArtists[t]
            }
            albumOf[t] = id
        }
        val albumOrder = IntArray(albumCount) { it }
        IntSort.sort(albumOrder) { a, b ->
            compareText(albumTitles[a], albumTitles[b]).takeIf { it != 0 }
                ?: compareBytes(albumArtistIds[a], albumArtistIds[b]).takeIf { it != 0 }
                ?: compareBytes(albumTitles[a], albumTitles[b])
        }
        val albumRank = IntArray(albumCount)
        for ((rank, id) in albumOrder.withIndex()) albumRank[id] = rank

        // Tracks: album by album, each album in track order (disc, number, path ignoring case).
        val albumStart = IntArray(albumCount + 1)
        for (t in 0 until count) albumStart[albumRank[albumOf[t]] + 1]++
        for (a in 0 until albumCount) albumStart[a + 1] += albumStart[a]
        val order = IntArray(count)
        val fill = albumStart.copyOf(albumCount)
        for (t in 0 until count) order[fill[albumRank[albumOf[t]]]++] = t
        val trackOrder = IntSort.Order { a, b ->
            val discA = if (flags[a].toInt() and LibraryIndex.HAS_DISC_NUMBER != 0) discNumbers[a] else 0
            val discB = if (flags[b].toInt() and LibraryIndex.HAS_DISC_NUMBER != 0) discNumbers[b] else 0
            val numberA = if (flags[a].toInt() and LibraryIndex.HAS_TRACK_NUMBER != 0) trackNumbers[a] else Int.MAX_VALUE
            val numberB = if (flags[b].toInt() and LibraryIndex.HAS_TRACK_NUMBER != 0) trackNumbers[b] else Int.MAX_VALUE
            discA.compareTo(discB).takeIf { it != 0 } ?: numberA.compareTo(numberB).takeIf { it != 0 } ?: compareText(paths[a], paths[b])
        }
        for (a in 0 until albumCount) {
            val range = order.copyOfRange(albumStart[a], albumStart[a + 1])
            IntSort.sort(range, trackOrder)
            range.copyInto(order, albumStart[a])
        }

        val trackAlbum = IntArray(count) { albumRank[albumOf[order[it]]] }
        val albumAdded = LongArray(albumCount) { Long.MIN_VALUE }
        val albumDuration = LongArray(albumCount)
        for (position in 0 until count) {
            val t = order[position]
            val a = trackAlbum[position]
            albumAdded[a] = maxOf(albumAdded[a], modified[t])
            albumDuration[a] += durations[t]
        }
        val albumTitle = IntArray(albumCount) { albumTitles[albumOrder[it]] }
        val albumArtist = IntArray(albumCount) { albumArtistIds[albumOrder[it]] }
        val recentAlbums = IntArray(albumCount) { it }
        IntSort.sort(recentAlbums) { a, b -> albumAdded[b].compareTo(albumAdded[a]) }

        // Artists: the album artists, by name ignoring case; each with its albums in album order.
        val artistIds = LongIntTable()
        val artistOfAlbum = IntArray(albumCount)
        var artistNames = IntArray(INITIAL)
        var artistCount = 0
        for (a in 0 until albumCount) {
            var id = artistIds.get(albumArtist[a].toLong())
            if (id < 0) {
                id = artistCount++
                artistIds.put(albumArtist[a].toLong(), id)
                if (id == artistNames.size) artistNames = artistNames.copyOf(id * 2)
                artistNames[id] = albumArtist[a]
            }
            artistOfAlbum[a] = id
        }
        val artistOrder = IntArray(artistCount) { it }
        IntSort.sort(artistOrder) { a, b ->
            compareText(artistNames[a], artistNames[b]).takeIf { it != 0 } ?: compareBytes(artistNames[a], artistNames[b])
        }
        val artistRank = IntArray(artistCount)
        for ((rank, id) in artistOrder.withIndex()) artistRank[id] = rank
        val artistAlbumStart = IntArray(artistCount + 1)
        for (a in 0 until albumCount) artistAlbumStart[artistRank[artistOfAlbum[a]] + 1]++
        for (r in 0 until artistCount) artistAlbumStart[r + 1] += artistAlbumStart[r]
        val artistAlbums = IntArray(albumCount)
        val next = artistAlbumStart.copyOf(artistCount)
        for (a in 0 until albumCount) artistAlbums[next[artistRank[artistOfAlbum[a]]]++] = a
        val artistTrackEnds = IntArray(albumCount)
        for (r in 0 until artistCount) {
            var total = 0
            for (i in artistAlbumStart[r] until artistAlbumStart[r + 1]) {
                val a = artistAlbums[i]
                total += albumStart[a + 1] - albumStart[a]
                artistTrackEnds[i] = total
            }
        }
        val artistName = IntArray(artistCount) { artistNames[artistOrder[it]] }
        val artistNewest =
            LongArray(artistCount) { r -> (artistAlbumStart[r] until artistAlbumStart[r + 1]).maxOf { albumAdded[artistAlbums[it]] } }
        val recentArtists = IntArray(artistCount) { it }
        IntSort.sort(recentArtists) { a, b -> artistNewest[b].compareTo(artistNewest[a]) }

        val trackPath = IntArray(count) { paths[order[it]] }
        val trackTitle = IntArray(count) { titles[order[it]] }
        val byTitle = IntArray(count) { it }
        IntSort.sort(byTitle) { a, b -> compareText(trackTitle[a], trackTitle[b]) }

        fun slots(size: Int, hash: (Int) -> Int): IntArray {
            var capacity = 2
            while (capacity < size * 2) capacity *= 2
            val slots = IntArray(capacity)
            for (i in 0 until size) {
                var slot = hash(i) and (capacity - 1)
                while (slots[slot] != 0) slot = (slot + 1) and (capacity - 1)
                slots[slot] = i + 1
            }
            return slots
        }

        return LibraryIndex(
            text = text,
            textEnds = textEnds,
            keys = keys,
            keyEnds = keyEnds,
            trackPath = trackPath,
            trackTitle = trackTitle,
            trackArtist = IntArray(count) { artists[order[it]] },
            trackAlbum = trackAlbum,
            trackNumbers = IntArray(count) { trackNumbers[order[it]] },
            discNumbers = IntArray(count) { discNumbers[order[it]] },
            trackFlags = ByteArray(count) { flags[order[it]] },
            durations = LongArray(count) { durations[order[it]] },
            sizes = LongArray(count) { sizes[order[it]] },
            modified = LongArray(count) { modified[order[it]] },
            pathSlots = slots(count) { Utf8.hash(text, start(trackPath[it]), textEnds[trackPath[it]]) },
            byTitle = byTitle,
            albumTitle = albumTitle,
            albumArtist = albumArtist,
            albumStart = albumStart,
            albumAdded = albumAdded,
            albumDuration = albumDuration,
            albumSlots =
                slots(albumCount) {
                    val artist = albumArtist[it]
                    val title = albumTitle[it]
                    // The hash of "<artist> NUL <title>", as LibraryIndex.album hashes a key.
                    val withSeparator = HASH_FACTOR * Utf8.hashOf(text, start(artist), textEnds[artist])
                    Utf8.spread(Utf8.hashOf(text, start(title), textEnds[title], withSeparator))
                },
            recentAlbums = recentAlbums,
            artistName = artistName,
            artistAlbumStart = artistAlbumStart,
            artistAlbums = artistAlbums,
            artistTrackEnds = artistTrackEnds,
            artistSlots = slots(artistCount) { Utf8.hash(text, start(artistName[it]), textEnds[artistName[it]]) },
            recentArtistOrder = recentArtists.copyOf(minOf(artistCount, LibraryIndex.HOME_ROW)),
        )
    }

    /**
     * The texts of the index being built, each stored once (as UTF-8, found again by an open-addressing table of its bytes), with the
     * search key of those that are not only paths.
     */
    @Suppress("TooManyFunctions") // interning, keys and the final arrays of one table
    private class Texts {
        private var bytes = ByteArray(INITIAL_BYTES)
        private var length = 0
        private var ends = IntArray(INITIAL)
        private var count = 0
        private var slots = IntArray(INITIAL * 2)
        private var keyBytes = ByteArray(INITIAL_BYTES)
        private var keyLength = 0
        /** Per text, where its key starts in [keyBytes] and ends; -1: not worked out yet. */
        private var keyFrom = IntArray(INITIAL)
        private var keyTo = IntArray(INITIAL)
        private var needsKey = BooleanArray(INITIAL)
        private var isPath = BooleanArray(INITIAL)

        private fun start(id: Int) = if (id == 0) 0 else ends[id - 1]

        fun isPath(id: Int): Boolean = isPath[id]

        fun markPath(id: Int) {
            isPath[id] = true
        }

        fun intern(value: String, key: Boolean): Int {
            val encoded = value.toByteArray(Charsets.UTF_8)
            return intern(encoded, 0, encoded.size, key)
        }

        /** Text [id] of [from], with its search key when it has one (an empty key is worked out again, as a path has none). */
        fun copy(from: LibraryIndex, id: Int, key: Boolean): Int {
            val start = if (id == 0) 0 else from.textEnds[id - 1]
            val copied = intern(from.text, start, from.textEnds[id], key)
            val keyStart = if (id == 0) 0 else from.keyEnds[id - 1]
            if (key && keyFrom[copied] < 0 && from.keyEnds[id] > keyStart) appendKey(copied, from.keys, keyStart, from.keyEnds[id])
            return copied
        }

        private fun intern(source: ByteArray, from: Int, to: Int, key: Boolean): Int {
            val hash = Utf8.hash(source, from, to)
            var slot = hash and (slots.size - 1)
            while (true) {
                val id = slots[slot] - 1
                if (id < 0) break
                if (Utf8.equal(bytes, start(id), ends[id], source, from, to)) {
                    if (key) needsKey[id] = true
                    return id
                }
                slot = (slot + 1) and (slots.size - 1)
            }
            val id = count++
            if (id == ends.size) growTexts()
            if (length + (to - from) > bytes.size) bytes = bytes.copyOf(maxOf(length + (to - from), bytes.size * 2))
            System.arraycopy(source, from, bytes, length, to - from)
            length += to - from
            ends[id] = length
            keyFrom[id] = -1
            keyTo[id] = -1
            needsKey[id] = key
            slots[slot] = id + 1
            if (count * 2 > slots.size) rehash()
            return id
        }

        private fun growTexts() {
            val capacity = ends.size * 2
            ends = ends.copyOf(capacity)
            keyFrom = keyFrom.copyOf(capacity)
            keyTo = keyTo.copyOf(capacity)
            needsKey = needsKey.copyOf(capacity)
            isPath = isPath.copyOf(capacity)
        }

        private fun rehash() {
            slots = IntArray(slots.size * 2)
            for (id in 0 until count) {
                var slot = Utf8.hash(bytes, start(id), ends[id]) and (slots.size - 1)
                while (slots[slot] != 0) slot = (slot + 1) and (slots.size - 1)
                slots[slot] = id + 1
            }
        }

        private fun appendKey(id: Int, source: ByteArray, from: Int, to: Int) {
            if (keyLength + (to - from) > keyBytes.size) keyBytes = keyBytes.copyOf(maxOf(keyLength + (to - from), keyBytes.size * 2))
            System.arraycopy(source, from, keyBytes, keyLength, to - from)
            keyFrom[id] = keyLength
            keyLength += to - from
            keyTo[id] = keyLength
        }

        fun bytes(): ByteArray = bytes.copyOf(length)

        fun ends(): IntArray = ends.copyOf(count)

        /** The search keys in text order (worked out for the texts that need one and have none yet), and their ends. */
        fun keys(): Pair<ByteArray, IntArray> {
            for (id in 0 until count) {
                if (needsKey[id] && keyFrom[id] < 0) {
                    val key =
                        LibraryFormat.searchKey(String(bytes, start(id), ends[id] - start(id), Charsets.UTF_8)).toByteArray(Charsets.UTF_8)
                    appendKey(id, key, 0, key.size)
                }
            }
            val keys = ByteArray((0 until count).sumOf { if (needsKey[it]) keyTo[it] - keyFrom[it] else 0 })
            val keyEnds = IntArray(count)
            var end = 0
            for (id in 0 until count) {
                if (needsKey[id]) {
                    System.arraycopy(keyBytes, keyFrom[id], keys, end, keyTo[id] - keyFrom[id])
                    end += keyTo[id] - keyFrom[id]
                }
                keyEnds[id] = end
            }
            return keys to keyEnds
        }
    }

    /** An open-addressing map from long keys to non-negative ints, for grouping without boxing. */
    private class LongIntTable {
        private var keys = LongArray(INITIAL * 2)
        private var values = IntArray(INITIAL * 2) { -1 }
        private var size = 0

        private fun slot(key: Long, capacity: Int): Int = Utf8.spread((key xor (key ushr Int.SIZE_BITS)).toInt()) and (capacity - 1)

        /** The value for [key], or -1. */
        fun get(key: Long): Int {
            var slot = slot(key, keys.size)
            while (values[slot] >= 0) {
                if (keys[slot] == key) return values[slot]
                slot = (slot + 1) and (keys.size - 1)
            }
            return -1
        }

        fun put(key: Long, value: Int) {
            if ((size + 1) * 2 > keys.size) grow()
            var slot = slot(key, keys.size)
            while (values[slot] >= 0 && keys[slot] != key) slot = (slot + 1) and (keys.size - 1)
            if (values[slot] < 0) size++
            keys[slot] = key
            values[slot] = value
        }

        private fun grow() {
            val oldKeys = keys
            val oldValues = values
            keys = LongArray(oldKeys.size * 2)
            values = IntArray(oldKeys.size * 2) { -1 }
            size = 0
            for (i in oldKeys.indices) if (oldValues[i] >= 0) put(oldKeys[i], oldValues[i])
        }
    }

    private companion object {
        const val INITIAL = 16
        const val INITIAL_BYTES = 1024
        const val BYTE = 0xFF
        const val INT_MASK = 0xFFFFFFFFL
        const val HASH_FACTOR = 31
    }
}
