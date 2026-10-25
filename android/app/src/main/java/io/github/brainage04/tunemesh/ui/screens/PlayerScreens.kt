package io.github.brainage04.tunemesh.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import io.github.brainage04.tunemesh.library.Limits
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.playback.QueueItems
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.CoverArt
import io.github.brainage04.tunemesh.ui.LikeButton
import io.github.brainage04.tunemesh.ui.LocalAppViewModel
import io.github.brainage04.tunemesh.ui.LocalLibrary
import io.github.brainage04.tunemesh.ui.LocalPlayerUi
import io.github.brainage04.tunemesh.ui.PlayCircle
import io.github.brainage04.tunemesh.ui.RowContext
import io.github.brainage04.tunemesh.ui.TrackMenu
import io.github.brainage04.tunemesh.ui.formatDuration
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

@Composable
private fun currentTrack(): Track? {
    val path = LocalPlayerUi.current.currentPath ?: return null
    return LocalLibrary.current.index.byPath[path]
}

private fun MediaItem.title() = mediaMetadata.title?.toString().orEmpty()

private fun MediaItem.artist() = mediaMetadata.artist?.toString().orEmpty()

@Composable
fun MiniPlayer(modifier: Modifier = Modifier) {
    val viewModel = LocalAppViewModel.current
    val ui = LocalPlayerUi.current
    val item = ui.current ?: return
    val track = currentTrack()
    val position by viewModel.player.position.collectAsState()
    val library = LocalLibrary.current
    Column(
        modifier.padding(horizontal = 8.dp).clip(RoundedCornerShape(6.dp)).background(Color(0xFF3A2F3A)).clickable {
            viewModel.playerExpanded = true
        }
    ) {
        Row(Modifier.padding(start = 8.dp, top = 8.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            CoverArt(track, item.mediaMetadata.albumTitle?.toString().orEmpty(), Modifier.size(40.dp))
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    Limits.shownLine(item.title()),
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    Limits.shownLine(item.artist()),
                    style = MaterialTheme.typography.bodySmall,
                    color = TextSecondary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (track != null) LikeButton(track.path in library.liked, { viewModel.toggleLike(track) })
            IconButton(onClick = { viewModel.player.togglePlay() }) {
                Icon(if (ui.showPause) AppIcons.Pause else AppIcons.Play, if (ui.showPause) "Pause" else "Play")
            }
        }
        LinearProgressIndicator(
            progress = { if (ui.durationMs > 0) (position.toFloat() / ui.durationMs).coerceIn(0f, 1f) else 0f },
            modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp).height(2.dp),
            color = Color.White,
            trackColor = Color.White.copy(alpha = 0.2f),
            drawStopIndicator = {},
            gapSize = 0.dp,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NowPlayingScreen() {
    val viewModel = LocalAppViewModel.current
    val ui = LocalPlayerUi.current
    val library = LocalLibrary.current
    val item = ui.current
    val track = currentTrack()
    val position by viewModel.player.position.collectAsState()
    var dragging by remember { mutableStateOf<Float?>(null) }
    var menuOpen by remember { mutableStateOf(false) }
    val seed = item?.mediaMetadata?.albumTitle?.toString().orEmpty()
    val hue = ((seed.hashCode() and 0x7fffffff) % 360).toFloat()
    Column(
        Modifier.fillMaxSize()
            .background(Brush.verticalGradient(listOf(Color.hsv(hue, 0.5f, 0.38f), Color(0xFF121212))))
            .systemBarsPadding()
            .padding(horizontal = 24.dp)
    ) {
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { viewModel.playerExpanded = false }) { Icon(AppIcons.Down, "Close player") }
            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("PLAYING FROM", style = MaterialTheme.typography.labelSmall, color = TextSecondary)
                Text(
                    Limits.shownLine(viewModel.playingFrom),
                    style = MaterialTheme.typography.bodyMedium,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Box {
                IconButton(onClick = { menuOpen = true }, enabled = track != null) { Icon(AppIcons.More, "More options") }
                if (track != null) TrackMenu(track, RowContext(), menuOpen) { menuOpen = false }
            }
        }
        Spacer(Modifier.weight(1f))
        CoverArt(track, seed, Modifier.fillMaxWidth().aspectRatio(1f), shape = RoundedCornerShape(8.dp), sizeHint = 400.dp)
        Spacer(Modifier.weight(1f))
        if (item == null) {
            Text("Nothing playing", style = MaterialTheme.typography.titleLarge)
            return@Column
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(
                    Limits.shownLine(item.title()),
                    style = MaterialTheme.typography.headlineSmall,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    Limits.shownLine(item.artist()),
                    style = MaterialTheme.typography.bodyLarge,
                    color = TextSecondary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable(enabled = track != null) { track?.let { viewModel.openArtist(it.albumArtist) } },
                )
            }
            if (track != null) LikeButton(track.path in library.liked, { viewModel.toggleLike(track) }, size = 28.dp)
        }
        Spacer(Modifier.height(12.dp))
        val duration = ui.durationMs.coerceAtLeast(1)
        Slider(
            value = dragging ?: (position.toFloat() / duration).coerceIn(0f, 1f),
            onValueChange = { dragging = it },
            onValueChangeFinished = {
                dragging?.let { viewModel.player.seekTo((it * duration).toLong()) }
                dragging = null
            },
            thumb = { Box(Modifier.size(14.dp).background(Color.White, CircleShape)) },
            track = { state ->
                val fraction = (state.value - state.valueRange.start) / (state.valueRange.endInclusive - state.valueRange.start)
                Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Color.White.copy(alpha = 0.25f))) {
                    Box(Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).fillMaxHeight().background(Color.White))
                }
            },
            modifier = Modifier.fillMaxWidth().height(28.dp),
        )
        Row(Modifier.fillMaxWidth()) {
            Text(
                formatDuration(dragging?.let { (it * duration).toLong() } ?: position),
                style = MaterialTheme.typography.labelSmall,
                color = TextSecondary,
                modifier = Modifier.weight(1f),
            )
            Text(formatDuration(ui.durationMs), style = MaterialTheme.typography.labelSmall, color = TextSecondary)
        }
        Spacer(Modifier.height(8.dp))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { viewModel.player.toggleShuffle() }) {
                Icon(
                    AppIcons.Shuffle,
                    if (ui.shuffle) "Shuffle on" else "Shuffle off",
                    tint = if (ui.shuffle) Accent else Color.White,
                    modifier = Modifier.size(26.dp),
                )
            }
            IconButton(onClick = { viewModel.player.previous() }, modifier = Modifier.size(56.dp)) {
                Icon(AppIcons.Previous, "Previous", modifier = Modifier.size(40.dp))
            }
            PlayCircle(playing = ui.showPause, onClick = { viewModel.player.togglePlay() }, size = 68.dp, color = Color.White)
            IconButton(onClick = { viewModel.player.next() }, modifier = Modifier.size(56.dp)) {
                Icon(AppIcons.Next, "Next", modifier = Modifier.size(40.dp))
            }
            IconButton(onClick = { viewModel.player.cycleRepeat() }) {
                Icon(
                    if (ui.repeatMode == Player.REPEAT_MODE_ONE) AppIcons.RepeatOne else AppIcons.Repeat,
                    when (ui.repeatMode) {
                        Player.REPEAT_MODE_ONE -> "Repeat one"
                        Player.REPEAT_MODE_ALL -> "Repeat all"
                        else -> "Repeat off"
                    },
                    tint = if (ui.repeatMode == Player.REPEAT_MODE_OFF) Color.White else Accent,
                    modifier = Modifier.size(26.dp),
                )
            }
        }
        Row(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalArrangement = Arrangement.End) {
            IconButton(onClick = { viewModel.queueOpen = true }) { Icon(AppIcons.Queue, "Queue") }
        }
    }
}

@Composable
fun QueueScreen() {
    val viewModel = LocalAppViewModel.current
    val ui = LocalPlayerUi.current
    val library = LocalLibrary.current
    val listState = rememberLazyListState(initialFirstVisibleItemIndex = 0)
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).systemBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { viewModel.queueOpen = false }) { Icon(AppIcons.Down, "Close queue") }
            Text("Queue", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
        }
        val current = ui.currentIndex
        LazyColumn(state = listState, modifier = Modifier.fillMaxSize()) {
            ui.current?.let { item ->
                item { QueueHeader("Now playing") }
                item { QueueRow(item, library.index.byPath[QueueItems.path(item)], current, isCurrent = true, last = ui.queue.lastIndex) }
            }
            val upcoming = ui.queue.withIndex().drop(current + 1)
            if (upcoming.isNotEmpty()) {
                item { QueueHeader("Next up") }
                // Keyed by position too: Compose crashes on a repeated key, and IDs come from other apps' controllers.
                itemsIndexed(upcoming, key = { _, entry -> "${entry.index}|${entry.value.mediaId}" }) { _, entry ->
                    QueueRow(
                        entry.value,
                        library.index.byPath[QueueItems.path(entry.value)],
                        entry.index,
                        isCurrent = false,
                        last = ui.queue.lastIndex,
                    )
                }
            }
            val played = ui.queue.withIndex().take(current.coerceAtLeast(0))
            if (played.isNotEmpty()) {
                item { QueueHeader("Played") }
                itemsIndexed(played, key = { _, entry -> "${entry.index}|${entry.value.mediaId}" }) { _, entry ->
                    QueueRow(
                        entry.value,
                        library.index.byPath[QueueItems.path(entry.value)],
                        entry.index,
                        isCurrent = false,
                        last = ui.queue.lastIndex,
                    )
                }
            }
        }
    }
}

@Composable
private fun QueueHeader(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleMedium,
        fontWeight = FontWeight.Bold,
        modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 8.dp),
    )
}

@Composable
private fun QueueRow(item: MediaItem, track: Track?, index: Int, isCurrent: Boolean, last: Int) {
    val viewModel = LocalAppViewModel.current
    Row(
        Modifier.fillMaxWidth()
            .clickable(enabled = !isCurrent) { viewModel.player.skipTo(index) }
            .padding(start = 16.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CoverArt(track, item.mediaMetadata.albumTitle?.toString().orEmpty(), Modifier.size(44.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                Limits.shownLine(item.title()),
                color = if (isCurrent) Accent else Color.White,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                Limits.shownLine(item.artist()),
                color = TextSecondary,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (!isCurrent) {
            IconButton(onClick = { viewModel.player.move(index, index - 1) }, enabled = index > 0) {
                Icon(AppIcons.Up, "Move up", tint = TextSecondary)
            }
            IconButton(onClick = { viewModel.player.move(index, index + 1) }, enabled = index < last) {
                Icon(AppIcons.DownArrow, "Move down", tint = TextSecondary)
            }
            IconButton(onClick = { viewModel.player.removeAt(index) }) { Icon(AppIcons.Close, "Remove from queue", tint = TextSecondary) }
        } else {
            Spacer(Modifier.width(16.dp))
        }
    }
}
