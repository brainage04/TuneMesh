package io.github.brainage04.tunemesh.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.screens.AlbumScreen
import io.github.brainage04.tunemesh.ui.screens.ArtistScreen
import io.github.brainage04.tunemesh.ui.screens.DownloadsScreen
import io.github.brainage04.tunemesh.ui.screens.HomeScreen
import io.github.brainage04.tunemesh.ui.screens.LibraryScreen
import io.github.brainage04.tunemesh.ui.screens.MiniPlayer
import io.github.brainage04.tunemesh.ui.screens.NowPlayingScreen
import io.github.brainage04.tunemesh.ui.screens.OnboardingScreen
import io.github.brainage04.tunemesh.ui.screens.PlaylistArt
import io.github.brainage04.tunemesh.ui.screens.PlaylistScreen
import io.github.brainage04.tunemesh.ui.screens.QueueScreen
import io.github.brainage04.tunemesh.ui.screens.SearchScreen
import io.github.brainage04.tunemesh.ui.screens.SettingsScreen
import io.github.brainage04.tunemesh.ui.screens.SyncthingSetupScreen
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

@Composable
fun App(viewModel: AppViewModel, finish: () -> Unit) {
    val library by viewModel.repository.state.collectAsStateWithLifecycle()
    val playerUi by viewModel.player.ui.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(Unit) {
        viewModel.repository.messages.collect { snackbar.showSnackbar(it) }
    }
    CompositionLocalProvider(
        LocalAppViewModel provides viewModel,
        LocalLibrary provides library,
        LocalPlayerUi provides playerUi,
    ) {
        if (!library.hasAccess) {
            OnboardingScreen()
            return@CompositionLocalProvider
        }
        if (viewModel.syncthingSetupOpen) {
            BackHandler { viewModel.closeSyncthingSetup() }
            SyncthingSetupScreen(onDone = { viewModel.closeSyncthingSetup() })
            return@CompositionLocalProvider
        }
        BackHandler { if (!viewModel.back()) finish() }
        Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            Column(Modifier.fillMaxSize()) {
                Box(Modifier.weight(1f)) {
                    val padding = PaddingValues(bottom = if (playerUi.current != null) 72.dp else 8.dp)
                    when (val screen = viewModel.screen) {
                        Screen.Root -> when (viewModel.tab) {
                            Tab.Home -> HomeScreen(padding)
                            Tab.Search -> SearchScreen(padding)
                            Tab.Library -> LibraryScreen(padding)
                            Tab.Downloads -> DownloadsScreen(padding)
                        }
                        is Screen.AlbumDetail -> AlbumScreen(screen.key, padding)
                        is Screen.ArtistDetail -> ArtistScreen(screen.name, padding)
                        is Screen.PlaylistDetail -> PlaylistScreen(screen.name, padding)
                        Screen.Settings -> SettingsScreen(padding)
                    }
                    MiniPlayer(Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp))
                }
                BottomBar(viewModel)
            }
            SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(bottom = 140.dp))
            AnimatedVisibility(viewModel.playerExpanded, enter = slideInVertically { it }, exit = slideOutVertically { it }) {
                NowPlayingScreen()
            }
            AnimatedVisibility(viewModel.queueOpen, enter = slideInVertically { it }, exit = slideOutVertically { it }) {
                QueueScreen()
            }
        }
        viewModel.dialog?.let { Dialogs(it, viewModel) }
    }
}

@Composable
private fun BottomBar(viewModel: AppViewModel) {
    NavigationBar(containerColor = Color(0xF0101010), tonalElevation = 0.dp) {
        Tab.entries.forEach { tab ->
            NavigationBarItem(
                selected = viewModel.tab == tab,
                onClick = { viewModel.select(tab) },
                icon = {
                    Icon(
                        when (tab) {
                            Tab.Home -> AppIcons.Home
                            Tab.Search -> AppIcons.Search
                            Tab.Library -> AppIcons.Library
                            Tab.Downloads -> AppIcons.Download
                        },
                        null,
                    )
                },
                label = { Text(tab.label) },
                colors = NavigationBarItemDefaults.colors(
                    selectedIconColor = Color.White,
                    selectedTextColor = Color.White,
                    unselectedIconColor = TextSecondary,
                    unselectedTextColor = TextSecondary,
                    indicatorColor = Color.Transparent,
                ),
            )
        }
    }
}

@Composable
private fun Dialogs(request: DialogRequest, viewModel: AppViewModel) {
    val dismiss = { viewModel.dialog = null }
    when (request) {
        is DialogRequest.AddToPlaylist -> AddToPlaylistDialog(request.tracks, viewModel, dismiss)
        is DialogRequest.CreatePlaylist -> NameDialog(
            title = "Give your playlist a name",
            initial = "",
            confirm = "Create",
            onDismiss = dismiss,
        ) { name -> viewModel.repository.createPlaylist(name, request.tracks); dismiss() }
        is DialogRequest.RenamePlaylist -> NameDialog(
            title = "Rename playlist",
            initial = request.name,
            confirm = "Rename",
            onDismiss = dismiss,
        ) { name -> viewModel.renamePlaylist(request.name, name); dismiss() }
        is DialogRequest.DeletePlaylist -> ConfirmDialog(
            title = "Delete \"${request.name}\"?",
            body = "The playlist file is removed on every device. The songs stay in your library.",
            confirm = "Delete",
            onDismiss = dismiss,
        ) { viewModel.deletePlaylist(request.name); dismiss() }
        is DialogRequest.DeleteTrack -> ConfirmDialog(
            title = "Delete \"${request.track.title}\"?",
            body = "This deletes ${request.track.path} from the library on every device that syncs it. " +
                "Playlists keep it as an unavailable entry. This cannot be undone.",
            confirm = "Delete everywhere",
            onDismiss = dismiss,
        ) { viewModel.deleteTrack(request.track); dismiss() }
    }
}

@Composable
private fun AddToPlaylistDialog(tracks: List<Track>, viewModel: AppViewModel, onDismiss: () -> Unit) {
    val library = viewModel.repository.state.collectAsStateWithLifecycle().value
    val playlists = library.playlists.filter { !it.isLikedSongs }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (tracks.size == 1) "Add to playlist" else "Add ${tracks.size} songs to playlist") },
        text = {
            LazyColumn(Modifier.heightIn(max = 360.dp)) {
                item {
                    Row(
                        Modifier.fillMaxWidth().clickable { viewModel.dialog = DialogRequest.CreatePlaylist(tracks) }.padding(vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(Modifier.size(48.dp).background(Color(0xFF333333)), contentAlignment = Alignment.Center) { Icon(AppIcons.Add, null) }
                        Spacer(Modifier.width(12.dp))
                        Text("New playlist", fontWeight = FontWeight.SemiBold)
                    }
                }
                items(playlists, key = { it.name }) { playlist ->
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            viewModel.repository.addToPlaylist(playlist.name, tracks)
                            onDismiss()
                        }.padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        PlaylistArt(playlist, Modifier.size(48.dp), 48.dp)
                        Spacer(Modifier.width(12.dp))
                        Column {
                            Text(playlist.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(songCount(playlist.size), color = TextSecondary, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        textContentColor = MaterialTheme.colorScheme.onSurface,
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
                if (reserved) Text("\"${LibraryFormat.LIKED_SONGS}\" is reserved", color = MaterialTheme.colorScheme.error)
            }
        },
        confirmButton = { TextButton(onClick = { onConfirm(name.trim()) }, enabled = name.isNotBlank() && !reserved) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun ConfirmDialog(title: String, body: String, confirm: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(body) },
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirm, color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
