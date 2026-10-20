package io.github.brainage04.tunemesh.ui.screens

import androidx.compose.foundation.background
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
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.CoverArt
import io.github.brainage04.tunemesh.ui.EmptyMessage
import io.github.brainage04.tunemesh.ui.LikedSongsArt
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.MediaTile
import io.github.brainage04.tunemesh.ui.Screen
import io.github.brainage04.tunemesh.ui.ScreenTitle
import io.github.brainage04.tunemesh.ui.SectionTitle
import io.github.brainage04.tunemesh.ui.playlistSubtitle
import io.github.brainage04.tunemesh.ui.theme.SurfaceHigh
import java.time.LocalTime

@Composable
fun HomeScreen(contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    val index = library.index
    val greeting = when (LocalTime.now().hour) {
        in 5..11 -> "Good morning"
        in 12..17 -> "Good afternoon"
        else -> "Good evening"
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item {
            ScreenTitle(greeting) {
                IconButton(onClick = { viewModel.push(Screen.Settings) }) { Icon(AppIcons.Settings, "Settings") }
            }
        }
        if (index.tracks.isEmpty() && library.playlists.isEmpty()) {
            item {
                EmptyMessage(
                    if (library.scanning || !library.loaded) "Scanning your library…" else "Your library is empty",
                    "Songs in ${library.root} show up here. Request downloads from the Downloads tab.",
                )
            }
            return@LazyColumn
        }
        val shortcuts = library.playlists.take(6)
        if (shortcuts.isNotEmpty()) {
            item {
                Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    shortcuts.chunked(2).forEach { pair ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            pair.forEach { PlaylistShortcut(it, Modifier.weight(1f)) }
                            if (pair.size == 1) Spacer(Modifier.weight(1f))
                        }
                    }
                }
            }
        }
        if (index.albums.isNotEmpty()) {
            item { SectionTitle("Recently added") }
            item {
                LazyRow(contentPadding = PaddingValues(horizontal = 10.dp)) {
                    // No item keys: a newly synced album must appear at the start instead of the row keeping its scroll anchor.
                    items(index.recentlyAdded.take(20)) { album ->
                        MediaTile(album.title, album.artist, { viewModel.push(Screen.AlbumDetail(album.key)) }, Modifier.width(160.dp)) {
                            CoverArt(album.tracks.first(), album.title, it, sizeHint = 160.dp)
                        }
                    }
                }
            }
        }
        if (library.playlists.isNotEmpty()) {
            item { SectionTitle("Your playlists") }
            item {
                LazyRow(contentPadding = PaddingValues(horizontal = 10.dp)) {
                    items(library.playlists, key = { it.name }) { playlist ->
                        MediaTile(playlist.name, playlistSubtitle(playlist), { viewModel.push(Screen.PlaylistDetail(playlist.name)) }, Modifier.width(160.dp)) {
                            PlaylistArt(playlist, it, 160.dp)
                        }
                    }
                }
            }
        }
        if (index.artists.isNotEmpty()) {
            item { SectionTitle("Artists") }
            item {
                LazyRow(contentPadding = PaddingValues(horizontal = 10.dp)) {
                    items(index.artists.sortedByDescending { artist -> artist.albums.maxOf { it.addedMs } }.take(20), key = { it.name }) { artist ->
                        MediaTile(artist.name, "Artist", { viewModel.openArtist(artist.name) }, Modifier.width(140.dp), circle = true) {
                            CoverArt(artist.tracks.first(), artist.name, it, shape = CircleShape, sizeHint = 140.dp, placeholderIcon = AppIcons.Person)
                        }
                    }
                }
            }
        }
        item { Spacer(Modifier.height(16.dp)) }
    }
}

@Composable
private fun PlaylistShortcut(playlist: Playlist, modifier: Modifier) {
    val viewModel = LocalAppViewModel.current
    Row(
        modifier
            .height(56.dp)
            .clip(RoundedCornerShape(4.dp))
            .background(SurfaceHigh)
            .clickable { viewModel.push(Screen.PlaylistDetail(playlist.name)) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlaylistArt(playlist, Modifier.size(56.dp), 56.dp, RoundedCornerShape(topStart = 4.dp, bottomStart = 4.dp))
        Text(
            playlist.name,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Bold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 10.dp).fillMaxWidth(),
        )
    }
}

/** Liked Songs gets its heart tile; other playlists use the art of their first available track. */
@Composable
fun PlaylistArt(
    playlist: Playlist,
    modifier: Modifier,
    sizeHint: androidx.compose.ui.unit.Dp,
    shape: androidx.compose.ui.graphics.Shape = RoundedCornerShape(4.dp),
) {
    if (playlist.isLikedSongs) {
        LikedSongsArt(modifier, shape)
        return
    }
    val library = LocalLibrary.current
    val first = playlist.artPaths.firstNotNullOfOrNull { library.index.byPath[it] }
    CoverArt(first, playlist.name, modifier, shape = shape, sizeHint = sizeHint, placeholderIcon = AppIcons.Queue)
}
