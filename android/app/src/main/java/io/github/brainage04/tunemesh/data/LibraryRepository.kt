package io.github.brainage04.tunemesh.data

import android.os.Environment
import android.os.FileObserver
import android.util.Log
import io.github.brainage04.tunemesh.library.DownloadList
import io.github.brainage04.tunemesh.library.DownloadStore
import io.github.brainage04.tunemesh.library.Downloader
import io.github.brainage04.tunemesh.library.EntryRef
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.LibraryIndex
import io.github.brainage04.tunemesh.library.LibraryUris
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.PlaylistContents
import io.github.brainage04.tunemesh.library.PlaylistStore
import io.github.brainage04.tunemesh.library.PlaylistSummaries
import io.github.brainage04.tunemesh.library.RawTags
import io.github.brainage04.tunemesh.library.Stignore
import io.github.brainage04.tunemesh.library.Track
import java.io.File
import java.io.IOException
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

data class LibraryState(
    val root: File,
    val hasAccess: Boolean,
    val loaded: Boolean = false,
    val scanning: Boolean = false,
    val index: LibraryIndex = LibraryIndex.EMPTY,
    val playlists: List<Playlist> = emptyList(),
    val liked: Set<String> = emptySet(),
    /** Entries of the playlist screen that is open (see [LibraryRepository.openPlaylist]); only that one is held. */
    val openPlaylist: PlaylistContents? = null,
    val downloads: DownloadList = DownloadList.EMPTY,
    /** From `.tunemesh/worker.json`, re-evaluated on every downloads refresh. */
    val downloader: Downloader = Downloader.None,
    val error: String? = null,
) {
    /** Liked Songs always exists for the UI, even before its file is first written. */
    fun playlist(name: String): Playlist? =
        playlists.firstOrNull { it.name == name } ?: if (name == LibraryFormat.LIKED_SONGS) Playlist(name, 0, emptyList(), 0) else null

    val likedSongs: Playlist?
        get() = playlists.firstOrNull { it.isLikedSongs }
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
    private val refreshGate = RefreshGate(scanMutex)
    @Volatile private var openedPlaylist: String? = null
    /** `lastModified` and length of the files behind [LibraryState.liked] and [LibraryState.openPlaylist]. */
    private var likedStamp: Pair<Long, Long>? = null
    private var openStamp: Pair<Long, Long>? = null

    val root: File
        get() = _state.value.root

    private val summaries = PlaylistSummaries()
    private var storeFor: Pair<File, LibraryIndex>? = null
    private var store: PlaylistStore? = null

    /** One store per root and index, so [summaries] can tell that its track lookup did not change. */
    private val playlistStore: PlaylistStore
        @Synchronized
        get() {
            val state = _state.value
            val key = state.root to state.index
            store
                ?.takeIf { key == storeFor }
                ?.let {
                    return it
                }
            val index = state.index
            return PlaylistStore(state.root, summaries) { index.byPath[it] }
                .also {
                    store = it
                    storeFor = key
                }
        }

    private val downloadStore
        get() = DownloadStore(root)

    fun hasAccess(): Boolean = Environment.isExternalStorageManager()

    /**
     * The track at the library-relative [path], or null when [path] does not name an audio file inside the library (absolute, `..` or
     * symlinks out of it, not audio, missing). Paths the index does not know yet (before the first scan) get path-derived metadata.
     */
    fun track(path: String): Track? {
        val root = root
        val resolved = LibraryUris.resolvePath(path, root) ?: return null
        _state.value.index.byPath[resolved]?.let {
            return it
        }
        val file = File(root, resolved)
        if (!file.isFile) return null
        return Track.fromTags(resolved, RawTags(), file.length(), file.lastModified())
    }

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

    /**
     * Rescans the library, playlists and downloads. Requests made while one refresh waits to start are merged into it (start-up, the
     * activity coming to the foreground and file events otherwise queue one full rescan each).
     */
    suspend fun refresh() {
        refreshGate.run(::refreshLocked)
    }

    private fun refreshLocked() {
        val access = hasAccess()
        val root = settings.libraryRoot
        if (!access) {
            _state.update { it.copy(root = root, hasAccess = false) }
            return
        }
        _state.update { it.copy(root = root, hasAccess = true, scanning = true) }
        try {
            ensureRoot(root)
            val result = scanner.scan(root)
            // An unchanged library keeps its index: a new one would make every screen showing it recompose (and an open
            // playlist look its entries up again) after each periodic rescan.
            val current = _state.value.index
            val index = if (result.tracks == current.tracks) current else LibraryIndex(result.tracks)
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
        // Syncthing must never sync TuneMesh's temporary files. A .stignore that cannot be updated (a symlink,
        // say) is left alone, as on the desktop, and does not stop the library from loading.
        try {
            Stignore.ensure(root)
        } catch (error: IOException) {
            Log.w(TAG, "Cannot update $root/.stignore", error)
        }
    }

    /**
     * Re-reads the playlist summaries, and Liked Songs and the open playlist when their files changed ([force] after this app changed one).
     * Unchanged files are not read again, so a large one is never held twice.
     */
    private fun reloadPlaylists(force: Boolean = false) {
        if (force) summaries.clear()
        val store = playlistStore
        val playlists = store.list()
        _state.update { it.copy(playlists = playlists) }

        val likedFile = store.file(LibraryFormat.LIKED_SONGS)
        val liked = stamp(likedFile)
        if (force || liked != likedStamp) {
            likedStamp = liked
            val paths =
                try {
                    store.likedPaths()
                } catch (error: IOException) {
                    Log.w(TAG, "Cannot read Liked Songs", error)
                    emptySet()
                }
            _state.update { it.copy(liked = paths) }
        }
        reloadOpenPlaylist(force)
    }

    private fun stamp(file: File): Pair<Long, Long> = file.lastModified() to file.length()

    private fun reloadOpenPlaylist(force: Boolean) {
        val name = openedPlaylist
        if (name == null) {
            openStamp = null
            _state.update { it.copy(openPlaylist = null) }
            return
        }
        val store = playlistStore
        val stamp = stamp(store.file(name))
        if (!force && stamp == openStamp && _state.value.openPlaylist?.name == name) return
        openStamp = stamp
        // The old entries stay on screen while the new ones load (both are bounded by Limits.MAX_PLAYLIST_ENTRIES).
        val contents =
            try {
                store.contents(name)
            } catch (error: IOException) {
                Log.w(TAG, "Cannot read playlist $name", error)
                null
            }
        if (openedPlaylist == name) _state.update { it.copy(openPlaylist = contents) }
    }

    /** Loads the entries of playlist [name] for its screen; one playlist is open at a time. */
    fun openPlaylist(name: String) {
        if (openedPlaylist == name && _state.value.openPlaylist?.name == name) return
        openedPlaylist = name
        scope.launch { scanMutex.withLock { reloadOpenPlaylist(force = true) } }
    }

    /** Releases the entries of [name] when its screen goes away. */
    fun closePlaylist(name: String) {
        if (openedPlaylist != name) return
        openedPlaylist = null
        _state.update { state -> if (state.openPlaylist?.name == name) state.copy(openPlaylist = null) else state }
    }

    private fun reloadDownloads() {
        val store = downloadStore
        val downloads = store.list()
        val downloader = Downloader.of(store.worker())
        _state.update { it.copy(downloads = downloads, downloader = downloader) }
    }

    private suspend fun refreshDownloads() =
        withContext(Dispatchers.IO) {
            if (_state.value.hasAccess) reloadDownloads()
        }

    @Synchronized
    private fun observe(libraryDirectories: List<File>) {
        if (!foreground) return
        val directories =
            (libraryDirectories +
                    listOf(
                        File(root, LibraryFormat.PLAYLISTS_DIR),
                        File(root, "${LibraryFormat.STATE_DIR}/requests"),
                        File(root, "${LibraryFormat.STATE_DIR}/status"),
                    ))
                .filter { it.isDirectory }
                .toSet()
        if (directories == observedDirectories) return
        stopObserving()
        val mask =
            FileObserver.CREATE or FileObserver.DELETE or FileObserver.MOVED_FROM or FileObserver.MOVED_TO or FileObserver.CLOSE_WRITE
        observers = directories.map { directory ->
            object : FileObserver(directory, mask) {
                    override fun onEvent(event: Int, path: String?) {
                        if (path == null || path.startsWith(".~tunemesh-") || path.startsWith(".syncthing.")) return
                        scheduleRefresh()
                    }
                }
                .also { it.startWatching() }
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

    /**
     * Playlist changes run one at a time, in the order they were asked for, so each one reads the file the one before it wrote (two quick
     * taps would otherwise both read the old file and the later write drop the other).
     */
    private val mutations = Dispatchers.IO.limitedParallelism(1)

    @Suppress("TooGenericExceptionCaught") // any failed change is reported instead of ending the app; action does not suspend
    private fun mutate(action: () -> Unit) {
        scope.launch(mutations) {
            try {
                action()
            } catch (error: Exception) {
                _messages.tryEmit(error.message ?: error.toString())
            }
            scanMutex.withLock { reloadPlaylists(force = true) }
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

    fun removeFromPlaylist(name: String, entry: EntryRef) = mutate { playlistStore.remove(name, entry) }

    fun movePlaylistEntry(name: String, from: EntryRef, to: EntryRef) = mutate { playlistStore.move(name, from, to) }

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

    /** Queues one request per line of [text]; see [DownloadStore.submitAll]. */
    fun submitDownloads(text: String, playlist: String?, saveAsPlaylists: Boolean, onDone: (Result<Int>) -> Unit = {}) {
        scope.launch {
            val result = runCatching {
                ensureRoot(root)
                downloadStore.submitAll(text, playlist, saveAsPlaylists, settings.deviceName).size
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
        private const val TAG = "TuneMesh"
        private const val DOWNLOAD_POLL_MS = 3_000L
        private const val LIBRARY_POLL_MS = 60_000L
    }
}
