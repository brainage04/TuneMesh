package io.github.brainage04.tunemesh.data

import android.os.Environment
import android.os.FileObserver
import android.util.Log
import io.github.brainage04.tunemesh.library.DirectoryStamps
import io.github.brainage04.tunemesh.library.DownloadList
import io.github.brainage04.tunemesh.library.DownloadStore
import io.github.brainage04.tunemesh.library.Downloader
import io.github.brainage04.tunemesh.library.EntryRef
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.LibraryIndex
import io.github.brainage04.tunemesh.library.LibraryUris
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.PlaylistBases
import io.github.brainage04.tunemesh.library.PlaylistContents
import io.github.brainage04.tunemesh.library.PlaylistStore
import io.github.brainage04.tunemesh.library.PlaylistSummaries
import io.github.brainage04.tunemesh.library.RawTags
import io.github.brainage04.tunemesh.library.Stignore
import io.github.brainage04.tunemesh.library.Track
import java.io.File
import java.io.IOException
import java.util.Locale
import java.util.concurrent.atomic.AtomicInteger
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
    /** Read while the Downloads screen is shown ([LibraryRepository.setDownloadsShown]); null until it first was. */
    val downloads: DownloadList? = null,
    /** From `.tunemesh/worker.json`, re-evaluated on every downloads refresh. */
    val downloader: Downloader = Downloader.None,
    /** Why the last downloads refresh failed, shown on the Downloads tab; null after one that worked. */
    val downloadsError: String? = null,
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
    /** App-private directory of the playlist bases (contract: "Playlist conflict copies"), one subdirectory per library. */
    private val playlistBases: File,
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
    /** How many Downloads screens are shown; the downloads list is read and polled only while one is. */
    private val downloadsShown = AtomicInteger()
    private var pendingRefresh: Job? = null
    private val refreshGate = RefreshGate(scanMutex)
    private val openSlot = OpenPlaylistSlot()
    /** `lastModified` and length of the files behind [LibraryState.liked] and [LibraryState.openPlaylist]. */
    private var likedStamp: Pair<Long, Long>? = null
    private var openStamp: Pair<Long, Long>? = null
    /** Conflict copies already logged as stuck (see [PlaylistStore.mergeConflicts]), so each is logged once per process. */
    private val stuckCopies: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

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
            return PlaylistStore(state.root, summaries, PlaylistBases(File(playlistBases, libraryKey(state.root))), index).also {
                store = it
                storeFor = key
            }
        }

    /** The library folder the last full scan walked, and its folders' stamps then; null until one worked. */
    @Volatile private var scanned: Pair<File, DirectoryStamps>? = null

    private var downloadStoreFor: DownloadStore? = null

    /** One store per root, so it can keep what it read of unchanged request and status files. */
    private val downloadStore: DownloadStore
        @Synchronized
        get() {
            val root = root
            return downloadStoreFor?.takeIf { it.root == root } ?: DownloadStore(root).also { downloadStoreFor = it }
        }

    fun hasAccess(): Boolean = Environment.isExternalStorageManager()

    /**
     * The track at the library-relative [path], or null when [path] does not name an audio file inside the library (absolute, `..` or
     * symlinks out of it, not audio, missing). Paths the index does not know yet (before the first scan) get path-derived metadata.
     */
    fun track(path: String): Track? {
        val root = root
        val resolved = LibraryUris.resolvePath(path, root) ?: return null
        _state.value.index.track(resolved)?.let {
            return it
        }
        val file = File(root, resolved)
        if (!file.isFile) return null
        return Track.fromTags(resolved, RawTags(), file.length(), file.lastModified())
    }

    /** Shows cached tags immediately, then rescans. */
    fun start() {
        scope.launch {
            // The folder this start is for: the property reads the state's, which a later folder change replaces.
            val root = root
            val cached = scanner.cached(root)
            if (cached != null && cached.size > 0 && !_state.value.loaded) {
                _state.update { state -> if (state.root == root) state.copy(index = cached, loaded = true) else state }
            }
            refresh()
        }
    }

    /** Called while an activity is visible: observes the tree and polls as a safety net (the downloads only while they are shown). */
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
                if (ticks % (LIBRARY_POLL_MS / DOWNLOAD_POLL_MS) == 0L) checkLibrary() else if (downloadsShown.get() > 0) refreshDownloads()
            }
        }
    }

    /**
     * Called while the Downloads screen is shown ([shown] true) and when it goes away. Its list is read when it appears and polled while it
     * is shown; other screens only need `worker.json`, read on every refresh.
     */
    fun setDownloadsShown(shown: Boolean) {
        if (shown) {
            downloadsShown.incrementAndGet()
            scope.launch { refreshDownloads() }
        } else {
            downloadsShown.decrementAndGet()
        }
    }

    /**
     * The periodic safety net: a full rescan only when a folder of the library changed since the last one ([DirectoryStamps]: one `stat`
     * per folder, nothing allocated), else just `worker.json` and the shown downloads. Walking 100,000 unchanged files allocated 60-160 MB
     * a minute.
     */
    private suspend fun checkLibrary() {
        val last = scanned
        if (last == null || last.first != root || last.second.changed()) refresh() else refreshDownloads()
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
        // An unchanged library keeps its index (the scan returns the one it was given): a new one would make every screen showing it
        // recompose (and an open playlist look its entries up again) after each periodic rescan.
        var previous = LibraryIndex.EMPTY
        // The state is that of another folder when the library folder changed after [root] was read: that change starts a refresh of its
        // own, so this one does nothing.
        val current =
            updateIfRoot(root) {
                previous = it.index
                if (access) it.copy(hasAccess = true, scanning = true) else it.copy(hasAccess = false)
            }
        if (!current || !access) return
        try {
            ensureRoot(root)
            val result = scanner.scan(root, previous)
            // The library folder changed during the scan: the old folder's tracks are not shown under the new one (the change starts a
            // scan of its own, which also must not take them as its previous index).
            if (!updateIfRoot(root) { it.copy(index = result.index, loaded = true, error = null) }) return
            // Playlists/ is not walked; its stamp is taken before the playlists are read, so a change while they are read is seen.
            val playlistsDirectory = File(root, LibraryFormat.PLAYLISTS_DIR).toPath()
            result.directories.add(playlistsDirectory, DirectoryStamps.modifiedMs(playlistsDirectory))
            // A new index: playlist entries hold track ordinals of the index they were read with, so the open one is read again.
            reloadPlaylists(changed = if (result.index === previous) emptySet() else null)
            // Conflict copies are merged with the playlist changes, one at a time; listing their names costs nothing.
            if (playlistStore.hasConflictCopies()) {
                mutate {
                    playlistStore.mergeConflicts { copy ->
                        if (stuckCopies.add(copy)) {
                            Log.w(TAG, "Playlists/$copy cannot be merged; it stays (holding the base) until it can be or is deleted")
                        }
                    }
                }
            }
            reloadDownloads()
            observe(result.directories.files)
            scanned = root to result.directories
        } catch (error: IOException) {
            scanned = null
            updateIfRoot(root) { it.copy(loaded = true, error = error.message ?: "Cannot read $root") }
        } finally {
            updateIfRoot(root) { it.copy(scanning = false) }
        }
    }

    /** Applies [change] while the state is still that of library [root]; false when the library folder changed meanwhile. */
    private fun updateIfRoot(root: File, change: (LibraryState) -> LibraryState): Boolean {
        var applied = false
        _state.update { state ->
            applied = state.root == root
            if (applied) change(state) else state
        }
        return applied
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
     * Re-reads the playlist summaries, and Liked Songs and the open playlist when their files changed. [changed] are the playlists this app
     * just changed: they are read again even when their modification time and length look the same (a change within the clock's
     * resolution); null means any playlist may have changed. Unchanged files are not read again, so a like reads Liked Songs only and a
     * large playlist is never held twice.
     */
    private fun reloadPlaylists(changed: Set<String>? = emptySet()) {
        if (changed == null) summaries.clear() else summaries.forget(changed.map { it + LibraryFormat.PLAYLIST_EXTENSION })
        val store = playlistStore
        val playlists = store.list()
        _state.update { it.copy(playlists = playlists) }

        val likedFile = store.file(LibraryFormat.LIKED_SONGS)
        val liked = stamp(likedFile)
        if (changed == null || LibraryFormat.LIKED_SONGS in changed || liked != likedStamp) {
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
        reloadOpenPlaylist(force = changed == null || openSlot.current.name in changed)
    }

    private fun stamp(file: File): Pair<Long, Long> = file.lastModified() to file.length()

    private fun reloadOpenPlaylist(force: Boolean) {
        val ticket = openSlot.current
        val name = ticket.name
        if (name == null) {
            openSlot.ifCurrent(ticket) {
                openStamp = null
                _state.update { it.copy(openPlaylist = null) }
            }
            return
        }
        val store = playlistStore
        val stamp = stamp(store.file(name))
        if (!force && stamp == openStamp && _state.value.openPlaylist?.name == name) return
        // The old entries stay on screen while the new ones load (both are bounded by Limits.MAX_PLAYLIST_ENTRIES).
        val contents =
            try {
                store.contents(name)
            } catch (error: IOException) {
                Log.w(TAG, "Cannot read playlist $name", error)
                null
            }
        // Dropped when the screen closed or another playlist opened meanwhile.
        openSlot.ifCurrent(ticket) {
            openStamp = stamp
            _state.update { it.copy(openPlaylist = contents) }
        }
    }

    /** Loads the entries of playlist [name] for its screen; one playlist is open at a time. */
    fun openPlaylist(name: String) {
        if (!openSlot.open(name) && _state.value.openPlaylist?.name == name) return
        scope.launch { scanMutex.withLock { reloadOpenPlaylist(force = true) } }
    }

    /** Releases the entries of [name] when its screen goes away. */
    fun closePlaylist(name: String) {
        openSlot.close(name) {
            openStamp = null
            _state.update { state -> if (state.openPlaylist?.name == name) state.copy(openPlaylist = null) else state }
        }
    }

    /**
     * Reads `worker.json`, and the Downloads list while the Downloads screen is shown ([setDownloadsShown]). A failure is shown on the
     * Downloads tab instead of ending the app; running out of memory also drops the list held so far (the reads are bounded, so that takes
     * a much smaller heap than the limits plan for).
     */
    @Suppress("TooGenericExceptionCaught") // anything a synced file makes the reader throw is reported, never fatal
    private fun reloadDownloads() {
        try {
            val store = downloadStore
            val downloads = if (downloadsShown.get() > 0) store.list() else null
            val downloader = Downloader.of(store.worker())
            _state.update {
                it.copy(
                    downloads = downloads ?: it.downloads,
                    downloader = downloader,
                    downloadsError = it.downloadsError.takeIf { downloads == null },
                )
            }
        } catch (error: RuntimeException) {
            Log.w(TAG, "Cannot read the downloads", error)
            _state.update { it.copy(downloadsError = "Cannot read the downloads: ${error.message ?: error.javaClass.simpleName}") }
        } catch (error: OutOfMemoryError) {
            _state.update { it.copy(downloads = DownloadList.EMPTY, downloadsError = "Not enough memory to read the downloads") }
            Log.w(TAG, "Out of memory reading the downloads", error)
        }
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
        // Absolute, as the settings keep it: a refresh applies its scan only while the state's root is the one it read from the settings.
        val file = File(path.trim()).absoluteFile
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

    /**
     * Runs [action], which returns the playlists it changed; then only those are read again. A failed change reloads everything (the
     * optimistic like is undone that way).
     */
    @Suppress("TooGenericExceptionCaught") // any failed change is reported instead of ending the app; action does not suspend
    private fun mutate(action: () -> Set<String>) {
        scope.launch(mutations) {
            val changed =
                try {
                    action()
                } catch (error: Exception) {
                    _messages.tryEmit(error.message ?: error.toString())
                    null
                }
            scanMutex.withLock { reloadPlaylists(changed) }
        }
    }

    fun setLiked(track: Track, liked: Boolean) {
        _state.update { state -> state.copy(liked = if (liked) state.liked + track.path else state.liked - track.path) }
        mutate {
            playlistStore.setLiked(track, liked)
            setOf(LibraryFormat.LIKED_SONGS)
        }
    }

    fun createPlaylist(name: String, tracks: List<Track> = emptyList()) = mutate {
        val created = playlistStore.create(name)
        if (tracks.isNotEmpty()) playlistStore.add(created, tracks)
        _messages.tryEmit(if (tracks.isEmpty()) "Created \"$created\"" else "Added to \"$created\"")
        setOf(created)
    }

    fun renamePlaylist(from: String, to: String) = mutate { setOf(from, playlistStore.rename(from, to)) }

    fun deletePlaylist(name: String) = mutate {
        playlistStore.delete(name)
        setOf(name)
    }

    fun addToPlaylist(name: String, tracks: List<Track>) = mutate {
        playlistStore.add(name, tracks)
        _messages.tryEmit("Added to \"$name\"")
        setOf(name)
    }

    fun removeFromPlaylist(name: String, entry: EntryRef) = mutate {
        playlistStore.remove(name, entry)
        setOf(name)
    }

    fun movePlaylistEntry(name: String, from: EntryRef, to: EntryRef) = mutate {
        playlistStore.move(name, from, to)
        setOf(name)
    }

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

        private const val LIBRARY_KEY_BYTES = 8

        /** The bases subdirectory of the library at [root]: the start of the SHA-256 of its path. */
        fun libraryKey(root: File): String =
            PlaylistBases.sha256(root.absolutePath.toByteArray(Charsets.UTF_8)).take(LIBRARY_KEY_BYTES).joinToString("") {
                "%02x".format(Locale.ROOT, it)
            }

        private const val DOWNLOAD_POLL_MS = 3_000L
        private const val LIBRARY_POLL_MS = 60_000L
    }
}
