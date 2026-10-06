package io.github.brainage04.tunemesh.data

import android.os.Environment
import android.os.FileObserver
import io.github.brainage04.tunemesh.library.DownloadItem
import io.github.brainage04.tunemesh.library.DownloadStore
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.LibraryIndex
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.PlaylistStore
import io.github.brainage04.tunemesh.library.Track
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

data class LibraryState(
    val root: File,
    val hasAccess: Boolean,
    val loaded: Boolean = false,
    val scanning: Boolean = false,
    val index: LibraryIndex = LibraryIndex.EMPTY,
    val playlists: List<Playlist> = emptyList(),
    val liked: Set<String> = emptySet(),
    val downloads: List<DownloadItem> = emptyList(),
    val error: String? = null,
) {
    /** Liked Songs always exists for the UI, even before its file is first written. */
    fun playlist(name: String): Playlist? = playlists.firstOrNull { it.name == name }
        ?: if (name == LibraryFormat.LIKED_SONGS) Playlist(name, emptyList(), 0) else null
    val likedSongs: Playlist? get() = playlists.firstOrNull { it.isLikedSongs }
}

class LibraryRepository(
    private val settings: AppSettings,
    private val scanner: LibraryScanner,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val scanMutex = Mutex()
    private val _state = MutableStateFlow(LibraryState(settings.libraryRoot, hasAccess()))
    val state: StateFlow<LibraryState> = _state
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages: SharedFlow<String> = _messages

    private var observers: List<FileObserver> = emptyList()
    private var observedDirectories: Set<File> = emptySet()
    private var foregroundJob: Job? = null
    @Volatile private var foreground = false
    private var pendingRefresh: Job? = null

    val root: File get() = _state.value.root
    private val playlistStore get() = PlaylistStore(root) { _state.value.index.byPath[it] }
    private val downloadStore get() = DownloadStore(root)

    fun hasAccess(): Boolean = Environment.isExternalStorageManager()

    /** Shows cached tags immediately, then rescans. */
    fun start() {
        scope.launch {
            val cached = scanner.cachedTracks(root)
            if (cached.isNotEmpty() && !_state.value.loaded) {
                _state.update { it.copy(index = LibraryIndex(cached), loaded = true) }
            }
            refresh()
        }
    }

    /** Called while an activity is visible: observes the tree and polls as a safety net. */
    fun setForeground(foreground: Boolean) {
        foregroundJob?.cancel()
        this.foreground = foreground
        if (!foreground) {
            stopObserving()
            return
        }
        foregroundJob = scope.launch {
            refresh()
            var ticks = 0
            while (isActive) {
                delay(DOWNLOAD_POLL_MS)
                ticks++
                if (ticks % (LIBRARY_POLL_MS / DOWNLOAD_POLL_MS) == 0L) refresh() else refreshDownloads()
            }
        }
    }

    fun refreshAsync() {
        scope.launch { refresh() }
    }

    suspend fun refresh() = scanMutex.withLock {
        val access = hasAccess()
        val root = settings.libraryRoot
        if (!access) {
            _state.update { it.copy(root = root, hasAccess = false) }
            return@withLock
        }
        _state.update { it.copy(root = root, hasAccess = true, scanning = true) }
        try {
            ensureRoot(root)
            val result = scanner.scan(root)
            val index = LibraryIndex(result.tracks)
            _state.update { it.copy(index = index, loaded = true, error = null) }
            reloadPlaylists()
            reloadDownloads()
            observe(result.directories)
        } catch (error: IOException) {
            _state.update { it.copy(loaded = true, error = error.message ?: "Cannot read $root") }
        } finally {
            _state.update { it.copy(scanning = false) }
        }
    }

    private fun ensureRoot(root: File) {
        if (!root.isDirectory && !root.mkdirs()) throw IOException("Cannot create $root")
        val stignore = File(root, ".stignore")
        if (!stignore.exists()) stignore.writeText(LibraryFormat.STIGNORE)
    }

    private fun reloadPlaylists() {
        val playlists = playlistStore.list()
        val liked = playlists.firstOrNull { it.isLikedSongs }?.entries?.mapNotNullTo(HashSet()) { it.path } ?: emptySet()
        _state.update { it.copy(playlists = playlists, liked = liked) }
    }

    private fun reloadDownloads() {
        val downloads = downloadStore.list()
        _state.update { it.copy(downloads = downloads) }
    }

    private suspend fun refreshDownloads() = withContext(Dispatchers.IO) {
        if (_state.value.hasAccess) reloadDownloads()
    }

    @Synchronized
    private fun observe(libraryDirectories: List<File>) {
        if (!foreground) return
        val directories = (libraryDirectories + listOf(
            File(root, LibraryFormat.PLAYLISTS_DIR),
            File(root, "${LibraryFormat.STATE_DIR}/requests"),
            File(root, "${LibraryFormat.STATE_DIR}/status"),
        )).filter { it.isDirectory }.toSet()
        if (directories == observedDirectories) return
        stopObserving()
        val mask = FileObserver.CREATE or FileObserver.DELETE or FileObserver.MOVED_FROM or
            FileObserver.MOVED_TO or FileObserver.CLOSE_WRITE
        observers = directories.map { directory ->
            object : FileObserver(directory, mask) {
                override fun onEvent(event: Int, path: String?) {
                    if (path == null || path.startsWith(".~tunemesh-") || path.startsWith(".syncthing.")) return
                    scheduleRefresh()
                }
            }.also { it.startWatching() }
        }
        observedDirectories = directories
    }

    @Synchronized
    private fun stopObserving() {
        observers.forEach { it.stopWatching() }
        observers = emptyList()
        observedDirectories = emptySet()
    }

    private fun scheduleRefresh() {
        pendingRefresh?.cancel()
        pendingRefresh = scope.launch {
            delay(800)
            refresh()
        }
    }

    fun setLibraryRoot(path: String) {
        val file = File(path.trim())
        settings.libraryRoot = file
        _state.update { LibraryState(file, hasAccess()) }
        stopObserving()
        start()
    }

    private fun mutate(action: () -> Unit) {
        scope.launch {
            try {
                action()
            } catch (error: Exception) {
                _messages.tryEmit(error.message ?: error.toString())
            }
            scanMutex.withLock { reloadPlaylists() }
        }
    }

    fun setLiked(track: Track, liked: Boolean) {
        _state.update { state -> state.copy(liked = if (liked) state.liked + track.path else state.liked - track.path) }
        mutate { playlistStore.setLiked(track, liked) }
    }

    fun createPlaylist(name: String, tracks: List<Track> = emptyList()) = mutate {
        val created = playlistStore.create(name)
        if (tracks.isNotEmpty()) playlistStore.add(created, tracks)
        _messages.tryEmit(if (tracks.isEmpty()) "Created \"$created\"" else "Added to \"$created\"")
    }

    fun renamePlaylist(from: String, to: String) = mutate { playlistStore.rename(from, to) }

    fun deletePlaylist(name: String) = mutate { playlistStore.delete(name) }

    fun addToPlaylist(name: String, tracks: List<Track>) = mutate {
        playlistStore.add(name, tracks)
        _messages.tryEmit("Added to \"$name\"")
    }

    fun removeFromPlaylist(name: String, index: Int, path: String?) = mutate { playlistStore.removeAt(name, index, path) }

    fun movePlaylistEntry(name: String, from: Int, to: Int) = mutate { playlistStore.move(name, from, to) }

    fun deleteTrack(track: Track, onDeleted: () -> Unit) {
        scope.launch {
            val file = File(root, track.path)
            if (file.delete() || !file.exists()) {
                withContext(Dispatchers.Main) { onDeleted() }
                _messages.tryEmit("Deleted \"${track.title}\"")
                refresh()
            } else {
                _messages.tryEmit("Could not delete \"${track.title}\"")
            }
        }
    }

    fun submitDownload(input: String, playlist: String?, playlistFromSource: Boolean, onDone: (Result<Unit>) -> Unit = {}) {
        scope.launch {
            val result = runCatching {
                ensureRoot(root)
                downloadStore.submit(input, playlist, playlistFromSource, settings.deviceName)
                Unit
            }
            result.exceptionOrNull()?.let { _messages.tryEmit(it.message ?: it.toString()) }
            if (_state.value.hasAccess) reloadDownloads()
            withContext(Dispatchers.Main) { onDone(result) }
        }
    }

    fun cancelDownload(id: String) = scope.launch {
        downloadStore.cancel(id)
        reloadDownloads()
    }

    fun clearFinishedDownloads() = scope.launch {
        downloadStore.clearFinished()
        reloadDownloads()
    }

    companion object {
        private const val DOWNLOAD_POLL_MS = 3_000L
        private const val LIBRARY_POLL_MS = 60_000L
    }
}
