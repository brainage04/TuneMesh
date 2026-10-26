package io.github.brainage04.tunemesh.library

import java.io.File

/**
 * Playlist summaries kept between refreshes, so an unchanged playlist (up to [Limits.MAX_PLAYLIST_BYTES]) is not read again every time the
 * library refreshes. A summary is reused while the file's modification time and length and the track lookup it was made with are the same.
 */
class PlaylistSummaries {
    private data class Key(val modifiedMs: Long, val length: Long, val lookup: Any)

    private val entries = HashMap<String, Pair<Key, Playlist?>>()

    @Synchronized
    fun get(file: File, lookup: Any, read: () -> Playlist?): Playlist? {
        val key = Key(file.lastModified(), file.length(), lookup)
        entries[file.name]?.let { (cached, playlist) -> if (cached == key) return playlist }
        val playlist = read()
        entries[file.name] = key to playlist
        return playlist
    }

    /** Forgets everything; when any playlist may have changed (a change within the clock's resolution keeps both). */
    @Synchronized fun clear() = entries.clear()

    /** Forgets the playlists this app just changed ([names]: file names), so only those are read again. */
    @Synchronized
    fun forget(names: Collection<String>) {
        entries.keys.removeAll(names.toSet())
    }

    /** Forgets every playlist not in [names] (file names). */
    @Synchronized
    fun retain(names: Collection<String>) {
        val keep = names.toHashSet()
        entries.keys.retainAll(keep)
    }
}
