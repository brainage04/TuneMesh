package io.github.brainage04.tunemesh.library

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

/** The audio files a scan found: library-relative path, size and modification time of each, without an object per file. */
class FoundFiles {
    private val paths = ArrayList<String>()
    private var sizes = LongArray(INITIAL)
    private var modified = LongArray(INITIAL)

    val size: Int
        get() = paths.size

    fun add(path: String, size: Long, modifiedMs: Long) {
        if (paths.size == sizes.size) {
            sizes = sizes.copyOf(sizes.size * 2)
            modified = modified.copyOf(modified.size * 2)
        }
        sizes[paths.size] = size
        modified[paths.size] = modifiedMs
        paths += path
    }

    fun path(index: Int): String = paths[index]

    fun size(index: Int): Long = sizes[index]

    fun modifiedMs(index: Int): Long = modified[index]

    private companion object {
        const val INITIAL = 1024
    }
}

object LibraryScan {
    /**
     * The index of the [found] files: a file whose path, size and modification time are those of a track of [previous] keeps that track
     * (copied as bytes, no tags read); [readTags] reads the others. Returns [previous] itself when every file is unchanged and none is
     * gone, so the screens showing it do not recompose.
     */
    fun index(previous: LibraryIndex, found: FoundFiles, readTags: (path: String) -> RawTags): LibraryIndex {
        val known =
            IntArray(found.size) { i ->
                previous.indexOf(found.path(i)).takeIf { it >= 0 && previous.isUnchanged(it, found.size(i), found.modifiedMs(i)) } ?: -1
            }
        if (found.size == previous.size && known.all { it >= 0 }) return previous
        val builder = LibraryIndexBuilder()
        for (i in 0 until found.size) {
            if (known[i] >= 0) {
                builder.copy(previous, known[i])
            } else {
                val path = found.path(i)
                builder.add(Track.fromTags(path, readTags(path), found.size(i), found.modifiedMs(i)))
            }
        }
        return builder.build()
    }

    /**
     * The tag cache earlier versions kept as JSON (`tag-cache.json`), as an index, or null when it is not one or belongs to another [root].
     * Read once, when there is no saved index yet, so an update does not read every file's tags again.
     */
    fun fromJsonTagCache(text: String, root: String): LibraryIndex? {
        val cache =
            try {
                Json { ignoreUnknownKeys = true }.decodeFromString<JsonTagCache>(text).takeIf { it.root == root }
            } catch (_: IllegalArgumentException) {
                null
            }
        return cache?.let {
            val builder = LibraryIndexBuilder()
            // Filtered like a scan, so an entry an older version kept under laxer rules is read again if it is still there.
            for ((path, entry) in it.entries) {
                if (LibraryFormat.isAudioPath(path)) builder.add(Track.fromTags(path, entry.tags, entry.size, entry.modifiedMs))
            }
            builder.build()
        }
    }

    @Serializable internal data class JsonTagCacheEntry(val size: Long, val modifiedMs: Long, val tags: RawTags)

    @Serializable internal data class JsonTagCache(val root: String, val entries: Map<String, JsonTagCacheEntry>)
}
