package io.github.brainage04.tunemesh.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.library.Album
import io.github.brainage04.tunemesh.library.Artist
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Limits
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.ClearFocusWhenImeHidden
import io.github.brainage04.tunemesh.ui.CoverArt
import io.github.brainage04.tunemesh.ui.DialogRequest
import io.github.brainage04.tunemesh.ui.EmptyMessage
import io.github.brainage04.tunemesh.ui.LikedSongsArt
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.MediaTile
import io.github.brainage04.tunemesh.ui.Screen
import io.github.brainage04.tunemesh.ui.ScreenTitle
import io.github.brainage04.tunemesh.ui.SectionTitle
import io.github.brainage04.tunemesh.ui.TrackRow
import io.github.brainage04.tunemesh.ui.playlistSubtitle
import io.github.brainage04.tunemesh.ui.songCount
import io.github.brainage04.tunemesh.ui.theme.TextSecondary
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

@Composable
fun SearchScreen(contentPadding: PaddingValues, background: CoroutineDispatcher = Dispatchers.Default) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    var query by rememberSaveable { mutableStateOf("") }
    val index = library.index
    // Searching 100,000 tracks takes longer than a frame: the results are worked out off the main thread, after a pause in typing,
    // and the previous ones stay on screen meanwhile.
    val results by
        produceState<SearchResults?>(null, query, index, library.playlists) {
            if (query.isBlank()) {
                value = null
                return@produceState
            }
            delay(SEARCH_DELAY_MS)
            val playlists = library.playlists
            value =
                withContext(background) {
                    SearchResults(
                        query = query,
                        tracks = index.searchTracks(query),
                        artists = index.searchArtists(query),
                        albums = index.searchAlbums(query),
                        playlists = index.searchPlaylists(query, playlists),
                    )
                }
        }
    ClearFocusWhenImeHidden()
    Column(Modifier.fillMaxSize()) {
        ScreenTitle("Search")
        TextField(
            value = query,
            onValueChange = { query = it },
            placeholder = { Text("What do you want to listen to?", color = Color(0xFF535353)) },
            leadingIcon = { Icon(AppIcons.Search, null, tint = Color.Black) },
            trailingIcon = {
                if (query.isNotEmpty()) IconButton(onClick = { query = "" }) { Icon(AppIcons.Close, "Clear", tint = Color.Black) }
            },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            shape = RoundedCornerShape(8.dp),
            colors =
                TextFieldDefaults.colors(
                    focusedContainerColor = Color.White,
                    unfocusedContainerColor = Color.White,
                    focusedTextColor = Color.Black,
                    unfocusedTextColor = Color.Black,
                    cursorColor = Color.Black,
                    focusedIndicatorColor = Color.Transparent,
                    unfocusedIndicatorColor = Color.Transparent,
                ),
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        )
        if (query.isBlank()) {
            if (index.artists.isEmpty()) return@Column
            Text(
                "Browse artists",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(16.dp),
            )
            LazyVerticalGrid(
                columns = GridCells.Adaptive(150.dp),
                contentPadding = PaddingValues(start = 10.dp, end = 10.dp, bottom = contentPadding.calculateBottomPadding()),
            ) {
                items(index.artists, key = { it.name }) { artist ->
                    MediaTile(artist.name, songCount(artist.tracks.size), { viewModel.openArtist(artist.name) }, circle = true) {
                        CoverArt(
                            artist.tracks.first(),
                            artist.name,
                            it,
                            shape = CircleShape,
                            sizeHint = 150.dp,
                            placeholderIcon = AppIcons.Person,
                        )
                    }
                }
            }
            return@Column
        }
        val found = results ?: return@Column
        val tracks = found.tracks
        val artists = found.artists
        val albums = found.albums
        val playlists = found.playlists
        if (tracks.isEmpty() && artists.isEmpty() && albums.isEmpty() && playlists.isEmpty()) {
            EmptyMessage("No results for \"${found.query}\"", "Check the spelling, or request it from the Downloads tab.")
            return@Column
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = contentPadding.calculateBottomPadding())) {
            if (artists.isNotEmpty() || albums.isNotEmpty() || playlists.isNotEmpty()) {
                item {
                    LazyRow(contentPadding = PaddingValues(horizontal = 10.dp, vertical = 8.dp)) {
                        items(artists, key = { "artist:" + it.name }) { artist ->
                            MediaTile(artist.name, "Artist", { viewModel.openArtist(artist.name) }, Modifier.width(130.dp), circle = true) {
                                CoverArt(
                                    artist.tracks.first(),
                                    artist.name,
                                    it,
                                    shape = CircleShape,
                                    sizeHint = 130.dp,
                                    placeholderIcon = AppIcons.Person,
                                )
                            }
                        }
                        items(albums, key = { "album:" + it.key }) { album ->
                            MediaTile(
                                album.title,
                                "Album • ${album.artist}",
                                { viewModel.push(Screen.AlbumDetail(album.key)) },
                                Modifier.width(130.dp),
                            ) {
                                CoverArt(album.tracks.first(), album.title, it, sizeHint = 130.dp)
                            }
                        }
                        items(playlists, key = { "playlist:" + it.name }) { playlist ->
                            MediaTile(
                                playlist.name,
                                "Playlist",
                                { viewModel.push(Screen.PlaylistDetail(playlist.name)) },
                                Modifier.width(130.dp),
                            ) {
                                PlaylistArt(playlist, it, 130.dp)
                            }
                        }
                    }
                }
            }
            if (tracks.isNotEmpty()) {
                item { SectionTitle("Songs") }
                itemsIndexed(tracks, key = { _, track -> track.path }) { position, track ->
                    TrackRow(track, onPlay = { viewModel.play("\"${found.query}\"", tracks, position) })
                }
            }
        }
    }
}

private class SearchResults(
    val query: String,
    val tracks: List<Track>,
    val artists: List<Artist>,
    val albums: List<Album>,
    val playlists: List<Playlist>,
)

private const val SEARCH_DELAY_MS = 150L

private enum class LibraryFilter(val label: String) {
    Playlists("Playlists"),
    Albums("Albums"),
    Artists("Artists"),
}

@Composable
fun LibraryScreen(contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    var filter by rememberSaveable { mutableStateOf<LibraryFilter?>(null) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item {
            ScreenTitle("Your Library") {
                IconButton(onClick = { viewModel.dialog = DialogRequest.CreatePlaylist(emptyList()) }) {
                    Icon(AppIcons.Add, "Create playlist")
                }
                IconButton(onClick = { viewModel.push(Screen.Settings) }) { Icon(AppIcons.Settings, "Settings") }
            }
        }
        item {
            LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                items(LibraryFilter.entries) { option ->
                    FilterChip(
                        selected = filter == option,
                        onClick = { filter = if (filter == option) null else option },
                        label = { Text(option.label) },
                        shape = RoundedCornerShape(50),
                        colors =
                            FilterChipDefaults.filterChipColors(
                                selectedContainerColor = MaterialTheme.colorScheme.primary,
                                selectedLabelColor = Color.Black,
                            ),
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
        }
        if (library.index.tracks.isEmpty() && library.playlists.isEmpty()) {
            item { EmptyMessage("Nothing here yet", "Create a playlist with + or request songs from Downloads.") }
        }
        if (filter == null || filter == LibraryFilter.Playlists) {
            if (library.likedSongs == null) {
                item {
                    LibraryRow("Liked Songs", "Playlist • 0 songs", { viewModel.push(Screen.PlaylistDetail(LibraryFormat.LIKED_SONGS)) }) {
                        LikedSongsArt(it)
                    }
                }
            }
            items(library.playlists, key = { "p:" + it.name }) { playlist ->
                LibraryRow(playlist.name, playlistSubtitle(playlist), { viewModel.push(Screen.PlaylistDetail(playlist.name)) }) {
                    PlaylistArt(playlist, it, 64.dp)
                }
            }
        }
        if (filter == null || filter == LibraryFilter.Albums) {
            items(library.index.albums, key = { "a:" + it.key }) { album ->
                LibraryRow(album.title, "Album • ${album.artist}", { viewModel.push(Screen.AlbumDetail(album.key)) }) {
                    CoverArt(album.tracks.first(), album.title, it, sizeHint = 64.dp)
                }
            }
        }
        if (filter == null || filter == LibraryFilter.Artists) {
            items(library.index.artists, key = { "r:" + it.name }) { artist ->
                LibraryRow(artist.name, "Artist", { viewModel.openArtist(artist.name) }) {
                    CoverArt(
                        artist.tracks.first(),
                        artist.name,
                        it,
                        shape = CircleShape,
                        sizeHint = 64.dp,
                        placeholderIcon = AppIcons.Person,
                    )
                }
            }
        }
    }
}

@Composable
private fun LibraryRow(title: String, subtitle: String, onClick: () -> Unit, art: @Composable (Modifier) -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        art(Modifier.size(64.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(Limits.shownLine(title), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                Limits.shownLine(subtitle),
                color = TextSecondary,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}
