package io.github.brainage04.tunemesh.ui.screens

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
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.library.DownloadInputs
import io.github.brainage04.tunemesh.library.DownloadItem
import io.github.brainage04.tunemesh.library.Downloader
import io.github.brainage04.tunemesh.library.Limits
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.ClearFocusWhenImeHidden
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.ScreenTitle
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.SurfaceHigh
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

private val WarningColor = Color(0xFFF5C451)

@Composable
fun DownloadsScreen(contentPadding: PaddingValues) {
    val viewModel = LocalAppViewModel.current
    val library = LocalLibrary.current
    var input by rememberSaveable { mutableStateOf("") }
    var playlist by rememberSaveable { mutableStateOf("") }
    var saveAsPlaylists by rememberSaveable { mutableStateOf<Boolean?>(null) }
    var submitting by remember { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = contentPadding) {
        item { ScreenTitle("Downloads") }
        item { DownloaderBanner(library.downloader, Modifier.padding(start = 16.dp, end = 16.dp, bottom = 16.dp)) }
        item {
            RequestForm(
                hasAccess = library.hasAccess,
                playlists = library.playlists,
                input = input,
                onInput = { input = it },
                playlist = playlist,
                onPlaylist = { playlist = it },
                saveAsPlaylists = saveAsPlaylists,
                onSaveAsPlaylists = { saveAsPlaylists = it },
                submitting = submitting,
                onSubmit = {
                    submitting = true
                    viewModel.repository.submitDownloads(
                        input,
                        playlist.ifBlank { null },
                        effectiveSaveAsPlaylists(input, saveAsPlaylists),
                    ) { result ->
                        submitting = false
                        if (result.isSuccess) {
                            input = ""
                            saveAsPlaylists = null
                        }
                    }
                },
            )
        }
        item {
            Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 24.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Requests", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
                if (library.downloads.items.any { it.isFinal }) {
                    TextButton(onClick = { viewModel.repository.clearFinishedDownloads() }) { Text("Clear finished") }
                }
            }
        }
        library.downloadsError?.let { error ->
            item { Text(error, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) }
        }
        if (library.downloads.items.isEmpty()) {
            item {
                Text(
                    "Nothing requested yet. Paste YouTube, YouTube Music or Spotify links (one per line), or type what you " +
                        "want to hear. You can also share a link to TuneMesh from those apps.",
                    color = TextSecondary,
                    modifier = Modifier.padding(16.dp),
                )
            }
        }
        items(library.downloads.items, key = { it.id }) { item ->
            DownloadCard(item, onCancel = { viewModel.repository.cancelDownload(item.id) })
        }
        if (library.downloads.older > 0) {
            item {
                Text(
                    "and ${library.downloads.older} older ${if (library.downloads.older == 1) "request" else "requests"}",
                    color = TextSecondary,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
    }
}

/** "Save playlists" as the user set it, or its default for what is pasted. */
fun effectiveSaveAsPlaylists(input: String, choice: Boolean?): Boolean =
    choice ?: DownloadInputs.saveAsPlaylistsByDefault(DownloadInputs.lines(input))

/** Which device downloads requests, from `.tunemesh/worker.json`. Shared by the Downloads tab and the share sheet. */
@Composable
fun DownloaderBanner(downloader: Downloader, modifier: Modifier = Modifier) {
    val active = downloader is Downloader.Active
    Row(
        modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(SurfaceHigh).padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(AppIcons.Computer, null, tint = if (active) Accent else TextSecondary, modifier = Modifier.size(22.dp))
        Spacer(Modifier.width(12.dp))
        Column {
            Text(downloader.description, fontWeight = FontWeight.SemiBold)
            if (!active) {
                Text(
                    "Requests wait in the library until a computer running the TuneMesh desktop app or download worker " + "picks them up.",
                    color = TextSecondary,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

/** Shared by the Downloads tab and the share sheet. [saveAsPlaylists] null means "the default for [input]". */
@Composable
fun RequestForm(
    hasAccess: Boolean,
    playlists: List<Playlist>,
    input: String,
    onInput: (String) -> Unit,
    playlist: String,
    onPlaylist: (String) -> Unit,
    saveAsPlaylists: Boolean?,
    onSaveAsPlaylists: (Boolean) -> Unit,
    submitting: Boolean,
    onSubmit: () -> Unit,
    showInput: Boolean = true,
) {
    var pickerOpen by remember { mutableStateOf(false) }
    val lines = DownloadInputs.lines(input)
    val save = effectiveSaveAsPlaylists(input, saveAsPlaylists)
    ClearFocusWhenImeHidden()
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (showInput) {
            OutlinedTextField(
                value = input,
                onValueChange = onInput,
                label = { Text("Links or searches, one per line") },
                placeholder = { Text("https://music.youtube.com/… or artist – song") },
                minLines = 2,
                maxLines = 8,
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
                DropdownMenuItem(
                    text = { Text("No playlist") },
                    onClick = {
                        onPlaylist("")
                        pickerOpen = false
                    },
                )
                playlists.forEach { option ->
                    DropdownMenuItem(
                        text = { Text(option.name) },
                        onClick = {
                            onPlaylist(option.name)
                            pickerOpen = false
                        },
                    )
                }
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Save playlists as TuneMesh playlists")
                Text(
                    if (playlist.isBlank()) "Playlist and album links get a playlist named after the source"
                    else "Not used when a playlist is chosen",
                    color = TextSecondary,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Switch(checked = save && playlist.isBlank(), onCheckedChange = onSaveAsPlaylists, enabled = playlist.isBlank())
        }
        Button(
            onClick = onSubmit,
            enabled = lines.isNotEmpty() && !submitting && hasAccess,
            colors = ButtonDefaults.buttonColors(containerColor = Accent, contentColor = Color.Black),
            modifier = Modifier.fillMaxWidth().height(48.dp),
        ) {
            Text(if (lines.size > 1) "Request ${lines.size} downloads" else "Request download", fontWeight = FontWeight.Bold)
        }
    }
}

@Composable
private fun DownloadCard(item: DownloadItem, onCancel: () -> Unit) {
    val shown = item.shown
    val count = item.itemsTotal?.takeIf { it > 1 }?.let { total -> "${(item.itemsDone ?: 0).coerceIn(0, total)} of $total" }
    val (label, color) =
        when (val state = item.state) {
            null -> "Waiting for the downloader" to TextSecondary
            "queued" -> "Queued" to TextSecondary
            "downloading" ->
                listOfNotNull(
                        "Downloading",
                        count,
                        item.progress?.let { "${(it * 100).toInt()}%" },
                    )
                    .joinToString(" • ") to Accent
            "done" ->
                "Done" + (if (item.trackCount > 0) " • ${item.trackCount} ${if (item.trackCount == 1) "song" else "songs"}" else "") to
                    Accent
            "failed" -> "Failed" + count?.let { " • $it done" }.orEmpty() to MaterialTheme.colorScheme.error
            "cancelled" -> "Cancelled" + count?.let { " • $it done" }.orEmpty() to TextSecondary
            else -> Limits.shownLine(state) to TextSecondary
        }
    var problemsOpen by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 6.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(SurfaceHigh)
            .padding(14.dp)
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(shown.title ?: shown.input, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (shown.title != null) {
                    Text(
                        shown.input,
                        color = TextSecondary,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Spacer(Modifier.height(4.dp))
                Text(label, color = color, style = MaterialTheme.typography.bodyMedium)
                val target = item.playlist
                if (target != null)
                    Text(
                        "→ $target",
                        color = TextSecondary,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                if (target == null && item.playlistFromSource) {
                    Text("→ playlist named after the source", color = TextSecondary, style = MaterialTheme.typography.bodySmall)
                }
            }
            if (item.canCancel) {
                Spacer(Modifier.width(8.dp))
                OutlinedButton(onClick = onCancel) { Text("Cancel") }
            }
        }
        if (item.state == "downloading") {
            Spacer(Modifier.height(10.dp))
            val progress = item.progress
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
        shown.message?.let { message ->
            Spacer(Modifier.height(6.dp))
            Text(message, color = TextSecondary, style = MaterialTheme.typography.bodySmall, maxLines = 8, overflow = TextOverflow.Ellipsis)
        }
        if (shown.failedCount > 0 || shown.warningCount > 0) {
            Spacer(Modifier.height(6.dp))
            Row(
                Modifier.fillMaxWidth().clickable { problemsOpen = !problemsOpen }.padding(vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(AppIcons.Warning, null, tint = WarningColor, modifier = Modifier.size(16.dp))
                Spacer(Modifier.width(6.dp))
                Text(
                    listOfNotNull(
                            shown.failedCount.takeIf { it > 0 }?.let { "$it failed" },
                            shown.warningCount.takeIf { it > 0 }?.let { if (it == 1) "1 warning" else "$it warnings" },
                        )
                        .joinToString(" • "),
                    color = WarningColor,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.weight(1f),
                )
                Icon(
                    AppIcons.Down,
                    if (problemsOpen) "Hide details" else "Show details",
                    tint = TextSecondary,
                    modifier = Modifier.size(18.dp).rotate(if (problemsOpen) 180f else 0f),
                )
            }
            if (problemsOpen) {
                shown.lines.forEach { line ->
                    Text(
                        "• $line",
                        color = TextSecondary,
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 4,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(start = 22.dp, top = 2.dp),
                    )
                }
                val hidden = shown.failedCount + shown.warningCount - shown.lines.size
                if (hidden > 0) {
                    Text(
                        "and $hidden more",
                        color = TextSecondary,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(start = 22.dp, top = 2.dp),
                    )
                }
            }
        }
    }
}
