package io.github.brainage04.tunemesh.ui

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.lifecycle.AndroidViewModel
import io.github.brainage04.tunemesh.TuneMeshApp
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.playback.PlayerConnection

enum class Tab(val label: String) {
    Home("Home"),
    Search("Search"),
    Library("Your Library"),
    Downloads("Downloads"),
}

sealed interface Screen {
    data object Root : Screen

    data class AlbumDetail(val key: String) : Screen

    data class ArtistDetail(val name: String) : Screen

    data class PlaylistDetail(val name: String) : Screen

    data object Settings : Screen
}

sealed interface DialogRequest {
    data class AddToPlaylist(val tracks: List<Track>) : DialogRequest

    data class CreatePlaylist(val tracks: List<Track>) : DialogRequest

    data class RenamePlaylist(val name: String) : DialogRequest

    data class DeletePlaylist(val name: String) : DialogRequest

    data class DeleteTrack(val track: Track) : DialogRequest
}

/** Navigation, overlays and the actions every track row offers. Survives configuration changes. */
class AppViewModel(application: Application) : AndroidViewModel(application) {
    val app = application as TuneMeshApp
    val repository = app.repository
    val covers = app.covers
    val player = PlayerConnection(application, app.playbackState)

    var tab by mutableStateOf(Tab.Home)
        private set

    private val stacks: Map<Tab, SnapshotStateList<Screen>> = Tab.entries.associateWith { mutableStateListOf<Screen>(Screen.Root) }
    var playerExpanded by mutableStateOf(false)
    var queueOpen by mutableStateOf(false)
    var dialog by mutableStateOf<DialogRequest?>(null)
    var playingFrom by mutableStateOf(app.getSharedPreferences("playback", 0).getString(KEY_CONTEXT, "").orEmpty())
        private set

    /** The onboarding's Syncthing step: shown once after All files access is granted, reopened from Settings. */
    var syncthingSetupOpen by mutableStateOf(!app.settings.syncthingSetupSeen)
        private set

    fun openSyncthingSetup() {
        syncthingSetupOpen = true
    }

    fun closeSyncthingSetup() {
        app.settings.syncthingSetupSeen = true
        syncthingSetupOpen = false
    }

    val screen: Screen
        get() = stacks.getValue(tab).last()

    fun select(target: Tab) {
        if (target == tab) {
            stacks.getValue(tab).let { stack -> while (stack.size > 1) stack.removeAt(stack.lastIndex) }
        }
        tab = target
    }

    fun push(screen: Screen) {
        playerExpanded = false
        queueOpen = false
        val stack = stacks.getValue(tab)
        if (stack.last() != screen) stack.add(screen)
    }

    /** Handles system back; false when there is nothing left to close. */
    fun back(): Boolean {
        when {
            queueOpen -> queueOpen = false
            playerExpanded -> playerExpanded = false
            stacks.getValue(tab).size > 1 -> stacks.getValue(tab).removeAt(stacks.getValue(tab).lastIndex)
            tab != Tab.Home -> tab = Tab.Home
            else -> return false
        }
        return true
    }

    private val root
        get() = repository.root

    fun play(context: String, tracks: List<Track>, index: Int, shuffle: Boolean? = null) {
        playingFrom = context
        app.getSharedPreferences("playback", 0).edit().putString(KEY_CONTEXT, context).apply()
        player.play(root, tracks, index, shuffle)
    }

    /** Header Play button: resumes/pauses the context if it is already loaded, else starts it. */
    fun playOrToggle(context: String, tracks: List<Track>) {
        if (playingFrom == context && player.ui.value.current != null) player.togglePlay() else play(context, tracks, 0, shuffle = false)
    }

    fun playNext(tracks: List<Track>) = player.playNext(root, tracks)

    fun addToQueue(tracks: List<Track>) = player.addToQueue(root, tracks)

    fun isLiked(path: String): Boolean = path in repository.state.value.liked

    fun toggleLike(track: Track) = repository.setLiked(track, !isLiked(track.path))

    fun openAlbum(track: Track) = push(Screen.AlbumDetail(track.albumKey))

    fun openArtist(name: String) = push(Screen.ArtistDetail(name))

    fun deleteTrack(track: Track) {
        repository.deleteTrack(track) { player.removePath(track.path) }
    }

    fun deletePlaylist(name: String) {
        repository.deletePlaylist(name)
        stacks.values.forEach { stack -> stack.removeAll { it == Screen.PlaylistDetail(name) } }
    }

    fun renamePlaylist(from: String, to: String) {
        repository.renamePlaylist(from, to)
        val renamed = LibraryFormat.playlistFileName(to).removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        stacks.values.forEach { stack ->
            stack.replaceAll { if (it == Screen.PlaylistDetail(from)) Screen.PlaylistDetail(renamed) else it }
        }
    }

    override fun onCleared() {
        player.disconnect()
    }

    companion object {
        private const val KEY_CONTEXT = "playingFrom"
    }
}
