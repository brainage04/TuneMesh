package io.github.brainage04.tunemesh.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.library.EntryRef
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.CoverArt
import io.github.brainage04.tunemesh.ui.DetailHeader
import io.github.brainage04.tunemesh.ui.DialogRequest
import io.github.brainage04.tunemesh.ui.EmptyMessage
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.LocalPlayerUi
import io.github.brainage04.tunemesh.ui.MediaTile
import io.github.brainage04.tunemesh.ui.MenuItem
import io.github.brainage04.tunemesh.ui.RowContext
import io.github.brainage04.tunemesh.ui.Screen
import io.github.brainage04.tunemesh.ui.SectionTitle
import io.github.brainage04.tunemesh.ui.TrackRow
import io.github.brainage04.tunemesh.ui.UnavailableRow
import io.github.brainage04.tunemesh.ui.songCount
import io.github.brainage04.tunemesh.ui.theme.TextSecondary
import io.github.brainage04.tunemesh.ui.totalLength
import androidx.compose.material3.DropdownMenu

@Composable
private fun playingContext(name: String): Boolean {
    val viewModel = LocalAppViewModel.current
    return viewModel.playingFrom == name && LocalPlayerUi.current.showPause
}

@Composable
private fun Missing(what: String, contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    Column(
        Modifier.fillMaxSize().padding(contentPadding),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        EmptyMessage("$what not found", "It may have been renamed or deleted on another device.")
        TextButton(onClick = { viewModel.back() }) { Text("Go back") }
    }
}

@Composable
fun AlbumScreen(key: String, contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val album = LocalLibrary.current.index.albumsByKey[key] ?: return Missing("Album", contentPadding)
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item {
            DetailHeader(
                title = album.title,
                subtitle = album.artist,
                stats = "Album • ${songCount(album.tracks.size)} • ${totalLength(album.tracks)}",
                seed = album.title,
                onBack = { viewModel.back() },
                onPlay = { viewModel.playOrToggle(album.title, album.tracks) },
                playingThis = playingContext(album.title),
                onShuffle = { viewModel.play(album.title, album.tracks, -1, shuffle = true) },
                playEnabled = album.tracks.isNotEmpty(),
                art = { CoverArt(album.tracks.first(), album.title, it, sizeHint = 232.dp) },
                onSubtitleClick = { viewModel.openArtist(album.artist) },
                actions = { AddAllMenu(album.tracks) },
            )
        }
        itemsIndexed(album.tracks, key = { _, track -> track.path }) { index, track ->
            TrackRow(
                track,
                onPlay = { viewModel.play(album.title, album.tracks, index) },
                context = RowContext(showAlbum = false),
                leading = {
                    Text(
                        (track.trackNumber ?: (index + 1)).toString(),
                        color = TextSecondary,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.widthIn(min = 28.dp),
                    )
                },
            )
        }
    }
}

@Composable
fun ArtistScreen(name: String, contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val artist = LocalLibrary.current.index.artistsByName[name] ?: return Missing("Artist", contentPadding)
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item {
            DetailHeader(
                title = artist.name,
                subtitle = "",
                stats = "${artist.albums.size} ${if (artist.albums.size == 1) "album" else "albums"} • ${songCount(artist.tracks.size)}",
                seed = artist.name,
                onBack = { viewModel.back() },
                onPlay = { viewModel.playOrToggle(artist.name, artist.tracks) },
                playingThis = playingContext(artist.name),
                onShuffle = { viewModel.play(artist.name, artist.tracks, -1, shuffle = true) },
                playEnabled = artist.tracks.isNotEmpty(),
                art = { CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = 232.dp, placeholderIcon = AppIcons.Person) },
                actions = { AddAllMenu(artist.tracks) },
            )
        }
        item { SectionTitle("Albums") }
        item {
            LazyRow(contentPadding = PaddingValues(horizontal = 10.dp)) {
                items(artist.albums, key = { it.key }) { album ->
                    MediaTile(album.title, songCount(album.tracks.size), { viewModel.push(Screen.AlbumDetail(album.key)) }, Modifier.width(150.dp)) {
                        CoverArt(album.tracks.first(), album.title, it, sizeHint = 150.dp)
                    }
                }
            }
        }
        item { SectionTitle("Songs") }
        itemsIndexed(artist.tracks, key = { _, track -> track.path }) { index, track ->
            TrackRow(track, onPlay = { viewModel.play(artist.name, artist.tracks, index) }, context = RowContext(showArtist = false))
        }
    }
}

@Composable
fun PlaylistScreen(name: String, contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    val playlist = library.playlist(name) ?: return Missing("Playlist", contentPadding)
    // Only the open playlist's entries are held, and only while its screen is shown.
    DisposableEffect(name) {
        viewModel.repository.openPlaylist(name)
        onDispose { viewModel.repository.closePlaylist(name) }
    }
    val contents = library.openPlaylist?.takeIf { it.name == name }
    val entries = contents?.entries ?: emptyList()
    val resolved = remember(entries, library.index) { entries.map { entry -> entry.path?.let { library.index.byPath[it] } } }
    val playable = remember(resolved) { resolved.filterNotNull() }
    var editing by remember(name) { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item {
            DetailHeader(
                title = playlist.name,
                subtitle = if (playlist.isLikedSongs) "Your liked songs" else "Playlist",
                stats = if (playlist.problem != null) "Too large to open" else buildString {
                    append(songCount(contents?.entries?.size ?: playlist.size))
                    if (contents != null && playable.size != entries.size) append(" • ${entries.size - playable.size} unavailable")
                    if (playable.isNotEmpty()) append(" • ${totalLength(playable)}")
                },
                seed = playlist.name,
                onBack = { viewModel.back() },
                onPlay = { viewModel.playOrToggle(playlist.name, playable) },
                playingThis = playingContext(playlist.name),
                onShuffle = { viewModel.play(playlist.name, playable, -1, shuffle = true) },
                playEnabled = playable.isNotEmpty(),
                art = { PlaylistArt(playlist, it, 232.dp) },
                actions = {
                    if (entries.size > 1) {
                        TextButton(onClick = { editing = !editing }) { Text(if (editing) "Done" else "Edit order") }
                    }
                    PlaylistMenu(playlist.name, playlist.isLikedSongs, playable)
                },
            )
        }
        playlist.problem?.let { problem ->
            item { EmptyMessage("This playlist cannot be opened", "$problem. It is left unchanged.") }
        }
        if (playlist.problem == null && contents != null && entries.isEmpty()) {
            item {
                EmptyMessage(
                    if (playlist.isLikedSongs) "Songs you like appear here" else "This playlist is empty",
                    if (playlist.isLikedSongs) "Tap the heart on any song." else "Add songs with \"Add to playlist\" from any song's menu.",
                )
            }
        }
        itemsIndexed(entries) { index, entry ->
            val track = resolved[index]
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) {
                    if (track != null) {
                        TrackRow(
                            track,
                            onPlay = { viewModel.play(playlist.name, playable, resolved.take(index).count { it != null }) },
                            context = RowContext(playlistName = playlist.name, index = index),
                        )
                    } else {
                        UnavailableRow(entry.raw, onRemove = { viewModel.repository.removeFromPlaylist(playlist.name, EntryRef(index, entry.key)) })
                    }
                }
                if (editing) {
                    IconButton(
                        onClick = { viewModel.repository.movePlaylistEntry(playlist.name, EntryRef(index, entry.key), EntryRef(index - 1, entries[index - 1].key)) },
                        enabled = index > 0,
                    ) {
                        Icon(AppIcons.Up, "Move up")
                    }
                    IconButton(
                        onClick = { viewModel.repository.movePlaylistEntry(playlist.name, EntryRef(index, entry.key), EntryRef(index + 1, entries[index + 1].key)) },
                        enabled = index < entries.lastIndex,
                    ) {
                        Icon(AppIcons.DownArrow, "Move down")
                    }
                }
            }
        }
        item { Spacer(Modifier.height(16.dp)) }
    }
}

@Composable
private fun PlaylistMenu(name: String, likedSongs: Boolean, tracks: List<Track>) {
    val viewModel = LocalAppViewModel.current
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(AppIcons.More, "Playlist options") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            if (tracks.isNotEmpty()) {
                MenuItem("Add to queue", AppIcons.Queue) { viewModel.addToQueue(tracks); open = false }
                MenuItem("Add to other playlist", AppIcons.PlaylistAdd) { viewModel.dialog = DialogRequest.AddToPlaylist(tracks); open = false }
            }
            if (!likedSongs) {
                MenuItem("Rename", AppIcons.Edit) { viewModel.dialog = DialogRequest.RenamePlaylist(name); open = false }
                MenuItem("Delete playlist", AppIcons.Delete, color = MaterialTheme.colorScheme.error) {
                    viewModel.dialog = DialogRequest.DeletePlaylist(name); open = false
                }
            }
        }
    }
}

@Composable
private fun AddAllMenu(tracks: List<Track>) {
    val viewModel = LocalAppViewModel.current
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(AppIcons.More, "More options") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            MenuItem("Play next", AppIcons.Queue) { viewModel.playNext(tracks); open = false }
            MenuItem("Add to queue", AppIcons.Queue) { viewModel.addToQueue(tracks); open = false }
            MenuItem("Add to playlist", AppIcons.PlaylistAdd) { viewModel.dialog = DialogRequest.AddToPlaylist(tracks); open = false }
        }
    }
}
