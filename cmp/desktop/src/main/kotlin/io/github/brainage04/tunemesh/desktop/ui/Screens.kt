package io.github.brainage04.tunemesh.desktop.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.theme.SurfaceHigh
import io.github.brainage04.tunemesh.ui.theme.TextSecondary
import java.time.LocalTime

private val TileWidth = 180.dp

@Composable
private fun playingContext(name: String): Boolean {
    val ui = LocalPlayerUi.current
    return ui.context == name && ui.playing
}

@Composable
fun HomeScreen() {
    val model = LocalAppModel.current
    val library = LocalLibrary.current
    val index = library.index
    val greeting = when (LocalTime.now().hour) {
        in 5..11 -> "Good morning"
        in 12..17 -> "Good afternoon"
        else -> "Good evening"
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item { PageTitle(greeting) }
        if (index.tracks.isEmpty() && library.playlists.isEmpty()) {
            item {
                EmptyMessage(
                    if (library.scanning || !library.loaded) "Scanning your library…" else "Your library is empty",
                    "Songs in ${library.root} show up here.",
                )
            }
            return@LazyColumn
        }
        val shortcuts = library.playlists.take(6)
        if (shortcuts.isNotEmpty()) {
            item {
                Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    shortcuts.chunked(3).forEach { row ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            row.forEach { PlaylistShortcut(it, Modifier.weight(1f)) }
                            repeat(3 - row.size) { Spacer(Modifier.weight(1f)) }
                        }
                    }
                }
            }
        }
        if (index.albums.isNotEmpty()) {
            item { SectionTitle("Recently added") }
            item {
                LazyRow(contentPadding = PaddingValues(horizontal = 14.dp)) {
                    items(index.recentlyAdded.take(20)) { album ->
                        MediaTile(album.title, album.artist, { model.navigate(Route.AlbumDetail(album.key)) }, Modifier.width(TileWidth)) {
                            CoverArt(album.tracks.first(), album.title, it, sizeHint = TileWidth)
                        }
                    }
                }
            }
        }
        if (library.playlists.isNotEmpty()) {
            item { SectionTitle("Your playlists") }
            item {
                LazyRow(contentPadding = PaddingValues(horizontal = 14.dp)) {
                    items(library.playlists, key = { it.name }) { playlist ->
                        MediaTile(playlist.name, "Playlist • ${songCount(playlist.entries.size)}", { model.navigate(Route.PlaylistDetail(playlist.name)) }, Modifier.width(TileWidth)) {
                            PlaylistArt(playlist, it, TileWidth)
                        }
                    }
                }
            }
        }
        if (index.artists.isNotEmpty()) {
            item { SectionTitle("Artists") }
            item {
                LazyRow(contentPadding = PaddingValues(horizontal = 14.dp)) {
                    items(index.artists.sortedByDescending { artist -> artist.albums.maxOf { it.addedMs } }.take(20), key = { it.name }) { artist ->
                        MediaTile(artist.name, "Artist", { model.openArtist(artist.name) }, Modifier.width(TileWidth), circle = true) {
                            CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = TileWidth, placeholderIcon = AppIcons.Person)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun PlaylistShortcut(playlist: Playlist, modifier: Modifier) {
    val model = LocalAppModel.current
    Row(
        modifier.height(64.dp).clip(RoundedCornerShape(4.dp)).background(SurfaceHigh).clickable { model.navigate(Route.PlaylistDetail(playlist.name)) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlaylistArt(playlist, Modifier.size(64.dp), 64.dp, RoundedCornerShape(topStart = 4.dp, bottomStart = 4.dp))
        Text(playlist.name, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(horizontal = 12.dp))
    }
}

@Composable
fun SearchScreen() {
    val model = LocalAppModel.current
    val library = LocalLibrary.current
    val index = library.index
    val query = model.query
    if (query.isBlank()) {
        LazyVerticalGrid(GridCells.Adaptive(TileWidth), Modifier.fillMaxSize(), contentPadding = PaddingValues(14.dp)) {
            item(span = { GridItemSpan(maxLineSpan) }) { PageTitle("Browse artists") }
            items(index.artists, key = { it.name }) { artist ->
                MediaTile(artist.name, songCount(artist.tracks.size), { model.openArtist(artist.name) }, circle = true) {
                    CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = TileWidth, placeholderIcon = AppIcons.Person)
                }
            }
        }
        return
    }
    val tracks = remember(query, index) { index.searchTracks(query) }
    val artists = remember(query, index) { index.searchArtists(query) }
    val albums = remember(query, index) { index.searchAlbums(query) }
    val playlists = remember(query, library.playlists, index) { index.searchPlaylists(query, library.playlists) }
    if (tracks.isEmpty() && artists.isEmpty() && albums.isEmpty() && playlists.isEmpty()) {
        EmptyMessage("No results found for \u201c$query\u201d", "Check the spelling, or request it as a download.")
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        if (tracks.isNotEmpty()) {
            item { SectionTitle("Songs") }
            itemsIndexed(tracks.take(50), key = { _, track -> track.path }) { position, track ->
                TrackRow(track, position + 1, onPlay = { model.play("Search \u201c$query\u201d", tracks, position) })
            }
        }
        tileSection("Artists", artists) { artist ->
            MediaTile(artist.name, "Artist", { model.openArtist(artist.name) }, Modifier.width(TileWidth), circle = true) {
                CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = TileWidth, placeholderIcon = AppIcons.Person)
            }
        }
        tileSection("Albums", albums) { album ->
            MediaTile(album.title, album.artist, { model.navigate(Route.AlbumDetail(album.key)) }, Modifier.width(TileWidth)) {
                CoverArt(album.tracks.first(), album.title, it, sizeHint = TileWidth)
            }
        }
        tileSection("Playlists", playlists) { playlist ->
            MediaTile(playlist.name, "Playlist", { model.navigate(Route.PlaylistDetail(playlist.name)) }, Modifier.width(TileWidth)) {
                PlaylistArt(playlist, it, TileWidth)
            }
        }
    }
}

private fun <T> LazyListScope.tileSection(title: String, items: List<T>, tile: @Composable (T) -> Unit) {
    if (items.isEmpty()) return
    item { SectionTitle(title) }
    item {
        LazyRow(contentPadding = PaddingValues(horizontal = 14.dp)) {
            items(items) { tile(it) }
        }
    }
}

@Composable
fun LibraryScreen(tab: LibraryTab) {
    val model = LocalAppModel.current
    val library = LocalLibrary.current
    val index = library.index
    Column(Modifier.fillMaxSize()) {
        PageTitle("Your Library") {
            IconButton(onClick = { model.dialog = DialogRequest.CreatePlaylist(emptyList()) }) { Icon(AppIcons.Add, "Create playlist") }
        }
        Row(Modifier.padding(horizontal = 24.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            LibraryTab.entries.forEach { option ->
                FilterChip(
                    selected = tab == option,
                    onClick = { model.navigate(Route.Library(option)) },
                    label = { Text(option.label) },
                    shape = RoundedCornerShape(50),
                    colors = FilterChipDefaults.filterChipColors(selectedContainerColor = MaterialTheme.colorScheme.primary, selectedLabelColor = Color.Black),
                )
            }
        }
        Spacer(Modifier.height(8.dp))
        when (tab) {
            LibraryTab.Songs -> {
                val songs = remember(index) {
                    index.tracks.sortedWith(compareBy<Track, String>(String.CASE_INSENSITIVE_ORDER) { it.title }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.artist })
                }
                LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
                    itemsIndexed(songs, key = { _, track -> track.path }) { position, track ->
                        TrackRow(track, position + 1, onPlay = { model.play("Library", songs, position) })
                    }
                }
            }
            else -> LazyVerticalGrid(GridCells.Adaptive(TileWidth), Modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = 14.dp)) {
                when (tab) {
                    LibraryTab.Playlists -> {
                        if (library.likedSongs == null) {
                            item {
                                MediaTile(LibraryFormat.LIKED_SONGS, "Playlist • 0 songs", { model.navigate(Route.PlaylistDetail(LibraryFormat.LIKED_SONGS)) }) { LikedSongsArt(it) }
                            }
                        }
                        items(library.playlists, key = { it.name }) { playlist ->
                            MediaTile(playlist.name, "Playlist • ${songCount(playlist.entries.size)}", { model.navigate(Route.PlaylistDetail(playlist.name)) }) {
                                PlaylistArt(playlist, it, TileWidth)
                            }
                        }
                    }
                    LibraryTab.Albums -> items(index.albums, key = { it.key }) { album ->
                        MediaTile(album.title, album.artist, { model.navigate(Route.AlbumDetail(album.key)) }) {
                            CoverArt(album.tracks.first(), album.title, it, sizeHint = TileWidth)
                        }
                    }
                    LibraryTab.Artists -> items(index.artists, key = { it.name }) { artist ->
                        MediaTile(artist.name, "Artist", { model.openArtist(artist.name) }, circle = true) {
                            CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = TileWidth, placeholderIcon = AppIcons.Person)
                        }
                    }
                    LibraryTab.Songs -> Unit
                }
            }
        }
    }
}

@Composable
private fun Missing(what: String) {
    val model = LocalAppModel.current
    Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        EmptyMessage("$what not found", "It may have been renamed or deleted on another device.")
        TextButton(onClick = { model.goBack() }) { Text("Go back") }
    }
}

@Composable
fun AlbumScreen(key: String) {
    val model = LocalAppModel.current
    val album = LocalLibrary.current.index.albumsByKey[key] ?: return Missing("Album")
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            DetailHeader(
                kind = "Album",
                title = album.title,
                subtitle = "${album.artist} • ${songCount(album.tracks.size)}, ${totalLength(album.tracks)}",
                seed = album.title,
                onPlay = { model.playOrToggle(album.title, album.tracks) },
                playingThis = playingContext(album.title),
                onShuffle = { model.play(album.title, album.tracks, -1, shuffle = true) },
                playEnabled = album.tracks.isNotEmpty(),
                art = { CoverArt(album.tracks.first(), album.title, it, sizeHint = 200.dp) },
                onSubtitleClick = { model.openArtist(album.artist) },
                actions = { AddAllMenu(album.tracks) },
            )
        }
        itemsIndexed(album.tracks, key = { _, track -> track.path }) { index, track ->
            TrackRow(track, track.trackNumber ?: (index + 1), onPlay = { model.play(album.title, album.tracks, index) }, context = RowContext(showAlbum = false, showCover = false))
        }
    }
}

@Composable
fun ArtistScreen(name: String) {
    val model = LocalAppModel.current
    val artist = LocalLibrary.current.index.artistsByName[name] ?: return Missing("Artist")
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            DetailHeader(
                kind = "Artist",
                title = artist.name,
                subtitle = "${artist.albums.size} ${if (artist.albums.size == 1) "album" else "albums"} • ${songCount(artist.tracks.size)}",
                seed = artist.name,
                onPlay = { model.playOrToggle(artist.name, artist.tracks) },
                playingThis = playingContext(artist.name),
                onShuffle = { model.play(artist.name, artist.tracks, -1, shuffle = true) },
                playEnabled = artist.tracks.isNotEmpty(),
                art = { CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = 200.dp, placeholderIcon = AppIcons.Person) },
                actions = { AddAllMenu(artist.tracks) },
            )
        }
        item { SectionTitle("Albums") }
        item {
            LazyRow(contentPadding = PaddingValues(horizontal = 14.dp)) {
                items(artist.albums, key = { it.key }) { album ->
                    MediaTile(album.title, songCount(album.tracks.size), { model.navigate(Route.AlbumDetail(album.key)) }, Modifier.width(TileWidth)) {
                        CoverArt(album.tracks.first(), album.title, it, sizeHint = TileWidth)
                    }
                }
            }
        }
        item { SectionTitle("Songs") }
        itemsIndexed(artist.tracks, key = { _, track -> track.path }) { index, track ->
            TrackRow(track, index + 1, onPlay = { model.play(artist.name, artist.tracks, index) }, context = RowContext(showArtist = false))
        }
    }
}

@Composable
fun PlaylistScreen(name: String) {
    val model = LocalAppModel.current
    val library = LocalLibrary.current
    val playlist = library.playlist(name) ?: return Missing("Playlist")
    val resolved = remember(playlist, library.index) { playlist.entries.map { entry -> entry.path?.let { library.index.byPath[it] } } }
    val playable = remember(resolved) { resolved.filterNotNull() }
    var editing by remember(name) { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            DetailHeader(
                kind = "Playlist",
                title = playlist.name,
                subtitle = buildString {
                    append(songCount(playlist.entries.size))
                    if (playable.size != playlist.entries.size) append(" • ${playlist.entries.size - playable.size} unavailable")
                    if (playable.isNotEmpty()) append(", ${totalLength(playable)}")
                },
                seed = playlist.name,
                onPlay = { model.playOrToggle(playlist.name, playable) },
                playingThis = playingContext(playlist.name),
                onShuffle = { model.play(playlist.name, playable, -1, shuffle = true) },
                playEnabled = playable.isNotEmpty(),
                art = { PlaylistArt(playlist, it, 200.dp) },
                actions = {
                    if (playlist.entries.size > 1) TextButton(onClick = { editing = !editing }) { Text(if (editing) "Done" else "Edit order") }
                    PlaylistMenu(playlist.name, playlist.isLikedSongs, playable)
                },
            )
        }
        if (playlist.entries.isEmpty()) {
            item {
                EmptyMessage(
                    if (playlist.isLikedSongs) "Songs you like appear here" else "This playlist is empty",
                    if (playlist.isLikedSongs) "Click the heart on any song." else "Add songs with \"Add to playlist\" from any song's menu.",
                )
            }
        }
        itemsIndexed(playlist.entries) { index, entry ->
            val track = resolved[index]
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) {
                    if (track != null) {
                        TrackRow(
                            track,
                            index + 1,
                            onPlay = { model.play(playlist.name, playable, resolved.take(index).count { it != null }) },
                            context = RowContext(playlistName = playlist.name, index = index),
                        )
                    } else {
                        UnavailableRow(entry.raw, index + 1, onRemove = { model.repository.removeFromPlaylist(playlist.name, index, entry.path) })
                    }
                }
                if (editing) {
                    IconButton(onClick = { model.repository.movePlaylistEntry(playlist.name, index, index - 1) }, enabled = index > 0) { Icon(AppIcons.Up, "Move up") }
                    IconButton(onClick = { model.repository.movePlaylistEntry(playlist.name, index, index + 1) }, enabled = index < playlist.entries.lastIndex) {
                        Icon(AppIcons.DownArrow, "Move down")
                    }
                }
            }
        }
    }
}

@Composable
private fun PlaylistMenu(name: String, likedSongs: Boolean, tracks: List<Track>) {
    val model = LocalAppModel.current
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(AppIcons.More, "Playlist options", tint = TextSecondary) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            if (tracks.isNotEmpty()) {
                MenuItem("Add to queue", AppIcons.Queue) { model.player.addToQueue(tracks); open = false }
                MenuItem("Add to other playlist", AppIcons.PlaylistAdd) { model.dialog = DialogRequest.AddToPlaylist(tracks); open = false }
            }
            if (!likedSongs) {
                MenuItem("Rename", AppIcons.Edit) { model.dialog = DialogRequest.RenamePlaylist(name); open = false }
                MenuItem("Delete playlist", AppIcons.Delete, color = MaterialTheme.colorScheme.error) { model.dialog = DialogRequest.DeletePlaylist(name); open = false }
            }
        }
    }
}

@Composable
private fun AddAllMenu(tracks: List<Track>) {
    val model = LocalAppModel.current
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(AppIcons.More, "More options", tint = TextSecondary) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            MenuItem("Play next", AppIcons.Queue) { model.player.playNext(tracks); open = false }
            MenuItem("Add to queue", AppIcons.Queue) { model.player.addToQueue(tracks); open = false }
            MenuItem("Add to playlist", AppIcons.PlaylistAdd) { model.dialog = DialogRequest.AddToPlaylist(tracks); open = false }
        }
    }
}
