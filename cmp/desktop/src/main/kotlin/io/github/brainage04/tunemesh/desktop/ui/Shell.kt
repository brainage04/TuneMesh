package io.github.brainage04.tunemesh.desktop.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.SurfaceHigh
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

/** Window content: sidebar, page with back/forward and search, optional queue panel, now-playing bar. */
@Composable
fun DesktopApp(model: AppModel) {
    val library by model.repository.state.collectAsState()
    val playerUi by model.player.ui.collectAsState()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(Unit) { model.repository.messages.collect { snackbar.showSnackbar(it) } }
    CompositionLocalProvider(LocalAppModel provides model, LocalLibrary provides library, LocalPlayerUi provides playerUi) {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            Column(Modifier.fillMaxSize()) {
                Row(Modifier.weight(1f).padding(8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Sidebar(Modifier.width(280.dp).fillMaxHeight())
                    Column(Modifier.weight(1f).fillMaxHeight().clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.background)) {
                        TopBar()
                        Box(Modifier.weight(1f)) {
                            when (val route = model.route) {
                                Route.Home -> HomeScreen()
                                Route.Search -> SearchScreen()
                                is Route.Library -> LibraryScreen(route.tab)
                                is Route.AlbumDetail -> AlbumScreen(route.key)
                                is Route.ArtistDetail -> ArtistScreen(route.name)
                                is Route.PlaylistDetail -> PlaylistScreen(route.name)
                                Route.NowPlaying -> NowPlayingScreen()
                            }
                        }
                    }
                    if (model.queueOpen) QueuePanel(Modifier.width(340.dp).fillMaxHeight().clip(RoundedCornerShape(8.dp)))
                }
                NowPlayingBar()
            }
            SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(bottom = 100.dp))
        }
        model.dialog?.let { Dialogs(it, model) }
    }
}

@Composable
private fun TopBar() {
    val model = LocalAppModel.current
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = { model.goBack() }, enabled = model.canGoBack) { Icon(AppIcons.Back, "Go back") }
        IconButton(onClick = { model.goForward() }, enabled = model.canGoForward) { Icon(DesktopIcons.Forward, "Go forward") }
        Spacer(Modifier.width(12.dp))
        Row(
            Modifier.width(380.dp).height(44.dp).clip(RoundedCornerShape(22.dp)).background(SurfaceHigh).padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(AppIcons.Search, null, tint = TextSecondary, modifier = Modifier.size(20.dp))
            Spacer(Modifier.width(10.dp))
            Box(Modifier.weight(1f)) {
                if (model.query.isEmpty()) Text("What do you want to play?", color = TextSecondary, style = MaterialTheme.typography.bodyLarge)
                BasicTextField(
                    value = model.query,
                    onValueChange = { model.search(it) },
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = Color.White),
                    cursorBrush = SolidColor(Color.White),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            if (model.query.isNotEmpty()) {
                Icon(AppIcons.Close, "Clear", tint = TextSecondary, modifier = Modifier.size(18.dp).clickable { model.query = "" })
            }
        }
    }
}

@Composable
private fun Sidebar(modifier: Modifier) {
    val model = LocalAppModel.current
    val library = LocalLibrary.current
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.background).padding(vertical = 8.dp)) {
            NavItem(AppIcons.Home, "Home", model.route == Route.Home) { model.navigate(Route.Home) }
            NavItem(AppIcons.Search, "Search", model.route == Route.Search) { model.navigate(Route.Search) }
        }
        Column(Modifier.fillMaxWidth().weight(1f).clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.background)) {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 8.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(AppIcons.Library, null, tint = TextSecondary, modifier = Modifier.size(22.dp))
                Spacer(Modifier.width(12.dp))
                Text("Your Library", fontWeight = FontWeight.Bold, color = TextSecondary, modifier = Modifier.weight(1f))
                IconButton(onClick = { model.dialog = DialogRequest.CreatePlaylist(emptyList()) }) { Icon(AppIcons.Add, "Create playlist", tint = TextSecondary) }
            }
            val playingContext = LocalPlayerUi.current.context
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 8.dp)) {
                item {
                    val liked = library.playlist(LibraryFormat.LIKED_SONGS)
                    SidebarRow(LibraryFormat.LIKED_SONGS, "Playlist • ${songCount(liked?.entries?.size ?: 0)}", Route.PlaylistDetail(LibraryFormat.LIKED_SONGS), playingContext == LibraryFormat.LIKED_SONGS) {
                        LikedSongsArt(it)
                    }
                }
                item {
                    SidebarRow("Albums", "${library.index.albums.size} albums", Route.Library(LibraryTab.Albums), false) { IconTile(AppIcons.Album, it) }
                }
                item {
                    SidebarRow("Artists", "${library.index.artists.size} artists", Route.Library(LibraryTab.Artists), false) { IconTile(AppIcons.Person, it) }
                }
                item {
                    SidebarRow("Songs", songCount(library.index.tracks.size), Route.Library(LibraryTab.Songs), false) { IconTile(AppIcons.MusicNote, it) }
                }
                items(library.playlists.filter { !it.isLikedSongs }, key = { it.name }) { playlist ->
                    SidebarRow(playlist.name, "Playlist • ${songCount(playlist.entries.size)}", Route.PlaylistDetail(playlist.name), playingContext == playlist.name) {
                        PlaylistArt(playlist, it, 48.dp)
                    }
                }
            }
        }
    }
}

@Composable
private fun NavItem(icon: ImageVector, label: String, active: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 20.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, tint = if (active) Color.White else TextSecondary)
        Spacer(Modifier.width(16.dp))
        Text(label, fontWeight = FontWeight.Bold, color = if (active) Color.White else TextSecondary)
    }
}

@Composable
private fun IconTile(icon: ImageVector, modifier: Modifier) {
    Box(modifier.clip(RoundedCornerShape(4.dp)).background(SurfaceHigh), contentAlignment = Alignment.Center) { Icon(icon, null, tint = TextSecondary) }
}

@Composable
private fun SidebarRow(title: String, subtitle: String, route: Route, playing: Boolean, art: @Composable (Modifier) -> Unit) {
    val model = LocalAppModel.current
    val active = model.route == route
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(if (active) SurfaceHigh else Color.Transparent)
            .clickable { model.navigate(route) }.padding(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        art(Modifier.size(48.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = if (playing) Accent else Color.White, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(subtitle, color = TextSecondary, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun Dialogs(request: DialogRequest, model: AppModel) {
    val dismiss = { model.dialog = null }
    when (request) {
        is DialogRequest.AddToPlaylist -> AddToPlaylistDialog(request.tracks, model, dismiss)
        is DialogRequest.CreatePlaylist -> NameDialog("Give your playlist a name", "", "Create", dismiss) { name ->
            model.repository.createPlaylist(name, request.tracks); dismiss()
        }
        is DialogRequest.RenamePlaylist -> NameDialog("Rename playlist", request.name, "Rename", dismiss) { name ->
            model.renamePlaylist(request.name, name); dismiss()
        }
        is DialogRequest.DeletePlaylist -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text("Delete \u201c${request.name}\u201d?") },
            text = { Text("The playlist file is removed on every device. The songs stay in your library.") },
            confirmButton = { TextButton(onClick = { model.deletePlaylist(request.name); dismiss() }) { Text("Delete", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = dismiss) { Text("Cancel") } },
        )
    }
}

@Composable
private fun AddToPlaylistDialog(tracks: List<Track>, model: AppModel, onDismiss: () -> Unit) {
    val playlists = LocalLibrary.current.playlists.filter { !it.isLikedSongs }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (tracks.size == 1) "Add to playlist" else "Add ${tracks.size} songs to playlist") },
        text = {
            LazyColumn(Modifier.heightIn(max = 360.dp)) {
                item {
                    Row(Modifier.fillMaxWidth().clickable { model.dialog = DialogRequest.CreatePlaylist(tracks) }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconTile(AppIcons.Add, Modifier.size(48.dp))
                        Spacer(Modifier.width(12.dp))
                        Text("New playlist", fontWeight = FontWeight.SemiBold)
                    }
                }
                items(playlists, key = { it.name }) { playlist ->
                    Row(
                        Modifier.fillMaxWidth().clickable { model.repository.addToPlaylist(playlist.name, tracks); onDismiss() }.padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        PlaylistArt(playlist, Modifier.size(48.dp), 48.dp)
                        Spacer(Modifier.width(12.dp))
                        Column {
                            Text(playlist.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(songCount(playlist.entries.size), color = TextSecondary, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun NameDialog(title: String, initial: String, confirm: String, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var name by remember { mutableStateOf(initial) }
    val reserved = name.trim().equals(LibraryFormat.LIKED_SONGS, ignoreCase = true)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(value = name, onValueChange = { name = it }, singleLine = true, label = { Text("Name") })
                if (reserved) Text("\u201c${LibraryFormat.LIKED_SONGS}\u201d is reserved", color = MaterialTheme.colorScheme.error)
            }
        },
        confirmButton = { TextButton(onClick = { onConfirm(name.trim()) }, enabled = name.isNotBlank() && !reserved) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
