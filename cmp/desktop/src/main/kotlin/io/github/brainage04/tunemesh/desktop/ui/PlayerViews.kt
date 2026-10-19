package io.github.brainage04.tunemesh.desktop.ui

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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.desktop.playback.RepeatMode
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.TextSecondary

/** Thin white progress slider (Spotify style) shared by the bar and the full player. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ThinSlider(fraction: Float, onChange: (Float) -> Unit, onFinished: () -> Unit, modifier: Modifier = Modifier) {
    Slider(
        value = fraction,
        onValueChange = onChange,
        onValueChangeFinished = onFinished,
        thumb = { Box(Modifier.size(12.dp).background(Color.White, CircleShape)) },
        track = { state ->
            val value = (state.value - state.valueRange.start) / (state.valueRange.endInclusive - state.valueRange.start)
            Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Color.White.copy(alpha = 0.25f))) {
                Box(Modifier.fillMaxWidth(value.coerceIn(0f, 1f)).fillMaxHeight().background(Color.White))
            }
        },
        modifier = modifier.height(20.dp),
    )
}

@Composable
private fun SeekRow(modifier: Modifier = Modifier) {
    val model = LocalAppModel.current
    val ui = LocalPlayerUi.current
    val position by model.player.position.collectAsState()
    var dragging by remember { mutableStateOf<Float?>(null) }
    val duration = ui.durationMs.coerceAtLeast(1)
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        Text(
            formatDuration(dragging?.let { (it * duration).toLong() } ?: position),
            style = MaterialTheme.typography.labelSmall,
            color = TextSecondary,
            textAlign = TextAlign.End,
            modifier = Modifier.widthIn(min = 40.dp),
        )
        ThinSlider(
            fraction = dragging ?: (position.toFloat() / duration).coerceIn(0f, 1f),
            onChange = { dragging = it },
            onFinished = {
                dragging?.let { model.player.seekTo((it * duration).toLong()) }
                dragging = null
            },
            modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
        )
        Text(formatDuration(ui.durationMs), style = MaterialTheme.typography.labelSmall, color = TextSecondary, modifier = Modifier.widthIn(min = 40.dp))
    }
}

@Composable
private fun Transport(playSize: Dp, iconSize: Dp) {
    val model = LocalAppModel.current
    val ui = LocalPlayerUi.current
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = { model.player.toggleShuffle() }) {
            Icon(AppIcons.Shuffle, if (ui.shuffle) "Disable shuffle" else "Enable shuffle", tint = if (ui.shuffle) Accent else TextSecondary, modifier = Modifier.size(iconSize * 0.8f))
        }
        IconButton(onClick = { model.player.previous() }) { Icon(AppIcons.Previous, "Previous", modifier = Modifier.size(iconSize)) }
        PlayCircle(playing = ui.playing, onClick = { model.player.togglePlay() }, enabled = ui.current != null, size = playSize, color = Color.White)
        IconButton(onClick = { model.player.next() }) { Icon(AppIcons.Next, "Next", modifier = Modifier.size(iconSize)) }
        IconButton(onClick = { model.player.cycleRepeat() }) {
            Icon(
                if (ui.repeat == RepeatMode.One) AppIcons.RepeatOne else AppIcons.Repeat,
                when (ui.repeat) {
                    RepeatMode.Off -> "Enable repeat"
                    RepeatMode.All -> "Enable repeat one"
                    RepeatMode.One -> "Disable repeat"
                },
                tint = if (ui.repeat == RepeatMode.Off) TextSecondary else Accent,
                modifier = Modifier.size(iconSize * 0.8f),
            )
        }
    }
}

@Composable
private fun currentTrack(): Track? = LocalPlayerUi.current.current

/** The persistent bottom bar: current track, transport and seek, queue, full player and volume. */
@Composable
fun NowPlayingBar() {
    val model = LocalAppModel.current
    val ui = LocalPlayerUi.current
    val library = LocalLibrary.current
    val track = currentTrack()
    Row(Modifier.fillMaxWidth().height(88.dp).background(Color.Black).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(Modifier.weight(0.3f), verticalAlignment = Alignment.CenterVertically) {
            if (track != null) {
                CoverArt(track, track.album, Modifier.size(56.dp).clickable { model.navigate(Route.NowPlaying) })
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f, fill = false)) {
                    Text(track.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        track.artistLine,
                        style = MaterialTheme.typography.bodySmall,
                        color = TextSecondary,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.clickable { model.openArtist(track.albumArtist) },
                    )
                }
                LikeButton(track.path in library.liked, { model.toggleLike(track) })
            }
        }
        Column(Modifier.weight(0.4f), horizontalAlignment = Alignment.CenterHorizontally) {
            Transport(playSize = 36.dp, iconSize = 24.dp)
            SeekRow(Modifier.fillMaxWidth())
        }
        Row(Modifier.weight(0.3f), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            BarToggle(DesktopIcons.Expand, "Now playing view", model.route == Route.NowPlaying) {
                if (model.route == Route.NowPlaying) model.goBack() else model.navigate(Route.NowPlaying)
            }
            BarToggle(AppIcons.Queue, "Queue", model.queueOpen) { model.queueOpen = !model.queueOpen }
            Icon(DesktopIcons.Volume, "Volume", tint = TextSecondary, modifier = Modifier.padding(start = 8.dp).size(20.dp))
            var volume by remember { mutableStateOf<Float?>(null) }
            ThinSlider(
                fraction = volume ?: (ui.volume / 100f),
                onChange = { volume = it; model.player.setVolume((it * 100).toInt()) },
                onFinished = { volume = null },
                modifier = Modifier.width(110.dp).padding(horizontal = 8.dp),
            )
        }
    }
}

@Composable
private fun BarToggle(icon: ImageVector, description: String, active: Boolean, onClick: () -> Unit) {
    IconButton(onClick = onClick) { Icon(icon, description, tint = if (active) Accent else TextSecondary, modifier = Modifier.size(20.dp)) }
}

/** Full player: large art beside title, seek and transport, over the album's colour. */
@Composable
fun NowPlayingScreen() {
    val model = LocalAppModel.current
    val ui = LocalPlayerUi.current
    val library = LocalLibrary.current
    val track = currentTrack()
    val seed = track?.album.orEmpty()
    val hue = ((seed.hashCode() and 0x7fffffff) % 360).toFloat()
    Row(
        Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(Color.hsv(hue, 0.5f, 0.38f), Color(0xFF121212)))).padding(40.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(40.dp),
    ) {
        CoverArt(track, seed, Modifier.weight(1f).widthIn(max = 520.dp).aspectRatio(1f), shape = RoundedCornerShape(8.dp), sizeHint = 520.dp)
        Column(Modifier.weight(1f)) {
            if (track == null) {
                Text("Nothing playing", style = MaterialTheme.typography.headlineSmall)
                return@Column
            }
            Text("PLAYING FROM", style = MaterialTheme.typography.labelSmall, color = TextSecondary)
            Text(ui.context, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.height(24.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(track.title, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(
                        track.artistLine,
                        style = MaterialTheme.typography.titleMedium,
                        color = TextSecondary,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.clickable { model.openArtist(track.albumArtist) },
                    )
                    Text(
                        track.album,
                        style = MaterialTheme.typography.bodyMedium,
                        color = TextSecondary,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.clickable { model.openAlbum(track) },
                    )
                }
                LikeButton(track.path in library.liked, { model.toggleLike(track) }, size = 28.dp)
            }
            Spacer(Modifier.height(16.dp))
            SeekRow(Modifier.fillMaxWidth())
            Spacer(Modifier.height(8.dp))
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { Transport(playSize = 64.dp, iconSize = 36.dp) }
        }
    }
}

/** Right-hand queue panel: now playing, next up, played. */
@Composable
fun QueuePanel(modifier: Modifier = Modifier) {
    val model = LocalAppModel.current
    val ui = LocalPlayerUi.current
    Column(modifier.background(MaterialTheme.colorScheme.surfaceContainerLow)) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Queue", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            IconButton(onClick = { model.queueOpen = false }) { Icon(AppIcons.Close, "Close queue", tint = TextSecondary) }
        }
        val current = ui.currentIndex
        LazyColumn(Modifier.fillMaxSize()) {
            val now = ui.current
            if (now == null) {
                item { QueueHeader("Nothing is playing") }
                return@LazyColumn
            }
            item { QueueHeader("Now playing") }
            item { QueueRow(now, current, isCurrent = true, last = ui.queue.lastIndex) }
            val upcoming = ui.queue.withIndex().drop(current + 1)
            if (upcoming.isNotEmpty()) {
                item { QueueHeader(if (ui.context.isEmpty()) "Next up" else "Next from: ${ui.context}") }
                items(upcoming, key = { it.index }) { entry -> QueueRow(entry.value, entry.index, isCurrent = false, last = ui.queue.lastIndex) }
            }
            val played = ui.queue.withIndex().take(current.coerceAtLeast(0))
            if (played.isNotEmpty()) {
                item { QueueHeader("Played") }
                items(played, key = { "p" + it.index }) { entry -> QueueRow(entry.value, entry.index, isCurrent = false, last = ui.queue.lastIndex) }
            }
        }
    }
}

@Composable
private fun QueueHeader(text: String) {
    Text(text, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold, color = TextSecondary, modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 8.dp))
}

@Composable
private fun QueueRow(track: Track, index: Int, isCurrent: Boolean, last: Int) {
    val model = LocalAppModel.current
    Row(
        Modifier.fillMaxWidth().clickable(enabled = !isCurrent) { model.player.skipTo(index) }.padding(start = 16.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CoverArt(track, track.album, Modifier.size(40.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(track.title, color = if (isCurrent) Accent else Color.White, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(track.artistLine, color = TextSecondary, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (!isCurrent) {
            IconButton(onClick = { model.player.move(index, index - 1) }, enabled = index > 0) { Icon(AppIcons.Up, "Move up", tint = TextSecondary, modifier = Modifier.size(18.dp)) }
            IconButton(onClick = { model.player.move(index, index + 1) }, enabled = index < last) { Icon(AppIcons.DownArrow, "Move down", tint = TextSecondary, modifier = Modifier.size(18.dp)) }
            IconButton(onClick = { model.player.removeAt(index) }) { Icon(AppIcons.Close, "Remove from queue", tint = TextSecondary, modifier = Modifier.size(18.dp)) }
        } else {
            Spacer(Modifier.width(16.dp))
        }
    }
}
