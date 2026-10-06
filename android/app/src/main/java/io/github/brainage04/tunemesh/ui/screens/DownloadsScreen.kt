package io.github.brainage04.tunemesh.ui.screens

import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.data.LibraryRepository
import io.github.brainage04.tunemesh.library.DownloadItem
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.ClearFocusWhenImeHidden
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.ScreenTitle
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.SurfaceHigh
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

@Composable
fun DownloadsScreen(contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    var input by rememberSaveable { mutableStateOf("") }
    var playlist by rememberSaveable { mutableStateOf("") }
    var fromSource by rememberSaveable { mutableStateOf(false) }
    var submitting by remember { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item { ScreenTitle("Downloads") }
        item {
            RequestForm(
                repository = viewModel.repository,
                playlists = library.playlists,
                input = input,
                onInput = { input = it },
                playlist = playlist,
                onPlaylist = { playlist = it },
                fromSource = fromSource,
                onFromSource = { fromSource = it },
                submitting = submitting,
                onSubmit = {
                    submitting = true
                    viewModel.repository.submitDownload(input, playlist.ifBlank { null }, fromSource) { result ->
                        submitting = false
                        if (result.isSuccess) {
                            input = ""
                            fromSource = false
                        }
                    }
                },
            )
        }
        item {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 24.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Requests", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
                if (library.downloads.any { it.isFinal }) {
                    TextButton(onClick = { viewModel.repository.clearFinishedDownloads() }) { Text("Clear finished") }
                }
            }
        }
        if (library.downloads.isEmpty()) {
            item {
                Text(
                    "Nothing requested yet. Paste a YouTube, YouTube Music or Spotify link, or type what you want to hear. " +
                        "You can also share a link to TuneMesh from those apps.",
                    color = TextSecondary,
                    modifier = Modifier.padding(16.dp),
                )
            }
        }
        items(library.downloads, key = { it.id }) { item ->
            DownloadCard(item, onCancel = { viewModel.repository.cancelDownload(item.id) })
        }
    }
}

/** Shared by the Downloads tab and the share sheet. */
@Composable
fun RequestForm(
    repository: LibraryRepository,
    playlists: List<Playlist>,
    input: String,
    onInput: (String) -> Unit,
    playlist: String,
    onPlaylist: (String) -> Unit,
    fromSource: Boolean,
    onFromSource: (Boolean) -> Unit,
    submitting: Boolean,
    onSubmit: () -> Unit,
    showInput: Boolean = true,
) {
    var pickerOpen by remember { mutableStateOf(false) }
    ClearFocusWhenImeHidden()
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (showInput) {
            OutlinedTextField(
                value = input,
                onValueChange = onInput,
                label = { Text("Link or search") },
                placeholder = { Text("https://music.youtube.com/… or artist – song") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        Box {
            OutlinedTextField(
                value = playlist,
                onValueChange = onPlaylist,
                label = { Text("Add to playlist (optional)") },
                singleLine = true,
                trailingIcon = {
                    IconButton(onClick = { pickerOpen = true }) { Icon(AppIcons.Down, "Choose playlist") }
                },
                modifier = Modifier.fillMaxWidth(),
            )
            DropdownMenu(expanded = pickerOpen, onDismissRequest = { pickerOpen = false }) {
                DropdownMenuItem(text = { Text("No playlist") }, onClick = { onPlaylist(""); pickerOpen = false })
                playlists.forEach { option ->
                    DropdownMenuItem(text = { Text(option.name) }, onClick = { onPlaylist(option.name); pickerOpen = false })
                }
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Also create a playlist from this link")
                Text(
                    if (playlist.isBlank()) "For playlist and album links: named after the source" else "Not used when a playlist is chosen",
                    color = TextSecondary,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Switch(checked = fromSource && playlist.isBlank(), onCheckedChange = onFromSource, enabled = playlist.isBlank())
        }
        Button(
            onClick = onSubmit,
            enabled = input.isNotBlank() && !submitting && repository.state.value.hasAccess,
            colors = ButtonDefaults.buttonColors(containerColor = Accent, contentColor = Color.Black),
            modifier = Modifier.fillMaxWidth().height(48.dp),
        ) {
            Text("Request download", fontWeight = FontWeight.Bold)
        }
    }
}

@Composable
private fun DownloadCard(item: DownloadItem, onCancel: () -> Unit) {
    val status = item.status
    val (label, color) = when (status?.state) {
        null -> "Waiting for the downloader" to TextSecondary
        "queued" -> "Queued" to TextSecondary
        "downloading" -> (status.progress?.let { "Downloading ${(it * 100).toInt()}%" } ?: "Downloading") to Accent
        "done" -> "Done" + (if (status.tracks.isNotEmpty()) " • ${status.tracks.size} ${if (status.tracks.size == 1) "song" else "songs"}" else "") to Accent
        "failed" -> "Failed" to MaterialTheme.colorScheme.error
        "cancelled" -> "Cancelled" to TextSecondary
        else -> status.state to TextSecondary
    }
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 6.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(SurfaceHigh)
            .padding(14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(status?.title ?: item.input, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (status?.title != null) {
                    Text(item.input, color = TextSecondary, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Spacer(Modifier.height(4.dp))
                Text(label, color = color, style = MaterialTheme.typography.bodyMedium)
                val target = item.request?.playlist
                if (target != null) Text("→ $target", color = TextSecondary, style = MaterialTheme.typography.bodySmall)
            }
            if (item.canCancel) {
                Spacer(Modifier.width(8.dp))
                OutlinedButton(onClick = onCancel) { Text("Cancel") }
            }
        }
        if (status?.state == "downloading") {
            Spacer(Modifier.height(10.dp))
            val progress = status.progress
            if (progress != null) {
                LinearProgressIndicator(
                    progress = { progress.toFloat() },
                    modifier = Modifier.fillMaxWidth(),
                    color = Accent,
                    drawStopIndicator = {},
                    gapSize = 0.dp,
                )
            } else {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth(), color = Accent)
            }
        }
        status?.message?.takeIf { it.isNotBlank() }?.let { message ->
            Spacer(Modifier.height(6.dp))
            Text(message, color = TextSecondary, style = MaterialTheme.typography.bodySmall)
        }
    }
}
