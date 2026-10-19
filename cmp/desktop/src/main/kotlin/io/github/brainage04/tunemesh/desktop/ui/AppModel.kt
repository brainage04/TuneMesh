package io.github.brainage04.tunemesh.desktop.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.brainage04.tunemesh.desktop.data.Covers
import io.github.brainage04.tunemesh.desktop.data.LibraryRepository
import io.github.brainage04.tunemesh.desktop.playback.Player
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Track

enum class LibraryTab(val label: String) { Playlists("Playlists"), Albums("Albums"), Artists("Artists"), Songs("Songs") }

sealed interface Route {
    data object Home : Route
    data object Search : Route
    data class Library(val tab: LibraryTab) : Route
    data class AlbumDetail(val key: String) : Route
    data class ArtistDetail(val name: String) : Route
    data class PlaylistDetail(val name: String) : Route
    data object NowPlaying : Route
}

sealed interface DialogRequest {
    data class AddToPlaylist(val tracks: List<Track>) : DialogRequest
    data class CreatePlaylist(val tracks: List<Track>) : DialogRequest
    data class RenamePlaylist(val name: String) : DialogRequest
    data class DeletePlaylist(val name: String) : DialogRequest
}

/** Navigation history (back and forward, as in a browser), the queue panel, dialogs and track actions. */
class AppModel(val repository: LibraryRepository, val covers: Covers, val player: Player) {
    private val back = mutableStateListOf<Route>()
    private val forward = mutableStateListOf<Route>()
    var route by mutableStateOf<Route>(Route.Home)
        private set
    var query by mutableStateOf("")
    var queueOpen by mutableStateOf(false)
    var dialog by mutableStateOf<DialogRequest?>(null)

    val canGoBack get() = back.isNotEmpty()
    val canGoForward get() = forward.isNotEmpty()

    fun navigate(target: Route) {
        if (target == route) return
        back += route
        forward.clear()
        route = target
    }

    fun goBack() {
        if (back.isEmpty()) return
        forward += route
        route = back.removeAt(back.lastIndex)
    }

    fun goForward() {
        if (forward.isEmpty()) return
        back += route
        route = forward.removeAt(forward.lastIndex)
    }

    fun search(text: String) {
        query = text
        if (route != Route.Search) navigate(Route.Search)
    }

    fun play(context: String, tracks: List<Track>, index: Int, shuffle: Boolean? = null) = player.play(context, tracks, index, shuffle)

    /** Header Play button: pauses/resumes the context if it is already loaded, else starts it. */
    fun playOrToggle(context: String, tracks: List<Track>) {
        val ui = player.ui.value
        if (ui.context == context && ui.current != null) player.togglePlay() else play(context, tracks, 0, shuffle = false)
    }

    fun isLiked(path: String): Boolean = path in repository.state.value.liked

    fun toggleLike(track: Track) = repository.setLiked(track, !isLiked(track.path))

    fun openAlbum(track: Track) = navigate(Route.AlbumDetail(track.albumKey))

    fun openArtist(name: String) = navigate(Route.ArtistDetail(name))

    fun deletePlaylist(name: String) {
        repository.deletePlaylist(name)
        if (route == Route.PlaylistDetail(name)) goBack()
    }

    fun renamePlaylist(from: String, to: String) {
        repository.renamePlaylist(from, to)
        val renamed = LibraryFormat.playlistFileName(to).removeSuffix(LibraryFormat.PLAYLIST_EXTENSION)
        if (route == Route.PlaylistDetail(from)) route = Route.PlaylistDetail(renamed)
    }

    /** Parses the GTK app's `app.navigate` route syntax (`album:<name>`, `playlist:<name>`, ...). */
    fun parseRoute(text: String): Route? {
        val kind = text.substringBefore(':')
        val arg = text.substringAfter(':', "")
        val index = repository.state.value.index
        return when (kind) {
            "home" -> Route.Home
            "search" -> { query = arg; Route.Search }
            "albums" -> Route.Library(LibraryTab.Albums)
            "artists" -> Route.Library(LibraryTab.Artists)
            "playlists" -> Route.Library(LibraryTab.Playlists)
            "songs" -> Route.Library(LibraryTab.Songs)
            "liked" -> Route.PlaylistDetail(LibraryFormat.LIKED_SONGS)
            "album" -> index.albums.firstOrNull { it.title == arg || it.key == arg }?.let { Route.AlbumDetail(it.key) }
            "artist" -> Route.ArtistDetail(arg)
            "playlist" -> Route.PlaylistDetail(arg)
            "player" -> Route.NowPlaying
            else -> null
        }
    }

    /** Tracks and context name a route plays, for the `play-route` control command. */
    fun routeTracks(route: Route): Pair<String, List<Track>> {
        val state = repository.state.value
        return when (route) {
            is Route.AlbumDetail -> state.index.albumsByKey[route.key]?.let { it.title to it.tracks }
            is Route.ArtistDetail -> state.index.artistsByName[route.name]?.let { it.name to it.tracks }
            is Route.PlaylistDetail -> state.playlist(route.name)?.let { playlist ->
                playlist.name to playlist.entries.mapNotNull { entry -> entry.path?.let { state.index.byPath[it] } }
            }
            else -> null
        } ?: ("Library" to state.index.tracks)
    }
}
