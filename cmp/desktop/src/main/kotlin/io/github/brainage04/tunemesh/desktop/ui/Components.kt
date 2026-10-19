package io.github.brainage04.tunemesh.desktop.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.PointerMatcher
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.onClick
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerButton
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.brainage04.tunemesh.desktop.data.LibraryState
import io.github.brainage04.tunemesh.desktop.playback.PlayerUi
import io.github.brainage04.tunemesh.library.LibraryFormat
import io.github.brainage04.tunemesh.library.Playlist
import io.github.brainage04.tunemesh.library.Track
import io.github.brainage04.tunemesh.ui.AppIcons
import io.github.brainage04.tunemesh.ui.theme.Accent
import io.github.brainage04.tunemesh.ui.theme.TextSecondary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

val LocalAppModel = staticCompositionLocalOf<AppModel> { error("AppModel not provided") }
val LocalLibrary = compositionLocalOf<LibraryState> { error("LibraryState not provided") }
val LocalPlayerUi = compositionLocalOf { PlayerUi() }

private val coverDispatcher = Dispatchers.IO.limitedParallelism(2)

/** Deterministic dark gradient used when a track, album or playlist has no art. */
fun placeholderBrush(seed: String): Brush {
    val hue = ((seed.hashCode() and 0x7fffffff) % 360).toFloat()
    return Brush.linearGradient(listOf(Color.hsv(hue, 0.55f, 0.55f), Color.hsv((hue + 40f) % 360f, 0.65f, 0.25f)))
}

/** Album art with a memory cache and a gradient placeholder. */
@Composable
fun CoverArt(
    track: Track?,
    seed: String,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(4.dp),
    sizeHint: Dp = 48.dp,
    placeholderIcon: ImageVector = AppIcons.MusicNote,
) {
    val model = LocalAppModel.current
    val sizePx = when {
        sizeHint <= 64.dp -> 96
        sizeHint <= 200.dp -> 320
        else -> 600
    }
    val bitmap by produceState<ImageBitmap?>(
        initialValue = track?.let { model.covers.cached(it, sizePx) },
        track?.path,
        track?.modifiedMs,
        sizePx,
    ) {
        value = track?.let { model.covers.cached(it, sizePx) }
        if (value == null && track != null) {
            value = withContext(coverDispatcher) { model.covers.load(model.repository.root, track, sizePx) }
        }
    }
    Box(modifier.clip(shape).background(placeholderBrush(seed)), contentAlignment = Alignment.Center) {
        val image = bitmap
        if (image != null) {
            Image(image, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        } else {
            BoxWithConstraints(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Icon(placeholderIcon, null, tint = Color.White.copy(alpha = 0.7f), modifier = Modifier.size(maxWidth * 0.42f))
            }
        }
    }
}

/** Liked Songs tile: purple-blue gradient with a heart. */
@Composable
fun LikedSongsArt(modifier: Modifier = Modifier, shape: Shape = RoundedCornerShape(4.dp)) {
    BoxWithConstraints(
        modifier.clip(shape).background(Brush.linearGradient(listOf(Color(0xFF4A1FD6), Color(0xFF8BB4E8)))),
        contentAlignment = Alignment.Center,
    ) {
        Icon(AppIcons.Favorite, null, tint = Color.White, modifier = Modifier.size(maxWidth * 0.42f))
    }
}

/** Liked Songs gets its heart tile; other playlists use the art of their first available track. */
@Composable
fun PlaylistArt(playlist: Playlist, modifier: Modifier, sizeHint: Dp, shape: Shape = RoundedCornerShape(4.dp)) {
    if (playlist.isLikedSongs) {
        LikedSongsArt(modifier, shape)
        return
    }
    val library = LocalLibrary.current
    val first = playlist.entries.firstNotNullOfOrNull { entry -> entry.path?.let { library.index.byPath[it] } }
    CoverArt(first, playlist.name, modifier, shape = shape, sizeHint = sizeHint, placeholderIcon = AppIcons.Queue)
}

@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.titleLarge,
        fontWeight = FontWeight.Bold,
        modifier = modifier.padding(start = 24.dp, end = 24.dp, top = 24.dp, bottom = 8.dp),
    )
}

@Composable
fun PageTitle(text: String, actions: @Composable () -> Unit = {}) {
    Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 16.dp, top = 16.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(text, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
        actions()
    }
}

@Composable
fun LikeButton(liked: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier, size: Dp = 20.dp) {
    IconButton(onClick = onClick, modifier = modifier) {
        Icon(
            if (liked) AppIcons.Favorite else AppIcons.FavoriteBorder,
            contentDescription = if (liked) "Remove from Liked Songs" else "Save to Liked Songs",
            tint = if (liked) Accent else TextSecondary,
            modifier = Modifier.size(size),
        )
    }
}

/** Where a track row is shown, so its menu can offer context-specific entries. */
data class RowContext(
    val playlistName: String? = null,
    val index: Int = -1,
    val showAlbum: Boolean = true,
    val showArtist: Boolean = true,
    val showCover: Boolean = true,
)

/** Desktop track row: number, cover and title/artist, album column, like, duration and a menu (also on right click). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TrackRow(track: Track, number: Int, onPlay: () -> Unit, context: RowContext = RowContext()) {
    val model = LocalAppModel.current
    val library = LocalLibrary.current
    val isCurrent = LocalPlayerUi.current.currentPath == track.path
    var menuOpen by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp)
            .clip(RoundedCornerShape(4.dp))
            .onClick(matcher = PointerMatcher.mouse(PointerButton.Secondary)) { menuOpen = true }
            .clickable(onClick = onPlay)
            .padding(start = 8.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            number.toString(),
            color = if (isCurrent) Accent else TextSecondary,
            textAlign = TextAlign.End,
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.widthIn(min = 28.dp),
        )
        Spacer(Modifier.width(16.dp))
        if (context.showCover) {
            CoverArt(track, track.album, Modifier.size(40.dp))
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1.4f)) {
            Text(track.title, color = if (isCurrent) Accent else Color.White, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (context.showArtist) {
                Text(track.artistLine, color = TextSecondary, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (context.showAlbum) {
            Text(
                track.album,
                color = TextSecondary,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).padding(horizontal = 12.dp).clickable { model.openAlbum(track) },
            )
        }
        LikeButton(track.path in library.liked, { model.toggleLike(track) })
        Text(formatDuration(track.durationMs), color = TextSecondary, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.widthIn(min = 44.dp), textAlign = TextAlign.End)
        Box {
            IconButton(onClick = { menuOpen = true }) { Icon(AppIcons.More, "More options", tint = TextSecondary) }
            TrackMenu(track, context, menuOpen) { menuOpen = false }
        }
    }
}

@Composable
fun TrackMenu(track: Track, context: RowContext, expanded: Boolean, onDismiss: () -> Unit) {
    val model = LocalAppModel.current
    val liked = track.path in LocalLibrary.current.liked
    DropdownMenu(expanded = expanded, onDismissRequest = onDismiss) {
        MenuItem("Play next", AppIcons.Queue) { model.player.playNext(listOf(track)); onDismiss() }
        MenuItem("Add to queue", AppIcons.Queue) { model.player.addToQueue(listOf(track)); onDismiss() }
        MenuItem(if (liked) "Remove from Liked Songs" else "Save to Liked Songs", if (liked) AppIcons.Favorite else AppIcons.FavoriteBorder) {
            model.toggleLike(track); onDismiss()
        }
        MenuItem("Add to playlist", AppIcons.PlaylistAdd) { model.dialog = DialogRequest.AddToPlaylist(listOf(track)); onDismiss() }
        if (context.playlistName != null && context.playlistName != LibraryFormat.LIKED_SONGS) {
            MenuItem("Remove from this playlist", AppIcons.Close) {
                model.repository.removeFromPlaylist(context.playlistName, context.index, track.path); onDismiss()
            }
        }
        MenuItem("Go to album", AppIcons.Album) { model.openAlbum(track); onDismiss() }
        MenuItem("Go to artist", AppIcons.Person) { model.openArtist(track.albumArtist); onDismiss() }
    }
}

@Composable
fun MenuItem(text: String, icon: ImageVector, color: Color = Color.White, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(text, color = color) },
        leadingIcon = { Icon(icon, null, tint = if (color == Color.White) TextSecondary else color) },
        onClick = onClick,
    )
}

/** Playlist entry whose file is not (yet) in the library. */
@Composable
fun UnavailableRow(raw: String, number: Int, onRemove: (() -> Unit)?) {
    Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 12.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(number.toString(), color = Color(0xFF6A6A6A), textAlign = TextAlign.End, modifier = Modifier.widthIn(min = 28.dp))
        Spacer(Modifier.width(16.dp))
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(4.dp)).background(Color(0xFF2A2A2A)), contentAlignment = Alignment.Center) {
            Icon(AppIcons.MusicNote, null, tint = Color(0xFF5A5A5A))
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(LibraryFormat.titleFromFileName(raw.substringAfterLast('/')), color = Color(0xFF6A6A6A), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text("Unavailable on this device", color = Color(0xFF6A6A6A), style = MaterialTheme.typography.bodyMedium)
        }
        if (onRemove != null) IconButton(onClick = onRemove) { Icon(AppIcons.Close, "Remove", tint = TextSecondary) }
    }
}

/** Square tile for albums, artists and playlists in rows and grids. */
@Composable
fun MediaTile(
    title: String,
    subtitle: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    circle: Boolean = false,
    art: @Composable (Modifier) -> Unit,
) {
    Column(modifier.clip(RoundedCornerShape(8.dp)).clickable(onClick = onClick).padding(10.dp)) {
        art(Modifier.fillMaxWidth().aspectRatio(1f))
        Spacer(Modifier.height(8.dp))
        Text(title, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = TextSecondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** Header of album, artist and playlist pages: art beside the title, then shuffle and play buttons. */
@Composable
fun DetailHeader(
    kind: String,
    title: String,
    subtitle: String,
    seed: String,
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    playEnabled: Boolean,
    art: @Composable (Modifier) -> Unit,
    playingThis: Boolean = false,
    onSubtitleClick: (() -> Unit)? = null,
    actions: @Composable () -> Unit = {},
) {
    val hue = ((seed.hashCode() and 0x7fffffff) % 360).toFloat()
    Column(Modifier.fillMaxWidth().background(Brush.verticalGradient(listOf(Color.hsv(hue, 0.45f, 0.42f), MaterialTheme.colorScheme.background)))) {
        Row(Modifier.padding(start = 24.dp, end = 24.dp, top = 32.dp, bottom = 16.dp), verticalAlignment = Alignment.Bottom) {
            art(Modifier.size(200.dp))
            Spacer(Modifier.width(24.dp))
            Column {
                Text(kind, style = MaterialTheme.typography.labelLarge)
                Text(title, style = MaterialTheme.typography.displaySmall, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(8.dp))
                Text(
                    subtitle,
                    style = MaterialTheme.typography.bodyLarge,
                    color = Color.White.copy(alpha = 0.85f),
                    modifier = if (onSubtitleClick != null) Modifier.clickable(onClick = onSubtitleClick) else Modifier,
                )
            }
        }
        Row(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            PlayCircle(playing = playingThis, onClick = onPlay, enabled = playEnabled)
            IconButton(onClick = onShuffle, enabled = playEnabled) {
                Icon(AppIcons.Shuffle, "Shuffle play", tint = TextSecondary, modifier = Modifier.size(28.dp))
            }
            actions()
        }
    }
}

@Composable
fun PlayCircle(playing: Boolean, onClick: () -> Unit, enabled: Boolean = true, size: Dp = 56.dp, color: Color = Accent) {
    FilledIconButton(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.size(size),
        shape = CircleShape,
        colors = IconButtonDefaults.filledIconButtonColors(containerColor = color, contentColor = Color.Black),
    ) {
        Icon(if (playing) AppIcons.Pause else AppIcons.Play, if (playing) "Pause" else "Play", modifier = Modifier.size(size * 0.5f))
    }
}

@Composable
fun EmptyMessage(title: String, body: String, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Text(body, color = TextSecondary, style = MaterialTheme.typography.bodyMedium)
    }
}

fun formatDuration(ms: Long): String {
    val totalSeconds = (ms / 1000).coerceAtLeast(0)
    val hours = totalSeconds / 3600
    val minutes = (totalSeconds % 3600) / 60
    val seconds = totalSeconds % 60
    return if (hours > 0) "%d:%02d:%02d".format(hours, minutes, seconds) else "%d:%02d".format(minutes, seconds)
}

fun totalLength(tracks: List<Track>): String {
    val minutes = tracks.sumOf { it.durationMs } / 60_000
    return if (minutes >= 60) "${minutes / 60} h ${minutes % 60} min" else "$minutes min"
}

/** Multi-valued artist tags (`A; B`) as shown to the user. */
val Track.artistLine: String get() = artist.split(';', '\u0000').map { it.trim() }.filter { it.isNotEmpty() }.joinToString(", ")

fun songCount(count: Int) = if (count == 1) "1 song" else "$count songs"
