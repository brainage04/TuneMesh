package io.github.brainage04.tunemesh.library

/** Albums, artists and search keys derived from a scan. Immutable; rebuilt after every scan. */
class LibraryIndex(val tracks: List<Track>) {
    val byPath: Map<String, Track> = tracks.associateBy { it.path }

    val albums: List<Album> = tracks.groupBy { it.albumKey }.map { (key, albumTracks) ->
        val sorted = albumTracks.sortedWith(trackOrder)
        Album(
            key = key,
            title = sorted.first().album,
            artist = sorted.first().albumArtist,
            tracks = sorted,
            addedMs = sorted.maxOf { it.modifiedMs },
        )
    }.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.title })

    val albumsByKey: Map<String, Album> = albums.associateBy { it.key }

    val artists: List<Artist> = albums.groupBy { it.artist }.map { (name, artistAlbums) ->
        Artist(name = name, albums = artistAlbums, tracks = artistAlbums.flatMap { it.tracks })
    }.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name })

    val artistsByName: Map<String, Artist> = artists.associateBy { it.name }

    val recentlyAdded: List<Album> get() = albums.sortedByDescending { it.addedMs }

    private val trackKeys: List<Pair<Track, String>> = tracks.map {
        it to LibraryFormat.searchKey("${it.title}\n${it.artist}\n${it.album}\n${it.albumArtist}")
    }

    fun searchTracks(query: String): List<Track> {
        val words = words(query)
        if (words.isEmpty()) return emptyList()
        return trackKeys.filter { (_, key) -> words.all { key.contains(it) } }
            .map { it.first }
            .sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.title })
    }

    fun searchAlbums(query: String): List<Album> = matching(query, albums) { "${it.title}\n${it.artist}" }

    fun searchArtists(query: String): List<Artist> = matching(query, artists) { it.name }

    fun searchPlaylists(query: String, playlists: List<Playlist>): List<Playlist> = matching(query, playlists) { it.name }

    private fun <T> matching(query: String, items: List<T>, text: (T) -> String): List<T> {
        val words = words(query)
        if (words.isEmpty()) return emptyList()
        return items.filter { item -> LibraryFormat.searchKey(text(item)).let { key -> words.all { key.contains(it) } } }
    }

    private fun words(query: String): List<String> =
        LibraryFormat.searchKey(query).split(Regex("\\s+")).filter { it.isNotEmpty() }

    companion object {
        val EMPTY = LibraryIndex(emptyList())

        val trackOrder: Comparator<Track> = compareBy<Track>({ it.discNumber ?: 0 }, { it.trackNumber ?: Int.MAX_VALUE })
            .thenBy(String.CASE_INSENSITIVE_ORDER) { it.path }
    }
}
