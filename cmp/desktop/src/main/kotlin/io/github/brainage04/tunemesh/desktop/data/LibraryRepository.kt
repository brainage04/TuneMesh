package io.github.brainage04.tunemesh.desktop.data

import io.github.brainage04.tunemesh.library.AtomicFiles
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.LibraryIndex
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.PlaylistStore
import io.github.brainage04.tunemesh.library.RawTags
import io.github.brainage04.tunemesh.library.Track
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes

data class LibraryState(
    val root: File,
    val loaded: Boolean = false,
    val scanning: Boolean = false,
    val index: LibraryIndex = LibraryIndex.EMPTY,
    val playlists: List<Playlist> = emptyList(),
    val liked: Set<String> = emptySet(),
) {
    /** Liked Songs always exists for the UI, even before its file is first written. */
    fun playlist(name: String): Playlist? = playlists.firstOrNull { it.name == name }
        ?: if (name == LibraryFormat.LIKED_SONGS) Playlist(name, emptyList(), 0) else null
    val likedSongs: Playlist? get() = playlists.firstOrNull { it.isLikedSongs }
}

@Serializable
private data class CachedTrack(val size: Long, val modifiedMs: Long, val tags: RawTags)

@Serializable
private data class TagCacheFile(val root: String, val entries: Map<String, CachedTrack>)

/**
 * Library state for the desktop app: cached tags first, then a rescan that reads tags of new or changed
 * files only (the Android LibraryScanner's approach). Playlists go through the shared [PlaylistStore].
 */
class LibraryRepository(root: File, private val cacheFile: File) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()
    private val json = Json { ignoreUnknownKeys = true }
    private val _state = MutableStateFlow(LibraryState(root))
    val state: StateFlow<LibraryState> = _state
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    val root: File get() = _state.value.root
    private val store get() = PlaylistStore(root) { _state.value.index.byPath[it] }

    fun start() {
        scope.launch {
            mutex.withLock {
                val cache = loadCache()
                if (cache.isNotEmpty()) {
                    val tracks = cache.map { (path, entry) -> Track.fromTags(path, entry.tags, entry.size, entry.modifiedMs) }
                    publish(LibraryIndex(tracks), loaded = true)
                }
                _state.update { it.copy(scanning = true) }
                val tracks = scan(cache)
                publish(LibraryIndex(tracks), loaded = true)
                _state.update { it.copy(scanning = false) }
            }
        }
    }

    private fun publish(index: LibraryIndex, loaded: Boolean) {
        val playlists = PlaylistStore(root) { index.byPath[it] }.list()
        _state.update { state ->
            state.copy(index = index, loaded = loaded, playlists = playlists, liked = likedPaths(playlists))
        }
    }

    private fun likedPaths(playlists: List<Playlist>): Set<String> =
        playlists.firstOrNull { it.isLikedSongs }?.entries?.mapNotNullTo(HashSet()) { it.path } ?: emptySet()

    private fun loadCache(): MutableMap<String, CachedTrack> = try {
        val file = json.decodeFromString<TagCacheFile>(cacheFile.readText())
        if (file.root == root.absolutePath) file.entries.toMutableMap() else HashMap()
    } catch (_: Exception) {
        HashMap()
    }

    private fun scan(cache: MutableMap<String, CachedTrack>): List<Track> {
        val rootPath = root.toPath()
        val found = HashMap<String, Pair<Long, Long>>()
        if (root.isDirectory) {
            Files.walkFileTree(rootPath, object : SimpleFileVisitor<Path>() {
                override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
                    val relative = rootPath.relativize(dir).toString()
                    return if (relative.isNotEmpty() && LibraryFormat.isExcludedDirectory(relative)) FileVisitResult.SKIP_SUBTREE else FileVisitResult.CONTINUE
                }

                override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                    if (attrs.isRegularFile) {
                        val relative = rootPath.relativize(file).toString()
                        if (LibraryFormat.isAudioPath(relative)) found[relative] = attrs.size() to attrs.lastModifiedTime().toMillis()
                    }
                    return FileVisitResult.CONTINUE
                }

                override fun visitFileFailed(file: Path, exc: IOException): FileVisitResult = FileVisitResult.CONTINUE
            })
        }
        var changed = cache.keys.retainAll(found.keys)
        val tracks = ArrayList<Track>(found.size)
        for ((path, stat) in found) {
            val (size, modified) = stat
            val cached = cache[path]
            val entry = if (cached != null && cached.size == size && cached.modifiedMs == modified) {
                cached
            } else {
                changed = true
                CachedTrack(size, modified, TagReader.read(File(root, path))).also { cache[path] = it }
            }
            tracks += Track.fromTags(path, entry.tags, size, modified)
        }
        if (changed) {
            try {
                cacheFile.parentFile?.mkdirs()
                AtomicFiles.write(cacheFile, json.encodeToString(TagCacheFile.serializer(), TagCacheFile(root.absolutePath, cache)))
            } catch (_: IOException) {
                // The cache only speeds up the next start.
            }
        }
        return tracks
    }

    private fun refreshPlaylists() {
        val playlists = store.list()
        _state.update { it.copy(playlists = playlists, liked = likedPaths(playlists)) }
    }

    private fun edit(failure: String, action: PlaylistStore.() -> Unit) {
        scope.launch {
            try {
                store.action()
            } catch (e: Exception) {
                _messages.tryEmit("$failure: ${e.message}")
            }
            refreshPlaylists()
        }
    }

    fun setLiked(track: Track, liked: Boolean) = edit("Could not update Liked Songs") { setLiked(track, liked) }

    fun createPlaylist(name: String, tracks: List<Track>) = edit("Could not create playlist") {
        val created = create(name)
        if (tracks.isNotEmpty()) add(created, tracks)
    }

    fun addToPlaylist(name: String, tracks: List<Track>) = edit("Could not add to playlist") { add(name, tracks) }

    fun removeFromPlaylist(name: String, index: Int, path: String?) = edit("Could not remove from playlist") { removeAt(name, index, path) }

    fun movePlaylistEntry(name: String, from: Int, to: Int) = edit("Could not move song") { move(name, from, to) }

    fun renamePlaylist(from: String, to: String) = edit("Could not rename playlist") { rename(from, to) }

    fun deletePlaylist(name: String) = edit("Could not delete playlist") { delete(name) }
}
